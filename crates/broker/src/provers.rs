// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};

use async_trait::async_trait;
use bonsai_sdk::{
    non_blocking::{Client as BonsaiClient, SessionId, SnarkId},
    SdkErr,
};
use boundless_market::input::InputEnv;
use risc0_zkvm::{
    compute_image_id, sha::Digestible, FakeReceipt, InnerReceipt, MaybePruned, Receipt,
    ReceiptClaim,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Executor output
#[derive(Clone, Deserialize, Serialize)]
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

// For mock prover:
use risc0_zkvm::{default_executor, ExecutorEnv};

use crate::config::{ConfigErr, ConfigLock};

#[derive(Error, Debug)]
pub enum ProverError {
    #[error("Bonsai proving error")]
    BonsaiErr(#[from] SdkErr),

    #[error("Config error")]
    ConfigReadErr(#[from] ConfigErr),

    #[error("Stark job missing stats data")]
    MissingStatus,

    #[error("Prover failure: {0}")]
    ProvingFailed(String),

    #[error("Bincode deserilization error")]
    BincodeErr(#[from] bincode::Error),

    #[error("proof status expired retry count")]
    StatusFailure,
}

#[derive(Clone)]
pub struct ProofResult {
    pub id: String,
    pub stats: ExecutorResp,
    pub elapsed_time: f64,
}

/// Encode inputs for Prover::upload_slice()
pub fn encode_input(input: &impl serde::Serialize) -> Result<Vec<u8>, anyhow::Error> {
    Ok(InputEnv::new().write(input)?.input())
}

#[async_trait]
pub trait Prover {
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
    ) -> Result<ProofResult, ProverError>;
    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError>;
    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError>;
    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
    async fn compress(&self, proof_id: &str) -> Result<String, ProverError>;
    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError>;
}

pub type ProverObj = Arc<dyn Prover + Send + Sync>;

pub struct Bonsai {
    client: BonsaiClient,
    config: ConfigLock,
}

impl Bonsai {
    pub fn new(config: ConfigLock, api_url: &str, api_key: &str) -> Result<Self, ProverError> {
        let risc0_ver = {
            let config = config.lock_all()?;
            config.prover.bonsai_r0_zkvm_ver.as_ref().ok_or(ConfigErr::InvalidConfig)?.clone()
        };

        Ok(Self {
            client: BonsaiClient::from_parts(api_url.into(), api_key.into(), &risc0_ver)?,
            config,
        })
    }
}

#[async_trait]
impl Prover for Bonsai {
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        Ok(self.client.upload_input(input).await?)
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        Ok(self.client.upload_img(image_id, image).await.map(|_| ())?)
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError> {
        let preflight_id = self
            .client
            .create_session_with_limit(
                image_id.into(),
                input_id.into(),
                assumptions,
                true,
                executor_limit,
            )
            .await?;

        let (poll_sleep, retry_counts) = {
            let config = self.config.lock_all()?;
            (config.prover.status_poll_ms, config.prover.req_retry_count)
        };

        loop {
            let mut status = None;
            for retry_count in 0..retry_counts {
                match preflight_id.status(&self.client).await {
                    Ok(res) => {
                        status = Some(res);
                        break;
                    }
                    Err(err) => {
                        tracing::warn!("Failed to get status: {err:?}, retrying {retry_count} / {retry_counts}");
                        tokio::time::sleep(tokio::time::Duration::from_secs(poll_sleep)).await;
                        continue;
                    }
                }
            }

            let Some(status) = status else {
                return Err(ProverError::StatusFailure);
            };

            match status.status.as_ref() {
                "RUNNING" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(poll_sleep)).await;
                    continue;
                }
                "SUCCEEDED" => {
                    let Some(stats) = status.stats else {
                        return Err(ProverError::MissingStatus);
                    };
                    return Ok(ProofResult {
                        id: preflight_id.uuid,
                        stats: ExecutorResp {
                            assumption_count: 0,
                            segments: stats.segments as u64,
                            user_cycles: stats.cycles,
                            total_cycles: stats.total_cycles,
                        },
                        elapsed_time: status.elapsed_time.unwrap_or(f64::NAN),
                    });
                }
                _ => {
                    let err_msg = status.error_msg.unwrap_or_default();
                    return Err(ProverError::ProvingFailed(format!("preflight failed: {err_msg}")));
                }
            }
        }
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        Ok(self
            .client
            .create_session(image_id.into(), input_id.into(), assumptions, false)
            .await?
            .uuid)
    }

    async fn prove_and_monitor_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<ProofResult, ProverError> {
        let proof_id = self.prove_stark(image_id, input_id, assumptions).await?;
        self.wait_for_stark(&proof_id).await
    }

    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError> {
        let proof_id = SessionId::new(proof_id.into());

        let poll_sleep = {
            let config = self.config.lock_all()?;
            config.prover.status_poll_ms
        };

        loop {
            let status = proof_id.status(&self.client).await?;
            match status.status.as_ref() {
                "RUNNING" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(poll_sleep)).await;
                    continue;
                }
                "SUCCEEDED" => {
                    let Some(stats) = status.stats else {
                        return Err(ProverError::MissingStatus);
                    };
                    return Ok(ProofResult {
                        id: proof_id.uuid,
                        stats: ExecutorResp {
                            assumption_count: 0,
                            segments: stats.segments as u64,
                            user_cycles: stats.cycles,
                            total_cycles: stats.total_cycles,
                        },
                        elapsed_time: status.elapsed_time.unwrap_or(f64::NAN),
                    });
                }
                _ => {
                    let err_msg = status.error_msg.unwrap_or_default();
                    return Err(ProverError::ProvingFailed(format!(
                        "stark proving failed: {err_msg}"
                    )));
                }
            }
        }
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let receipt = self.client.receipt_download(&session_id).await?;
        Ok(Some(bincode::deserialize(&receipt)?))
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let journal = session_id.exec_only_journal(&self.client).await?;
        Ok(Some(journal))
    }

    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let receipt = self.get_receipt(proof_id).await?;
        let Some(receipt) = receipt else {
            return Ok(None);
        };

        Ok(Some(receipt.journal.bytes))
    }

    async fn compress(&self, proof_id: &str) -> Result<String, ProverError> {
        let proof_id = self.client.create_snark(proof_id.into()).await?;

        let poll_sleep = {
            let config = self.config.lock_all()?;
            config.prover.status_poll_ms
        };

        loop {
            let status = proof_id.status(&self.client).await?;
            match status.status.as_ref() {
                "RUNNING" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(poll_sleep)).await;
                    continue;
                }
                "SUCCEEDED" => return Ok(proof_id.uuid),
                _ => {
                    let err_msg = status.error_msg.unwrap_or_default();
                    return Err(ProverError::ProvingFailed(format!(
                        "snark proving failed: {err_msg}"
                    )));
                }
            }
        }
    }

    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let snark_id = SnarkId { uuid: proof_id.into() };
        let status = snark_id.status(&self.client).await?;

        let Some(output) = status.output else { return Ok(None) };
        let receipt_buf = self.client.download(&output).await?;

        Ok(Some(receipt_buf))
    }
}

