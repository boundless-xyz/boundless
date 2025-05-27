// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use async_trait::async_trait;
use bonsai_sdk::SdkErr;
use boundless_market::input::GuestEnv;
use risc0_zkvm::Receipt;
use serde::{Deserialize, Serialize};
use thiserror::Error;

mod bonsai;
mod default;

pub use bonsai::Bonsai;
pub use default::DefaultProver;

/// Executor output
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ExecutorResp {
    /// Total segments output
    pub segments: u64,
    /// risc0-zkvm user cycles
    pub user_cycles: u64,
    /// risc0-zkvm total cycles
    pub total_cycles: u64,
    /// Count of assumptions included
    pub assumption_count: u64,
}

use crate::{
    config::ConfigErr,
    errors::{impl_coded_debug, CodedError},
};

#[derive(Error)]
pub enum ProverError {
    #[error("Bonsai proving error")]
    BonsaiErr(#[from] SdkErr),

    #[error("Config error")]
    ConfigReadErr(#[from] ConfigErr),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Stark job missing stats data")]
    MissingStatus,

    #[error("Prover failure: {0}")]
    ProvingFailed(String),

    #[error("Bincode deserilization error")]
    BincodeErr(#[from] bincode::Error),

    #[error("proof status expired retry count")]
    StatusFailure,

    #[error("Prover internal error: {0}")]
    ProverInternalError(String),

    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}

impl_coded_debug!(ProverError);

impl CodedError for ProverError {
    fn code(&self) -> &str {
        match self {
            ProverError::BonsaiErr(_) => "[B-BON-001]",
            ProverError::ConfigReadErr(_) => "[B-BON-002]",
            ProverError::NotFound(_) => "[B-BON-003]",
            ProverError::MissingStatus => "[B-BON-004]",
            ProverError::ProvingFailed(_) => "[B-BON-005]",
            ProverError::BincodeErr(_) => "[B-BON-006]",
            ProverError::StatusFailure => "[B-BON-007]",
            ProverError::ProverInternalError(_) => "[B-BON-008]",
            ProverError::UnexpectedError(_) => "[B-BON-500]",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ProofResult {
    pub id: String,
    pub stats: ExecutorResp,
    pub elapsed_time: f64,
}

/// Encode inputs for Prover::upload_slice()
pub fn encode_input(input: &impl serde::Serialize) -> Result<Vec<u8>, anyhow::Error> {
    Ok(GuestEnv::builder().write(input)?.stdin)
}

#[async_trait]
pub trait Prover {
    async fn has_image(&self, image_id: &str) -> Result<bool, ProverError>;
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError>;
    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError>;
    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError>;
    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError>;
    async fn prove_and_monitor_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<ProofResult, ProverError> {
        let proof_id = self.prove_stark(image_id, input_id, assumptions).await?;
        self.wait_for_stark(&proof_id).await
    }
    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError>;
    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError>;
    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
    async fn compress(&self, proof_id: &str) -> Result<String, ProverError>;
    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
}

pub type ProverObj = Arc<dyn Prover + Send + Sync>;
