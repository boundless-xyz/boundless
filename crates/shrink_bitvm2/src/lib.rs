use std::path::Path;

use hex::FromHex;
use risc0_zkvm::{
    digest, seal_to_json, sha::Digestible, Digest, Groth16ProofJson, Groth16Receipt, Groth16Seal,
    Journal, Receipt, ReceiptClaim, SuccinctReceipt,
};

use anyhow::{ensure, Context, Result};
use num_bigint::BigUint;
use num_traits::Num;

use tokio::process::Command;
pub const ALLOWED_CONTROL_ROOT: Digest =
    digest!("ce52bf56033842021af3cf6db8a50d1b7535c125a34f1a22c6fdcf002c5a1529");

pub const BN254_IDENTITY_CONTROL_ID: Digest =
    digest!("c07a65145c3cb48b6101962ea607a4dd93c753bb26975cb47feb00d3666e4404");

pub async fn prove_and_verify(
    proof_id: &str,
    work_dir: &Path,
    p254_receipt: SuccinctReceipt<ReceiptClaim>,
    journal: Journal,
) -> Result<Receipt> {
    let receipt_claim = p254_receipt.claim.clone().value()?;

    let seal_path = work_dir.join("input.json");
    let proof_path = work_dir.join("proof.json");
    let public_path = work_dir.join("public.json");

    write_seal(&journal.bytes, p254_receipt, &receipt_claim, &seal_path).await?;

    let proof_json = generate_proof(work_dir, &proof_path).await?;
    tracing::info!("{proof_id}: generated shrink groth16 proof");

    let image_id = receipt_claim.pre.digest();
    let final_receipt = finalize(journal.bytes, receipt_claim, proof_json)?;
    let output_bytes = decode_output(&public_path).await?;
    tracing::info!("bvm2 decoded output byte length: {}", output_bytes.len());

    check_output(image_id, &final_receipt, &output_bytes)?;

    verify_proof(&final_receipt, &output_bytes)?;

    Ok(final_receipt)
}

pub fn calculate_output_prefix(control_id: &Digest, image_id: &[u8]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};

    let verifier_params = risc0_zkvm::SuccinctReceiptVerifierParameters::default();
    let control_root = verifier_params.control_root;
    let mut control_root_bytes: [u8; 32] = control_root.as_bytes().try_into().unwrap();
    for byte in &mut control_root_bytes {
        *byte = byte.reverse_bits();
    }
    let pre_state_bytes = image_id.to_vec();
    let control_id_bytes = control_id.as_bytes();

    // Expected post state for an execution that halted successfully
    let post_state = risc0_zkvm::SystemState { pc: 0, merkle_root: Digest::default() };
    let post_state_bytes: [u8; 32] = post_state.digest().into();

    let mut hasher = Sha256::new();
    hasher.update(control_root_bytes);
    hasher.update(&pre_state_bytes);
    hasher.update(post_state_bytes);
    hasher.update(control_id_bytes);

    hasher.finalize().into()
}

pub fn check_output(image_id: Digest, final_receipt: &Receipt, output_bytes: &[u8]) -> Result<()> {
    let expected_output_bytes = blake3_claim_digest(&image_id, &final_receipt.journal.bytes);

    let expected_output_bytes: [u8; 31] = expected_output_bytes[1..=31].try_into()?;
    tracing::info!("check output: computed output: {}", hex::encode(expected_output_bytes));

    ensure!(expected_output_bytes == output_bytes, "check output: public output mismatch");

    Ok(())
}

pub fn blake3_claim_digest(image_id: &Digest, journal_bytes: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&calculate_output_prefix(&BN254_IDENTITY_CONTROL_ID, image_id.as_bytes()));
    hasher.update(journal_bytes);
    let mut digest_bytes: [u8; 32] = hasher.finalize().into();
    // trim to 31 bytes
    digest_bytes[31] = 0;
    // shift because of endianness
    digest_bytes.rotate_right(1);
    digest_bytes
}

pub async fn decode_output(public_path: &Path) -> Result<Vec<u8>> {
    let output_content_dec = tokio::fs::read_to_string(public_path).await?;

    // Convert output_content_dec from decimal to hex
    let parsed_json: serde_json::Value = serde_json::from_str(&output_content_dec)?;
    let output_str = parsed_json[0].as_str().context("read public output from json")?;
    let output_content_hex = BigUint::from_str_radix(output_str, 10)?.to_str_radix(16);

    // If the length of the hexadecimal string is odd, add a leading zero
    let output_content_hex = if output_content_hex.len() % 2 == 0 {
        output_content_hex
    } else {
        format!("0{output_content_hex}")
    };

    Ok(hex::decode(&output_content_hex)?)
}

