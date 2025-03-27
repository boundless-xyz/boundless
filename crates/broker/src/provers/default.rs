// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{borrow::Borrow, collections::HashMap, sync::Arc};

use crate::config::ProverConf;
use crate::provers::{ExecutorResp, ProofResult, Prover, ProverError};
use anyhow::{Context, Result as AnyhowResult};
use async_trait::async_trait;
use risc0_zkvm::{
    default_executor, default_prover, ExecutorEnv, ProveInfo, ProverOpts, Receipt, SessionInfo,
    VERSION,
};
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Default)]
pub struct DefaultProver {
    state: Arc<ProverState>,
}

#[derive(Debug, Default)]
struct ProverState {
    inputs: RwLock<HashMap<String, Vec<u8>>>,
    images: RwLock<HashMap<String, Vec<u8>>>,
    proofs: RwLock<HashMap<String, ProofData>>,
}

#[derive(Debug, Default)]
struct ProofData {
    status: Status,
    error_msg: String,
    stats: Option<ExecutorResp>,
    preflight_journal: Option<Vec<u8>>,
    receipt: Option<Receipt>,
    compressed_receipt: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
enum Status {
    #[default]
    Running,
    Succeeded,
    Failed,
}

impl DefaultProver {
    /// Creates a new [DefaultProver].
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }

    async fn execute(
        elf: Vec<u8>,
        input: Vec<u8>,
        assumptions: Vec<Receipt>,
        executor_limit: Option<u64>,
    ) -> AnyhowResult<SessionInfo> {
        tokio::task::spawn_blocking(move || {
            let mut env_builder = ExecutorEnv::builder();
            env_builder.session_limit(executor_limit);
            env_builder.write_slice(&input);
            assumptions.into_iter().for_each(|receipt| {
                env_builder.add_assumption(receipt);
            });
            let env = env_builder.build()?;

            default_executor().execute(env, &elf)
        })
        .await
        .unwrap()
    }

    async fn prove(
        elf: Vec<u8>,
        input: Vec<u8>,
        assumptions: Vec<Receipt>,
        opts: ProverOpts,
    ) -> AnyhowResult<ProveInfo> {
        tokio::task::spawn_blocking(move || {
            let mut env_builder = ExecutorEnv::builder();
            env_builder.write_slice(&input);
            assumptions.into_iter().for_each(|receipt| {
                env_builder.add_assumption(receipt);
            });
            let env = env_builder.build()?;

            default_prover().prove_with_opts(env, &elf, &opts)
        })
        .await
        .unwrap()
    }

    async fn get_input(&self, id: &str) -> Option<Vec<u8>> {
        self.state.inputs.read().await.get(id).cloned()
    }

    async fn get_image(&self, id: &str) -> Option<Vec<u8>> {
        self.state.images.read().await.get(id).cloned()
    }

    async fn get_receipts<S: Borrow<String>>(
        &self,
        proof_id: impl IntoIterator<Item = S>,
    ) -> AnyhowResult<Vec<Receipt>> {
        let mut receipts = Vec::new();

        let proofs = self.state.proofs.read().await;
        for proof_id in proof_id {
            let receipt = proofs
                .get(proof_id.borrow())
                .and_then(|proof| proof.receipt.clone())
                .with_context(|| format!("no receipt for proof {}", proof_id.borrow()))?;

            receipts.push(receipt);
        }
        Ok(receipts)
    }
}

#[async_trait]
impl Prover for DefaultProver {
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        let input_id = format!("input_{}", Uuid::new_v4());

        let mut inputs = self.state.inputs.write().await;
        inputs.insert(input_id.clone(), input);

        Ok(input_id)
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        let mut images = self.state.images.write().await;
        images.insert(image_id.to_string(), image);

