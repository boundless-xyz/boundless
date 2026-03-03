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

use boundless_market::input::GuestEnv;

mod bonsai;
mod default;
mod risc0_blake3;

pub use bonsai::Bonsai;
pub use default::DefaultProver;
pub use risc0_blake3::{Blake3Bonsai, Blake3DefaultProver};

pub use boundless_market::prover_utils::prover::{
    ExecutorResp, ProofResult, Prover, ProverEntry, ProverError, ProverObj, ProverPriority,
    ProverReceipt, ProverRegistry,
};

pub fn encode_input(input: &impl serde::Serialize) -> Result<Vec<u8>, anyhow::Error> {
    Ok(GuestEnv::builder().write(input)?.stdin)
}

/// Fetch a compressed receipt from a [`Prover`], deserialize as a standard
/// `risc0_zkvm::Receipt`, and encode it as a seal.
pub(crate) async fn risc0_encode_compressed_seal(
    prover: &impl Prover,
    proof_id: &str,
) -> Result<Vec<u8>, ProverError> {
    let receipt_bytes = prover
        .get_compressed_receipt(proof_id)
        .await?
        .ok_or_else(|| ProverError::NotFound(format!("Compressed receipt not found: {proof_id}")))?;

    let receipt: risc0_zkvm::Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    risc0_ethereum_contracts::encode_seal(&receipt).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to encode seal: {e}"))
    })
}

/// Fetch a compressed receipt from a [`Prover`], deserialize as a standard
/// `risc0_zkvm::Receipt`, and verify its integrity.
pub(crate) async fn risc0_verify_compressed_receipt(
    prover: &impl Prover,
    proof_id: &str,
) -> Result<(), ProverError> {
    let receipt_bytes = prover
        .get_compressed_receipt(proof_id)
        .await?
        .ok_or_else(|| ProverError::NotFound(format!("Compressed receipt not found: {proof_id}")))?;

    let receipt: risc0_zkvm::Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    receipt
        .verify_integrity_with_context(&Default::default())
        .map_err(|e| ProverError::ProverInternalError(format!("Groth16 verification failed: {e}")))
}

#[cfg(test)]
pub fn new_test_registry() -> ProverRegistry {
    let default = std::sync::Arc::new(DefaultProver::new());
    ProverRegistry {
        risc0_default: ProverEntry::standard_only(default.clone()),
        risc0_blake3: ProverEntry::standard_only(std::sync::Arc::new(Blake3DefaultProver::new(
            default,
        ))),
    }
}
