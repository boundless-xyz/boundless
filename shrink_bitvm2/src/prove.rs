use anyhow::{Context, Result};
use num_bigint::BigUint;
use num_traits::Num;
use risc0_groth16::prove::to_json as seal_to_json;
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt};
#[cfg(feature = "cuda")]
pub(crate) mod cuda;
#[cfg(not(feature = "cuda"))]
pub(crate) mod rapidsnark;
pub(crate) mod witgen;

pub(crate) fn identity_seal_json(
    journal_bytes: [u8; 32],
    p254_receipt: &SuccinctReceipt<ReceiptClaim>,
) -> Result<serde_json::Value> {
    let seal_bytes = p254_receipt.get_seal_bytes();
    let seal_json = seal_to_json(seal_bytes.as_slice())?; // TODO(ec2): This is currently using a local version of risc0 which exposes this method
    let mut seal_json: serde_json::Value = serde_json::from_str(&seal_json)?;

    let mut journal_bits = Vec::new();
    for byte in journal_bytes {
        for i in 0..8 {
            journal_bits.push((byte >> (7 - i)) & 1);
        }
    }
    let receipt_claim = p254_receipt.claim.as_value().unwrap();
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
    // remove 248th and 249th bits
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

    Ok(seal_json)
}

fn to_decimal(s: &str) -> Option<String> {
    let int = BigUint::from_str_radix(s, 16).ok();
    int.map(|n| n.to_str_radix(10))
}