pub fn finalize(
    journal_bytes: Vec<u8>,
    receipt_claim: ReceiptClaim,
    proof_json: Groth16ProofJson,
) -> Result<Receipt> {
    let seal: Groth16Seal = proof_json.try_into()?;
    let verifier_parameters_digest =
        Digest::from_hex("b72859b60cfe0bb13cbde70859fbc67ef9dbd5410bbe66bdb7be64a3dcf6814e")
            .unwrap(); // TODO(ec2): dont hardcode this (actually not sure if this is ever even used, so could be digest zero)
    let groth16_receipt =
        Groth16Receipt::new(seal.to_vec(), receipt_claim.into(), verifier_parameters_digest);
    let receipt = Receipt::new(risc0_zkvm::InnerReceipt::Groth16(groth16_receipt), journal_bytes);
    Ok(receipt)
}

pub async fn write_seal(
    journal_bytes: &[u8],
    p254_receipt: SuccinctReceipt<ReceiptClaim>,
    receipt_claim: &ReceiptClaim,
    seal_path: &Path,
) -> Result<()> {
    tracing::info!("Writing seal");

    let seal_bytes = p254_receipt.get_seal_bytes();
    let mut seal_json = Vec::new();
    seal_to_json(seal_bytes.as_slice(), &mut seal_json)?;
    let mut seal_json: serde_json::Value =
        serde_json::from_str(std::str::from_utf8(seal_json.as_slice())?)?;

    let mut journal_bits = Vec::new();
    for byte in journal_bytes {
        for i in 0..8 {
            journal_bits.push((byte >> (7 - i)) & 1);
        }
    }

    let pre_state_digest_bits: Vec<_> = receipt_claim
        .pre
        .digest()
        .as_bytes()
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| ((byte >> i) & 1).to_string()))
        .collect();

    let post_state_digest_bits: Vec<_> = receipt_claim
        .post
        .digest()
        .as_bytes()
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| ((byte >> i) & 1).to_string()))
        .collect();

    let mut id_bn254_fr_bits: Vec<String> = p254_receipt
        .control_id
        .as_bytes()
        .iter()
        .flat_map(|&byte| (0..8).rev().map(move |i| ((byte >> i) & 1).to_string()))
        .collect();
    id_bn254_fr_bits.remove(248);
    id_bn254_fr_bits.remove(248);

    let mut succinct_control_root_bytes: [u8; 32] =
        risc0_zkvm::SuccinctReceiptVerifierParameters::default()
            .control_root
            .as_bytes()
            .try_into()?;

    succinct_control_root_bytes.reverse();
    let succinct_control_root_hex = hex::encode(succinct_control_root_bytes);

    let a1_str = succinct_control_root_hex[0..32].to_string();
    let a0_str = succinct_control_root_hex[32..64].to_string();
    let a0_dec = to_decimal(&a0_str).context("a0_str returned None")?;
    let a1_dec = to_decimal(&a1_str).context("a1_str returned None")?;

    let control_root = vec![a0_dec, a1_dec];

    seal_json["journal_digest_bits"] = journal_bits.into();
    seal_json["pre_state_digest_bits"] = pre_state_digest_bits.into();
    seal_json["post_state_digest_bits"] = post_state_digest_bits.into();
    seal_json["id_bn254_fr_bits"] = id_bn254_fr_bits.into();
    seal_json["control_root"] = control_root.into();

    tokio::fs::write(seal_path, serde_json::to_string_pretty(&seal_json)?).await?;

    Ok(())
}

// #!/bin/bash
//
// set -eoux
//
// ulimit -s unlimited
// ./verify_for_guest /mnt/input.json output.wtns
// rapidsnark verify_for_guest_final.zkey output.wtns /mnt/proof.json /mnt/public.json

pub async fn generate_proof(work_dir: &Path, proof_path: &Path) -> Result<Groth16ProofJson> {
    tracing::info!("docker run ozancw/risc0-to-bitvm2-groth16-prover");

    let volume = format!("{}:/mnt", work_dir.display());
    let status = Command::new("docker")
        .args(["run", "--rm", "-v", &volume, "ozancw/risc0-to-bitvm2-groth16-prover"])
        .status()
        .await?;

    anyhow::ensure!(
        status.success(),
        "ozancw/risc0-to-bitvm2-groth16-prover failed: {:?}",
        status.code()
    );

    let proof_content = tokio::fs::read_to_string(proof_path).await?;
    let proof_json: Groth16ProofJson = serde_json::from_str(&proof_content)?;

    Ok(proof_json)
}

fn to_decimal(s: &str) -> Option<String> {
    let int = BigUint::from_str_radix(s, 16).ok();
    int.map(|n| n.to_str_radix(10))
}

pub fn verify_proof(final_receipt: &Receipt, output_bytes: &[u8]) -> Result<()> {
    tracing::info!("verify_proof output bytes: {}", hex::encode(output_bytes));
    use ark_ff::PrimeField;

    let ark_proof = from_seal(&final_receipt.inner.groth16()?.seal);
    let public_input_scalar = ark_bn254::Fr::from_be_bytes_mod_order(output_bytes);
    let ark_vk = get_ark_verifying_key();
    let ark_pvk = ark_groth16::prepare_verifying_key(&ark_vk);
    let res = ark_groth16::Groth16::<ark_bn254::Bn254>::verify_proof(
        &ark_pvk,
        &ark_proof,
        &[public_input_scalar],
    )
    .unwrap();
    tracing::info!("Shrink bitvm2 Proof verification result: {:?}", res);

    ensure!(res, "proof verification failed");

    Ok(())
}

