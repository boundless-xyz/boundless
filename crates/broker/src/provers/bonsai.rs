// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use async_trait::async_trait;
use bonsai_sdk::{
    non_blocking::{Client as BonsaiClient, SessionId, SnarkId},
    responses::SnarkStatusRes,
    SdkErr,
};
use risc0_zkvm::Receipt;

use super::{ExecutorResp, ProofResult, Prover, ProverError};
use crate::config::ProverConf;
use crate::{
    config::{ConfigErr, ConfigLock},
    futures_retry::retry,
};

pub struct Bonsai {
    client: BonsaiClient,
    req_retry_sleep_ms: u64,
    req_retry_count: u64,
    status_poll_ms: u64,
    status_poll_retry_count: u64,
}

impl Bonsai {
    pub fn new(config: ConfigLock, api_url: &str, api_key: &str) -> Result<Self, ProverError> {
        let (
            risc0_ver,
            req_retry_count,
            req_retry_sleep_ms,
            status_poll_ms,
            status_poll_retry_count,
        ) = {
            let config = config.lock_all().unwrap();
            (
                config.prover.bonsai_r0_zkvm_ver.as_ref().ok_or(ConfigErr::InvalidConfig)?.clone(),
                config.prover.req_retry_count,
                config.prover.req_retry_sleep_ms,
                config.prover.status_poll_ms,
                config.prover.status_poll_retry_count,
            )
        };

        Ok(Self {
            client: BonsaiClient::from_parts(api_url.into(), api_key.into(), &risc0_ver)?,
            req_retry_sleep_ms,
            req_retry_count,
            status_poll_ms,
            status_poll_retry_count,
        })
    }

    pub async fn compress(
        client: &BonsaiClient,
        receipt: &Receipt,
        cfg: &ProverConf,
    ) -> Result<Receipt, ProverError> {
        let receipt_bytes = bincode::serialize(receipt).unwrap();
        let session_id = retry::<String, ProverError, _, _>(
            cfg.req_retry_count,
            cfg.req_retry_sleep_ms,
            || async { Ok(client.upload_receipt(receipt_bytes.clone()).await?) },
            "upload input",
        )
        .await?;
        let proof_id = retry::<SnarkId, ProverError, _, _>(
            cfg.req_retry_count,
            cfg.req_retry_sleep_ms,
            || async { Ok(client.create_snark(session_id.clone()).await?) },
            "create snark",
        )
        .await?;

        loop {
            let status = retry::<_, SdkErr, _, _>(
                cfg.status_poll_retry_count,
                cfg.status_poll_ms,
                || async { proof_id.status(client).await },
                "get snark status",
            )
            .await?;

            match status.status.as_ref() {
                "RUNNING" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(cfg.status_poll_ms))
                        .await;
                    continue;
                }
                "SUCCEEDED" => {
                    let output = status.output.unwrap();
                    let receipt_buf = client.download(&output).await?;
                    return Ok(bincode::deserialize(&receipt_buf)?);
                }
                _ => {
                    let err_msg = status.error_msg.unwrap_or_default();
                    return Err(ProverError::ProvingFailed(format!(
                        "snark proving failed: {err_msg}"
                    )));
                }
            }
        }
    }
}

struct StatusPoller {
    poll_sleep_ms: u64,
    retry_counts: u64,
}

