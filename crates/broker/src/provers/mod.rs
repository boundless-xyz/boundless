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

use blake3_groth16::Blake3Groth16Receipt;
use boundless_market::input::GuestEnv;
use risc0_zkvm::Receipt;

mod bonsai;
mod default;

pub use bonsai::Bonsai;
pub use default::DefaultProver;

// Re-export types from boundless_market::prover_utils::prover
pub use boundless_market::prover_utils::prover::{
    ExecutorResp, ProofResult, Prover, ProverError, ProverObj, ProverReceipt,
};

/// Encode inputs for Prover::upload_slice()
pub fn encode_input(input: &impl serde::Serialize) -> Result<Vec<u8>, anyhow::Error> {
    Ok(GuestEnv::builder().write(input)?.stdin)
}

/// Verify a Groth16 compressed receipt
///
/// This helper fetches the compressed receipt, deserializes it, and verifies its integrity.
/// Used by both aggregator and proving services to validate Groth16 proofs before submission.
pub(crate) async fn verify_groth16_receipt(
    prover: &ProverObj,
    proof_id: &str,
) -> Result<(), ProverError> {
    tracing::trace!("Verifying Groth16 receipt locally for proof_id: {proof_id}");

    let receipt_bytes = prover
        .get_compressed_receipt(proof_id)
        .await?
        .ok_or_else(|| ProverError::NotFound(format!("Groth16 receipt not found: {proof_id}")))?;

    let receipt: Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    receipt.verify_integrity_with_context(&Default::default()).map_err(|e| {
        ProverError::ProverInternalError(format!("Groth16 verification failed: {e}"))
    })?;

    tracing::debug!("Groth16 verification passed for proof_id: {proof_id}");
    Ok(())
}

/// Verify a blake3 Groth16 compressed receipt
///
/// This helper fetches the compressed receipt, deserializes it, and verifies its integrity.
/// Used by both aggregator and proving services to validate Groth16 proofs before submission.
pub(crate) async fn verify_blake3_groth16_receipt(
    prover: &ProverObj,
    proof_id: &str,
) -> Result<(), ProverError> {
    tracing::trace!("Verifying Blake3 Groth16 receipt locally for proof_id: {proof_id}");

    let receipt_bytes = prover.get_blake3_groth16_receipt(proof_id).await?.ok_or_else(|| {
        ProverError::NotFound(format!("Blake3 Groth16 receipt not found: {proof_id}"))
    })?;

    let receipt: Blake3Groth16Receipt = bincode::deserialize(&receipt_bytes).map_err(|e| {
        ProverError::ProverInternalError(format!("Failed to deserialize receipt: {e}"))
    })?;

    receipt.verify_integrity().map_err(|e| {
        ProverError::ProverInternalError(format!("Blake3 Groth16 verification failed: {e}"))
    })?;

    tracing::debug!("Blake3 Groth16 verification passed for proof_id: {proof_id}");
    Ok(())
}
