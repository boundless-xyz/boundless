// Copyright 2026 Boundless Foundation, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

extern crate alloc;

use alloc::vec;
use anyhow::{ensure, Result};
use ark_bn254::{Fq, Fq2, G1Affine, G2Affine};
use ark_ff::PrimeField;
use ark_serialize::CanonicalSerialize;
use risc0_zkvm::Digest;
use std::str::FromStr;

// Constants from: risc0-ethereum/contracts/src/blake3/Groth16Verifier.sol
// When running a new ceremony, update them by running cargo xtask bootstrap-blake3-groth16
// after updating the new Groth16Verifier.sol on the risc0-ethereum repo.
const ALPHA_X: &str =
    "16428432848801857252194528405604668803277877773566238944394625302971855135431";
const ALPHA_Y: &str =
    "16846502678714586896801519656441059708016666274385668027902869494772365009666";
const BETA_X1: &str =
    "3182164110458002340215786955198810119980427837186618912744689678939861918171";
const BETA_X2: &str =
    "16348171800823588416173124589066524623406261996681292662100840445103873053252";
const BETA_Y1: &str =
    "4920802715848186258981584729175884379674325733638798907835771393452862684714";
const BETA_Y2: &str =
    "19687132236965066906216944365591810874384658708175106803089633851114028275753";
const GAMMA_X1: &str =
    "11559732032986387107991004021392285783925812861821192530917403151452391805634";
const GAMMA_X2: &str =
    "10857046999023057135944570762232829481370756359578518086990519993285655852781";
const GAMMA_Y1: &str =
    "4082367875863433681332203403145435568316851327593401208105741076214120093531";
const GAMMA_Y2: &str =
    "8495653923123431417604973247489272438418190587263600148770280649306958101930";
const DELTA_X1: &str =
    "18786665442134809547367793008388252094276956707083189371748822844215202271178";
const DELTA_X2: &str =
    "17296777349791701671871010047490559682924748762983962242018229225890177681165";
const DELTA_Y1: &str =
    "21546884238630900902634517213362010321565339505810557359182294051078510536811";
const DELTA_Y2: &str =
    "7214627676570978956115414107903354102221009447018809863680303520130992055423";

const IC0_X: &str = "1396989810128049774239906514097458055670219613079348950494410066757721605523";
const IC0_Y: &str = "20069629286434534534516684991063672335613842540347999544849171590987775766961";
const IC1_X: &str = "19282603452922066135228857769519044667044696173320493211119861249451600114594";
const IC1_Y: &str = "11966256187809052800087108088094647243345273965264062329687482664981607072161";

/// Verifies the soundness of a Groth16Seal against the BLAKE3 claim digest.
pub fn verify_seal(seal_bytes: &[u8], blake3_claim_digest: impl Into<Digest>) -> Result<()> {
    let ark_proof = from_seal(seal_bytes);
    let public_input_scalar =
        ark_bn254::Fr::from_be_bytes_mod_order(blake3_claim_digest.into().as_ref());
    let ark_vk = get_ark_verifying_key();
    let ark_pvk = ark_groth16::prepare_verifying_key(&ark_vk);
    let res = ark_groth16::Groth16::<ark_bn254::Bn254>::verify_proof(
        &ark_pvk,
        &ark_proof,
        &[public_input_scalar],
    )
    .unwrap();
    ensure!(res, "proof verification failed");
    Ok(())
}

fn get_r0_verifying_key() -> risc0_groth16::VerifyingKey {
    let ark_key = get_ark_verifying_key();
    let mut b = vec![];
    ark_key.serialize_uncompressed(&mut b).unwrap();
    let j = serde_json::to_string(&b).expect("Failed to serialize verification key to JSON");
    let vk: risc0_groth16::VerifyingKey =
        serde_json::from_str(j.as_str()).expect("failed to parse JSON");
    vk
}

pub fn verifier_parameters() -> risc0_zkvm::Groth16ReceiptVerifierParameters {
    let vk: risc0_groth16::VerifyingKey = get_r0_verifying_key();
    risc0_zkvm::Groth16ReceiptVerifierParameters { verifying_key: vk, ..Default::default() }
}

fn from_seal(seal_bytes: &[u8]) -> ark_groth16::Proof<ark_bn254::Bn254> {
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

fn get_ark_verifying_key() -> ark_groth16::VerifyingKey<ark_bn254::Bn254> {
    let alpha_g1 = G1Affine::new(Fq::from_str(ALPHA_X).unwrap(), Fq::from_str(ALPHA_Y).unwrap());

    let beta_g2 = G2Affine::new(
        Fq2::new(Fq::from_str(BETA_X2).unwrap(), Fq::from_str(BETA_X1).unwrap()),
        Fq2::new(Fq::from_str(BETA_Y2).unwrap(), Fq::from_str(BETA_Y1).unwrap()),
    );

    let gamma_g2 = G2Affine::new(
        Fq2::new(Fq::from_str(GAMMA_X2).unwrap(), Fq::from_str(GAMMA_X1).unwrap()),
        Fq2::new(Fq::from_str(GAMMA_Y2).unwrap(), Fq::from_str(GAMMA_Y1).unwrap()),
    );

    let delta_g2 = G2Affine::new(
        Fq2::new(Fq::from_str(DELTA_X2).unwrap(), Fq::from_str(DELTA_X1).unwrap()),
        Fq2::new(Fq::from_str(DELTA_Y2).unwrap(), Fq::from_str(DELTA_Y1).unwrap()),
    );

    let gamma_abc_g1 = vec![
        G1Affine::new(Fq::from_str(IC0_X).unwrap(), Fq::from_str(IC0_Y).unwrap()),
        G1Affine::new(Fq::from_str(IC1_X).unwrap(), Fq::from_str(IC1_Y).unwrap()),
    ];

    ark_groth16::VerifyingKey { alpha_g1, beta_g2, gamma_g2, delta_g2, gamma_abc_g1 }
}

#[cfg(test)]
mod test {
    use risc0_zkvm::sha::Digestible;

    use super::*;

    #[test]
    fn print_verifier_params_digest() {
        let digest = verifier_parameters().digest();
        println!("Blake3 Groth16 verifier parameters digest: {}", digest);
    }
}
