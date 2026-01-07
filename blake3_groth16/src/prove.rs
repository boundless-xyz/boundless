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

use std::path::Path;

use anyhow::{Context, Result};
use num_bigint::BigUint;
use num_traits::Num;
use risc0_groth16::{prove::to_json as seal_to_json, ProofJson as Groth16ProofJson};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{ProverOpts, ReceiptClaim, SuccinctReceipt};
use tempfile::tempdir;

#[cfg(feature = "cuda")]
pub(crate) mod cuda;
#[cfg(not(feature = "cuda"))]
pub(crate) mod docker;

/// Creates a BLAKE3 Groth16 proof from a Risc0 SuccinctReceipt.
/// It will first run the identity_p254 program to convert the STARK to BN254,
/// which is more efficient to verify.
pub(crate) fn succinct_to_blake3_groth16(
    succinct_receipt: &SuccinctReceipt<ReceiptClaim>,
    journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    let p254_receipt = risc0_zkvm::get_prover_server(&ProverOpts::default())?
        .identity_p254(succinct_receipt)
        .context("failed to create p254 receipt")?;
    shrink_wrap(&p254_receipt, journal)
}

/// Creates a BLAKE3 Groth16 proof from a identity p254 Risc0 SuccinctReceipt.
pub(crate) fn shrink_wrap(
    p254_receipt: &SuccinctReceipt<ReceiptClaim>,
    journal: [u8; 32],
) -> Result<Groth16ProofJson> {
    let seal_json = identity_seal_json(journal, p254_receipt)?;

    let tmp_dir = tempdir().context("failed to create temporary directory")?;
    let work_dir = std::env::var("BLAKE3_GROTH16_WORK_DIR");
    let work_dir: &Path = work_dir.as_ref().map(Path::new).unwrap_or(tmp_dir.path());

    #[cfg(feature = "cuda")]
    let proof_json = cuda::shrink_wrap(work_dir, seal_json)?;
    #[cfg(not(feature = "cuda"))]
    let proof_json = docker::shrink_wrap(work_dir, seal_json)?;

    Ok(proof_json)
}

pub(crate) fn identity_seal_json(
    journal_bytes: [u8; 32],
    p254_receipt: &SuccinctReceipt<ReceiptClaim>,
) -> Result<serde_json::Value> {
    let seal_bytes = p254_receipt.get_seal_bytes();
    let seal_json = seal_to_json(seal_bytes.as_slice())?;
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