        Ok(())
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError> {
        let image = self
            .get_image(image_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("image {image_id}")))?;
        let input = self
            .get_input(input_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("input {input_id}")))?;
        let assumption_receipts = self
            .get_receipts(assumptions)
            .await
            .map_err(|err| ProverError::NotFound(err.to_string()))?;

        let proof_id = format!("execute_{}", Uuid::new_v4());
        self.state.proofs.write().await.insert(proof_id.clone(), ProofData::default());

        let execute_result =
            DefaultProver::execute(image, input, assumption_receipts, executor_limit).await;

        let mut proofs = self.state.proofs.write().await;
        let proof = proofs.get_mut(&proof_id).unwrap();
        match execute_result {
            Ok(info) => {
                let stats = ExecutorResp {
                    segments: info.segments.len() as u64,
                    user_cycles: info.cycles(),
                    total_cycles: info.cycles(),
                    ..Default::default()
                };

                proof.status = Status::Succeeded;
                proof.stats = Some(stats.clone());
                proof.preflight_journal = Some(info.journal.bytes);

                Ok(ProofResult { id: proof_id, stats, ..Default::default() })
            }
            Err(err) => {
                proof.status = Status::Failed;
                proof.error_msg = err.to_string();

                Err(ProverError::ProvingFailed(err.to_string()))
            }
        }
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        let image = self
            .get_image(image_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("image {image_id}")))?;
        let input = self
            .get_input(input_id)
            .await
            .ok_or_else(|| ProverError::NotFound(format!("input {input_id}")))?;
        let assumption_receipts = self
            .get_receipts(assumptions)
            .await
            .map_err(|err| ProverError::NotFound(err.to_string()))?;

        let proof_id = format!("stark_{}", Uuid::new_v4());
        self.state.proofs.write().await.insert(proof_id.clone(), ProofData::default());

        tokio::spawn({
            let state = self.state.clone();
            let proof_id = proof_id.clone();
            async move {
                let proof_result =
                    DefaultProver::prove(image, input, assumption_receipts, ProverOpts::succinct())
                        .await;

                let mut proofs = state.proofs.write().await;
                let proof = proofs.get_mut(&proof_id).unwrap();
                match proof_result {
                    Ok(info) => {
                        *proof = ProofData {
                            status: Status::Succeeded,
                            stats: Some(ExecutorResp {
                                segments: info.stats.segments as u64,
                                user_cycles: info.stats.user_cycles,
                                total_cycles: info.stats.total_cycles,
                                ..Default::default()
                            }),
                            receipt: Some(info.receipt),
                            ..Default::default()
                        };
                    }
                    Err(err) => {
                        *proof = ProofData {
                            status: Status::Failed,
                            error_msg: err.to_string(),
                            ..Default::default()
                        }
                    }
                }
            }
        });

        Ok(proof_id)
    }

    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError> {
        if !proof_id.starts_with("stark_") {
            return Err(ProverError::NotFound(format!("stark proof {proof_id}")))?;
        }

        const MAX_ATTEMPTS: u32 = 1800; // 30 minutes at 1 second intervals
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

        for _ in 0..MAX_ATTEMPTS {
            {
                let proofs = self.state.proofs.read().await;
                let proof_data = proofs
                    .get(proof_id)
                    .ok_or_else(|| ProverError::NotFound(format!("stark proof {proof_id}")))?;

                match proof_data.status {
                    Status::Running => {}
                    Status::Succeeded => {
                        let stats = proof_data.stats.as_ref().unwrap();
                        return Ok(ProofResult {
                            id: proof_id.to_string(),
                            stats: ExecutorResp {
                                segments: stats.segments,
                                user_cycles: stats.user_cycles,
                                total_cycles: stats.total_cycles,
                                ..Default::default()
                            },
                            ..Default::default()
                        });
                    }
                    Status::Failed => {
                        return Err(ProverError::ProvingFailed(proof_data.error_msg.clone()));
                    }
                }
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }

        Err(ProverError::ProvingFailed(format!("timeout after {:?}", POLL_INTERVAL * MAX_ATTEMPTS)))
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
        let proofs = self.state.proofs.read().await;
        let proof_data = proofs
            .get(proof_id)
            .ok_or_else(|| ProverError::NotFound(format!("proof {proof_id}")))?;
        Ok(proof_data.receipt.clone())
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let proofs = self.state.proofs.read().await;
        let proof_data = proofs
            .get(proof_id)
            .ok_or_else(|| ProverError::NotFound(format!("proof {proof_id}")))?;
        Ok(proof_data.preflight_journal.clone())
    }

    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let proofs = self.state.proofs.read().await;
        let proof_data = proofs
            .get(proof_id)
            .ok_or_else(|| ProverError::NotFound(format!("proof {proof_id}")))?;
        Ok(proof_data.receipt.as_ref().map(|receipt| receipt.journal.bytes.clone()))
    }

    async fn compress(&self, proof_id: &str) -> Result<String, ProverError> {
        let receipt = self
            .get_receipt(proof_id)
            .await?
            .ok_or_else(|| ProverError::NotFound(format!("no receipt for proof {}", proof_id)))?;

        let proof_id = format!("snark_{}", Uuid::new_v4());
        self.state.proofs.write().await.insert(proof_id.clone(), ProofData::default());

        // TODO: remove this workaround when default_prover().compress works for Bonsai
        let compress_result = if default_prover().get_name() == "bonsai" {
            let client = bonsai_sdk::non_blocking::Client::from_env(VERSION)?;
            super::Bonsai::compress(&client, &receipt, &ProverConf::default()).await
        } else {
            tokio::task::spawn_blocking(move || {
                default_prover().compress(&ProverOpts::groth16(), &receipt)
            })
            .await
            .unwrap()
            .map_err(ProverError::from)
        };
        let compressed_bytes = compress_result
            .as_ref()
            .map(|receipt| bincode::serialize(receipt).unwrap())
            .unwrap_or_default();

        let mut proofs = self.state.proofs.write().await;
        let proof = proofs.get_mut(&proof_id).unwrap();
        match compress_result {
            Ok(_) => {
                proof.status = Status::Succeeded;
                proof.compressed_receipt = Some(compressed_bytes);

                Ok(proof_id)
            }
            Err(err) => {
                proof.status = Status::Failed;
                proof.error_msg = err.to_string();

                Err(err)
            }
        }
    }

    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let proofs = self.state.proofs.read().await;
        let proof_data = proofs
            .get(proof_id)
            .ok_or_else(|| ProverError::NotFound(format!("proof {proof_id}")))?;
        Ok(proof_data.compressed_receipt.as_ref().cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::sha::Digest;
    use tokio::test;

    #[test]
    async fn test_upload_input_and_image() {
        let prover = DefaultProver::new();

        // Test input upload
        let input_data = b"Hello, World!".to_vec();
        let input_id = prover.upload_input(input_data.clone()).await.unwrap();

        // Test image upload
        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();

        // Verify input was stored
        let stored_input = prover.get_input(&input_id).await.unwrap();
        assert_eq!(stored_input, input_data);

        // Verify image was stored
        let stored_image = prover.get_image(&image_id).await.unwrap();
        assert_eq!(stored_image.as_slice(), ECHO_ELF);
    }

    #[test]
    async fn test_preflight() {
        let prover = DefaultProver::new();

        // Upload test data
        let input_data = b"Hello, World!".to_vec();
        let input_id = prover.upload_input(input_data.clone()).await.unwrap();
        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();

        // Run preflight
        let result = prover.preflight(&image_id, &input_id, vec![], None).await.unwrap();
        assert!(!result.id.is_empty());
        assert!(result.stats.segments > 0 && result.stats.user_cycles > 0);

        // Fetch the journal
        let journal = prover.get_preflight_journal(&result.id).await.unwrap().unwrap();
        assert_eq!(journal, input_data);
    }

    #[test]
    async fn test_prove_stark() {
        let prover = DefaultProver::new();

        // Upload test data
        let input_data = b"Hello, World!".to_vec();
        let input_id = prover.upload_input(input_data.clone()).await.unwrap();
        let image_id = Digest::from(ECHO_ID);
        prover.upload_image(&image_id.to_string(), ECHO_ELF.to_vec()).await.unwrap();

        // Run STARK proving
        let result =
            prover.prove_and_monitor_stark(&image_id.to_string(), &input_id, vec![]).await.unwrap();
        assert!(!result.id.is_empty());
        assert!(
            result.stats.segments > 0
                && result.stats.total_cycles > 0
                && result.stats.user_cycles > 0
        );

        // Fetch the journal
        let journal = prover.get_journal(&result.id).await.unwrap().unwrap();
        assert_eq!(journal, input_data);

        // Fetch the receipt
        let receipt = prover.get_receipt(&result.id).await.unwrap().unwrap();
        receipt.verify(image_id).unwrap();
    }

    #[test]
    async fn test_compress() {
        let prover = DefaultProver::new();

        // Upload test data
        let input_data = b"Hello, World!".to_vec();
        let input_id = prover.upload_input(input_data.clone()).await.unwrap();
        let image_id = Digest::from(ECHO_ID);
        prover.upload_image(&image_id.to_string(), ECHO_ELF.to_vec()).await.unwrap();

        // Run SNARK proving
        let ProofResult { id: stark_id, .. } =
            prover.prove_and_monitor_stark(&image_id.to_string(), &input_id, vec![]).await.unwrap();
        let snark_id = prover.compress(&stark_id).await.unwrap();

        // Fetch the compressed receipt
        let compressed_receipt = prover.get_compressed_receipt(&snark_id).await.unwrap().unwrap();
        let receipt: Receipt = bincode::deserialize(&compressed_receipt).unwrap();
        receipt.verify(image_id).unwrap();
    }

    #[test]
    async fn test_error_handling() {
        let prover = DefaultProver::new();

        // Test handling of non-existent resources
        let nonexistent_id = "nonexistent";

        // Should return appropriate errors
        let image_err = prover.get_image(nonexistent_id).await;
        assert!(image_err.is_none());

        let input_err = prover.get_input(nonexistent_id).await;
        assert!(input_err.is_none());

        let receipt_err = prover.get_receipt(nonexistent_id).await;
        assert!(matches!(receipt_err, Err(ProverError::NotFound(_))));

        let journal_err = prover.get_journal(nonexistent_id).await;
        assert!(matches!(journal_err, Err(ProverError::NotFound(_))));
    }
}