impl StatusPoller {
    async fn poll_with_retries_session_id(
        &self,
        proof_id: &SessionId,
        client: &BonsaiClient,
    ) -> Result<ProofResult, ProverError> {
        loop {
            let status = retry::<_, SdkErr, _, _>(
                self.retry_counts,
                self.poll_sleep_ms,
                || async { proof_id.status(client).await },
                "get session status",
            )
            .await;

            if let Err(_err) = status {
                return Err(ProverError::StatusFailure);
            }

            let status = status.unwrap();

            match status.status.as_ref() {
                "RUNNING" => {
                    tracing::trace!(
                        "Session {proof_id:?} is still running. Elapsed time: {}",
                        status.elapsed_time.unwrap_or(f64::NAN)
                    );
                    tokio::time::sleep(tokio::time::Duration::from_millis(self.poll_sleep_ms))
                        .await;
                    continue;
                }
                "SUCCEEDED" => {
                    let Some(stats) = status.stats else {
                        return Err(ProverError::MissingStatus);
                    };
                    return Ok(ProofResult {
                        id: proof_id.uuid.clone(),
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
                        "{proof_id:?} failed: {err_msg}"
                    )));
                }
            }
        }
    }

    async fn poll_with_retries_snark_id(
        &self,
        proof_id: &SnarkId,
        client: &BonsaiClient,
    ) -> Result<String, ProverError> {
        loop {
            let status = retry::<_, SdkErr, _, _>(
                self.retry_counts,
                self.poll_sleep_ms,
                || async { proof_id.status(client).await },
                "get snark status",
            )
            .await;

            if let Err(_err) = status {
                return Err(ProverError::StatusFailure);
            }

            let status = status.unwrap();

            match status.status.as_ref() {
                "RUNNING" => {
                    tokio::time::sleep(tokio::time::Duration::from_millis(self.poll_sleep_ms))
                        .await;
                    continue;
                }
                "SUCCEEDED" => return Ok(proof_id.uuid.clone()),
                _ => {
                    let err_msg = status.error_msg.unwrap_or_default();
                    return Err(ProverError::ProvingFailed(format!(
                        "snark proving failed: {err_msg}"
                    )));
                }
            }
        }
    }
}

#[async_trait]
impl Prover for Bonsai {
    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        retry::<String, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(self.client.upload_input(input.clone()).await?) },
            "upload input",
        )
        .await
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        retry::<(), ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(self.client.upload_img(image_id, image.clone()).await.map(|_| ())?) },
            "upload image",
        )
        .await
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
    ) -> Result<ProofResult, ProverError> {
        let preflight_id: SessionId = retry::<SessionId, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async {
                Ok(self
                    .client
                    .create_session_with_limit(
                        image_id.into(),
                        input_id.into(),
                        assumptions.clone(),
                        true,
                        executor_limit,
                    )
                    .await?)
            },
            "create session for preflight",
        )
        .await?;

        let poller = StatusPoller {
            poll_sleep_ms: self.status_poll_ms,
            retry_counts: self.status_poll_retry_count,
        };
        poller.poll_with_retries_session_id(&preflight_id, &self.client).await
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        retry::<_, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async {
                Ok(self
                    .client
                    .create_session(image_id.into(), input_id.into(), assumptions.clone(), false)
                    .await?
                    .uuid)
            },
            "create session for prove stark",
        )
        .await
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

        let poller = StatusPoller {
            poll_sleep_ms: self.status_poll_ms,
            retry_counts: self.status_poll_retry_count,
        };

        poller.poll_with_retries_session_id(&proof_id, &self.client).await
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let receipt = retry::<Vec<u8>, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(self.client.receipt_download(&session_id).await?) },
            "get receipt",
        )
        .await?;
        Ok(Some(bincode::deserialize(&receipt)?))
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let journal = retry::<Vec<u8>, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(session_id.exec_only_journal(&self.client).await?) },
            "get preflight journal",
        )
        .await?;
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
        let proof_id = retry::<SnarkId, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(self.client.create_snark(proof_id.into()).await?) },
            "create snark",
        )
        .await?;

        let poller = StatusPoller {
            poll_sleep_ms: self.status_poll_ms,
            retry_counts: self.status_poll_retry_count,
        };

        poller.poll_with_retries_snark_id(&proof_id, &self.client).await?;

        Ok(proof_id.uuid)
    }

    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let snark_id = SnarkId { uuid: proof_id.into() };
        let status = retry::<SnarkStatusRes, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(snark_id.status(&self.client).await?) },
            "get status of snark",
        )
        .await?;

        let Some(output) = status.output else { return Ok(None) };
        let receipt_buf = retry::<Vec<u8>, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { Ok(self.client.download(&output).await?) },
            "download snark output",
        )
        .await?;

        Ok(Some(receipt_buf))
    }
}