#[derive(Default)]
pub struct MockProver {
    images: Mutex<HashMap<String, Vec<u8>>>,
    inputs: Mutex<HashMap<String, Vec<u8>>>,
    starks: Mutex<HashMap<String, (ProofResult, Receipt)>>,
    snarks: Mutex<HashMap<String, Receipt>>,
}

impl MockProver {
    fn mock_prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError> {
        let image = self
            .images
            .lock()
            .unwrap()
            .get(image_id)
            .ok_or(ProverError::BonsaiErr(SdkErr::InternalServerErr("image not found".into())))?
            .clone();
        let input = self
            .inputs
            .lock()
            .unwrap()
            .get(input_id)
            .ok_or(ProverError::BonsaiErr(SdkErr::InternalServerErr("input not found".into())))?
            .clone();

        let mut env = ExecutorEnv::builder();
        env.write_slice(&input);
        env.session_limit(executor_limit);

        for assumption_id in assumptions.iter() {
            let assumption_receipt = self
                .starks
                .lock()
                .unwrap()
                .get(assumption_id)
                .ok_or(ProverError::BonsaiErr(SdkErr::InternalServerErr(
                    "assumption not found".into(),
                )))?
                .clone();
            env.add_assumption(assumption_receipt.1.clone());
        }
        let start = Instant::now();
        let env = env.build().map_err(|_| {
            ProverError::BonsaiErr(SdkErr::InternalServerErr("failed to build env".into()))
        })?;
        let elapsed = Instant::now() - start;

        let image_id = compute_image_id(&image).unwrap();
        let session = default_executor().execute(env, &image).unwrap();
        let id = Uuid::new_v4().to_string();

        let receipt = Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                image_id,
                MaybePruned::Pruned(session.journal.digest()),
            ))),
            session.journal.bytes,
        );

        // TODO: Get total cycles
        let cycles = session.segments.iter().map(|segment| segment.cycles as u64).sum();
        let proof_res = ProofResult {
            id: id.clone(),
            stats: ExecutorResp {
                assumption_count: assumptions.len() as u64,
                segments: session.segments.len() as u64,
                user_cycles: cycles,
                total_cycles: cycles,
            },
            elapsed_time: elapsed.as_secs_f32().into(),
        };

        self.starks.lock().unwrap().insert(id.clone(), (proof_res.clone(), receipt.clone()));

        Ok(proof_res)
    }
}

#[async_trait]
impl Prover for MockProver {
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        let id = Uuid::new_v4().to_string();
        self.inputs.lock().unwrap().insert(id.clone(), input);
        Ok(id)
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        self.images.lock().unwrap().insert(image_id.to_string(), image);
        Ok(())
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError> {
        self.mock_prove_stark(image_id, input_id, assumptions, executor_limit)
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        Ok(self.mock_prove_stark(image_id, input_id, assumptions, None)?.id)
    }

    async fn prove_and_monitor_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<ProofResult, ProverError> {
        self.mock_prove_stark(image_id, input_id, assumptions, None)
    }

    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError> {
        let starks_lock = self.starks.lock().unwrap();
        let res = starks_lock.get(proof_id).unwrap();
        Ok(res.0.clone())
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
        let proofs = self.starks.lock().unwrap();
        let Some(res) = proofs.get(proof_id) else {
            return Ok(None);
        };

        Ok(Some(res.1.clone()))
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        self.get_journal(proof_id).await
    }

    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let proofs = self.starks.lock().unwrap();
        let Some(res) = proofs.get(proof_id) else {
            return Ok(None);
        };

        Ok(Some(res.1.journal.clone().bytes))
    }

    async fn compress(&self, proof_id: &str) -> Result<String, ProverError> {
        let id = Uuid::new_v4().to_string();
        let proofs = self.starks.lock().unwrap();
        let proof = proofs.get(proof_id).unwrap();
        self.snarks.lock().unwrap().insert(id.clone(), proof.1.clone());
        Ok(id)
    }

    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let proofs = self.snarks.lock().unwrap();
        let Some(res) = proofs.get(proof_id) else {
            return Ok(None);
        };

        Ok(Some(bincode::serialize(&res).unwrap()))
    }
}
