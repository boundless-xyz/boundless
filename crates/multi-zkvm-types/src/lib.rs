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

pub mod rpc;

use async_trait::async_trait;
use risc0_zkvm::Receipt;

use boundless_market::prover_utils::prover::{ProofResult, Prover, ProverError};

type Result<T> = std::result::Result<T, ProverError>;

#[derive(Default)]
pub struct MultiZkvmClient;

impl MultiZkvmClient {
    pub fn new(_endpoint: &str) -> Result<Self> {
        Ok(Self)
    }
}

#[async_trait]
impl Prover for MultiZkvmClient {
    async fn has_image(&self, _image_id: &str) -> Result<bool> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn upload_input(&self, _input: Vec<u8>) -> Result<String> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn upload_image(&self, _image_id: &str, _image: Vec<u8>) -> Result<()> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn preflight(
        &self,
        _image_id: &str,
        _input_id: &str,
        _assumptions: Vec<String>,
        _executor_limit: Option<u64>,
        _order_id: &str,
    ) -> Result<ProofResult> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn prove_stark(
        &self,
        _image_id: &str,
        _input_id: &str,
        _assumptions: Vec<String>,
    ) -> Result<String> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn wait_for_stark(&self, _proof_id: &str) -> Result<ProofResult> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn cancel_stark(&self, _proof_id: &str) -> Result<()> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn get_receipt(&self, _proof_id: &str) -> Result<Option<Receipt>> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn get_preflight_journal(&self, _proof_id: &str) -> Result<Option<Vec<u8>>> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn get_journal(&self, _proof_id: &str) -> Result<Option<Vec<u8>>> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn compress(&self, _proof_id: &str) -> Result<String> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn get_compressed_receipt(&self, _proof_id: &str) -> Result<Option<Vec<u8>>> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn compress_blake3_groth16(&self, _proof_id: &str) -> Result<String> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }

    async fn get_blake3_groth16_receipt(&self, _proof_id: &str) -> Result<Option<Vec<u8>>> {
        Err(ProverError::ProverInternalError("not implemented".into()))
    }
}