pub fn get_ark_verifying_key() -> ark_groth16::VerifyingKey<ark_bn254::Bn254> {
    use ark_bn254::{Fq, Fq2, G1Affine, G2Affine};
    use std::str::FromStr;

    let alpha_g1 = G1Affine::new(
        Fq::from_str(
            "20491192805390485299153009773594534940189261866228447918068658471970481763042",
        )
        .unwrap(),
        Fq::from_str(
            "9383485363053290200918347156157836566562967994039712273449902621266178545958",
        )
        .unwrap(),
    );

    let beta_g2 = G2Affine::new(
        Fq2::new(
            Fq::from_str(
                "6375614351688725206403948262868962793625744043794305715222011528459656738731",
            )
            .unwrap(),
            Fq::from_str(
                "4252822878758300859123897981450591353533073413197771768651442665752259397132",
            )
            .unwrap(),
        ),
        Fq2::new(
            Fq::from_str(
                "10505242626370262277552901082094356697409835680220590971873171140371331206856",
            )
            .unwrap(),
            Fq::from_str(
                "21847035105528745403288232691147584728191162732299865338377159692350059136679",
            )
            .unwrap(),
        ),
    );

    let gamma_g2 = G2Affine::new(
        Fq2::new(
            Fq::from_str(
                "10857046999023057135944570762232829481370756359578518086990519993285655852781",
            )
            .unwrap(),
            Fq::from_str(
                "11559732032986387107991004021392285783925812861821192530917403151452391805634",
            )
            .unwrap(),
        ),
        Fq2::new(
            Fq::from_str(
                "8495653923123431417604973247489272438418190587263600148770280649306958101930",
            )
            .unwrap(),
            Fq::from_str(
                "4082367875863433681332203403145435568316851327593401208105741076214120093531",
            )
            .unwrap(),
        ),
    );

    let delta_g2 = G2Affine::new(
        Fq2::new(
            Fq::from_str(
                "19928663713463533589216209779412278386769407450988172849262535478593422929698",
            )
            .unwrap(),
            Fq::from_str(
                "19916519943909223643323234301580053157586699704876134064841182937085943926141",
            )
            .unwrap(),
        ),
        Fq2::new(
            Fq::from_str(
                "4584600978911428195337731119171761277167808711062125916470525050324985708782",
            )
            .unwrap(),
            Fq::from_str(
                "903010326261527050999816348900764705196723158942686053018929539519969664840",
            )
            .unwrap(),
        ),
    );

    let gamma_abc_g1 = vec![
        G1Affine::new(
            Fq::from_str(
                "6698887085900109660417671413804888867145870700073340970189635830129386206569",
            )
            .unwrap(),
            Fq::from_str(
                "10431087902009508261375793061696708147989126018612269070732549055898651692604",
            )
            .unwrap(),
        ),
        G1Affine::new(
            Fq::from_str(
                "20225609417084538563062516991929114218412992453664808591983416996515711931386",
            )
            .unwrap(),
            Fq::from_str(
                "3236310410959095762960658876334609343091075204896196791007975095263664214628",
            )
            .unwrap(),
        ),
    ];

    ark_groth16::VerifyingKey { alpha_g1, beta_g2, gamma_g2, delta_g2, gamma_abc_g1 }
}
pub fn get_r0_verifying_key() -> risc0_groth16::VerifyingKey {
    let json_content = std::fs::read_to_string("/home/etu/risc0-to-bitvm2/vkey_guest.json")
        .expect("Failed to read verification key JSON file");
    let vk_json: risc0_groth16::VerifyingKeyJson =
        serde_json::from_str(&json_content).expect("Failed to parse verification key JSON");

    vk_json.verifying_key().unwrap()
}
pub fn from_seal(seal_bytes: &[u8]) -> ark_groth16::Proof<ark_bn254::Bn254> {
    use ark_bn254::{Fq, Fq2, G1Affine, G2Affine};
    use ark_ff::{Field, PrimeField};

    let a = G1Affine::new(
        Fq::from_be_bytes_mod_order(&seal_bytes[0..32]),
        Fq::from_be_bytes_mod_order(&seal_bytes[32..64]),
    );

    let b = G2Affine::new(
        Fq2::from_base_prime_field_elems([
            Fq::from_be_bytes_mod_order(&seal_bytes[96..128]),
            Fq::from_be_bytes_mod_order(&seal_bytes[64..96]),
        ])
        .unwrap(),
        Fq2::from_base_prime_field_elems([
            Fq::from_be_bytes_mod_order(&seal_bytes[160..192]),
            Fq::from_be_bytes_mod_order(&seal_bytes[128..160]),
        ])
        .unwrap(),
    );

    let c = G1Affine::new(
        Fq::from_be_bytes_mod_order(&seal_bytes[192..224]),
        Fq::from_be_bytes_mod_order(&seal_bytes[224..256]),
    );

    ark_groth16::Proof { a, b, c }
}
