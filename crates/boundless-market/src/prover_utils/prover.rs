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

//! Prover trait and types for zkVM proving operations.

use std::sync::Arc;

use async_trait::async_trait;
use risc0_zkvm::sha::{Digest, Digestible};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Executor output statistics.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExecutorResp {
    /// Total segments output.
    pub segments: u64,
    /// risc0-zkvm user cycles.
    pub user_cycles: u64,
    /// risc0-zkvm total cycles.
    pub total_cycles: u64,
    /// Count of assumptions included.
    pub assumption_count: u64,
}

/// Errors that can occur during proving operations.
#[derive(Error, Debug)]
pub enum ProverError {
    /// Resource not found.
    #[error("Not found: {0}")]
    NotFound(String),

    /// Stark job missing stats data.
    #[error("Stark job missing stats data")]
    MissingStatus,

    /// Prover failure.
    #[error("Prover failure: {0}")]
    ProvingFailed(String),

    /// Proof status expired retry count.
    #[error("proof status expired retry count")]
    StatusFailure,

    /// Prover internal error.
    #[error("Prover internal error: {0}")]
    ProverInternalError(String),

    /// Unexpected error.
    #[error("{0:#}")]
    UnexpectedError(#[from] anyhow::Error),
}

#[cfg(feature = "prover_utils")]
impl From<bincode::Error> for ProverError {
    fn from(err: bincode::Error) -> Self {
        ProverError::ProverInternalError(format!("Bincode error: {err}"))
    }
}

/// Result of a proof operation.
#[derive(Clone, Debug, Default)]
pub struct ProofResult {
    /// Unique identifier for this proof.
    pub id: String,
    /// Execution statistics.
    pub stats: ExecutorResp,
    /// Time elapsed during proving.
    #[allow(unused)]
    pub elapsed_time: f64,
}

/// Backend-agnostic receipt returned by [`Prover::get_receipt`].
///
/// Each variant wraps a backend-specific receipt type. Callers can use the
/// convenience methods for common operations, or pattern-match for
/// backend-specific access.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub enum ProverReceipt {
    /// A receipt from a RISC Zero zkVM prover.
    Risc0(risc0_zkvm::Receipt),
}

impl ProverReceipt {
    /// Get the journal bytes from the receipt.
    pub fn journal(&self) -> &[u8] {
        match self {
            Self::Risc0(receipt) => &receipt.journal.bytes,
        }
    }

    /// Get the claim digest from the receipt.
    pub fn claim_digest(&self) -> Result<Digest, ProverError> {
        match self {
            Self::Risc0(receipt) => {
                let claim = receipt.claim().map_err(|e| {
                    ProverError::ProverInternalError(format!("Failed to get claim: {e}"))
                })?;
                Ok(claim
                    .value()
                    .map_err(|e| {
                        ProverError::ProverInternalError(format!("Failed to get claim value: {e}"))
                    })?
                    .digest())
            }
        }
    }

    /// Extract the inner RISC Zero receipt, or return an error for other variants.
    pub fn into_risc0(self) -> Result<risc0_zkvm::Receipt, ProverError> {
        match self {
            Self::Risc0(receipt) => Ok(receipt),
        }
    }
}

/// Trait for prover implementations.
///
/// This trait defines the interface for zkVM proving operations including
/// image/input management, preflight execution, STARK proving, and compression.
#[async_trait]
pub trait Prover {
    /// Check if an image exists in the prover's storage.
    async fn has_image(&self, image_id: &str) -> Result<bool, ProverError>;

    /// Upload input data and return its identifier.
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError>;

    /// Upload an image (ELF binary) with the given image ID.
    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError>;

    /// Execute a preflight (execution-only, no proving) for cycle counting.
    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
        order_id: &str,
    ) -> Result<ProofResult, ProverError>;

    /// Start a STARK proving job.
    #[allow(unused)]
    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError>;

    /// Start and wait for a STARK proving job to complete.
    #[allow(unused)]
    async fn prove_and_monitor_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<ProofResult, ProverError> {
        let proof_id = self.prove_stark(image_id, input_id, assumptions).await?;
        self.wait_for_stark(&proof_id).await
    }

    /// Wait for a STARK proof to complete.
    #[allow(unused)]
    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError>;

    /// Cancel a STARK proving job.
    #[allow(unused)]
    async fn cancel_stark(&self, proof_id: &str) -> Result<(), ProverError>;

    /// Get the receipt for a completed proof.
    #[allow(unused)]
    async fn get_receipt(&self, proof_id: &str) -> Result<Option<ProverReceipt>, ProverError>;

    /// Get the journal from a preflight execution.
    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;

    /// Get the journal from a completed proof.
    #[allow(unused)]
    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;

    /// Compress a STARK proof to Groth16.
    #[allow(unused)]
    async fn compress(&self, proof_id: &str) -> Result<String, ProverError>;

    /// Get the compressed (Groth16) receipt.
    #[allow(unused)]
    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;

    /// Compress a STARK proof to Blake3 Groth16.
    #[allow(unused)]
    async fn compress_blake3_groth16(&self, proof_id: &str) -> Result<String, ProverError>;

    /// Get the Blake3 Groth16 receipt.
    #[allow(unused)]
    async fn get_blake3_groth16_receipt(
        &self,
        proof_id: &str,
    ) -> Result<Option<Vec<u8>>, ProverError>;

    /// Compute the image ID for the given ELF binary.
    async fn compute_image_id(&self, elf: &[u8]) -> Result<Digest, ProverError>;

    /// Compute the claim digest for the given image ID and journal.
    async fn compute_claim_digest(
        &self,
        image_id: Digest,
        journal: &[u8],
    ) -> Result<Digest, ProverError>;
}

/// Type alias for a boxed Prover trait object.
pub type ProverObj = Arc<dyn Prover + Send + Sync>;
