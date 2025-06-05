// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::future::Future;

use async_trait::async_trait;
use bonsai_sdk::{
    non_blocking::{Client as BonsaiClient, SessionId, SnarkId},
    SdkErr,
};
use risc0_zkvm::Receipt;
use sqlx::{self, Postgres, Transaction};

use super::{ExecutorResp, ProofResult, Prover, ProverError};
use crate::{config::ProverConf, futures_retry::retry_only};
use crate::{
    config::{ConfigErr, ConfigLock},
    futures_retry::retry,
};

#[derive(Debug, Clone, Copy)]
enum ProverType {
    Bonsai,
    Bento,
}

pub struct Bonsai {
    client: BonsaiClient,
    req_retry_sleep_ms: u64,
    req_retry_count: u64,
    status_poll_ms: u64,
    status_poll_retry_count: u64,
    prover_type: ProverType,
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

        let prover_type = if api_key.is_empty() { ProverType::Bento } else { ProverType::Bonsai };

        Ok(Self {
            client: BonsaiClient::from_parts(api_url.into(), api_key.into(), &risc0_ver)?,
            req_retry_sleep_ms,
            req_retry_count,
            status_poll_ms,
            status_poll_retry_count,
            prover_type,
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

    async fn retry<T, F, Fut>(&self, f: F, msg: &str) -> Result<T, ProverError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, ProverError>>,
    {
        retry::<T, ProverError, _, _>(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { f().await },
            msg,
        )
        .await
    }

    async fn retry_only<T, F, Fut>(
        &self,
        f: F,
        msg: &str,
        should_retry: impl Fn(&ProverError) -> bool,
    ) -> Result<T, ProverError>
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Result<T, ProverError>>,
    {
        retry_only(
            self.req_retry_count,
            self.req_retry_sleep_ms,
            || async { f().await },
            msg,
            should_retry,
        )
        .await
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
                    tracing::trace!(
                        "Proof {proof_id:?} succeeded with user cycles: {} and total cycles: {}",
                        stats.cycles,
                        stats.total_cycles
                    );
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
                    if err_msg.contains("INTERNAL_ERROR") {
                        return Err(ProverError::ProverInternalError(err_msg.clone()));
                    }
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
                    if err_msg.contains("INTERNAL_ERROR") {
                        return Err(ProverError::ProverInternalError(err_msg.clone()));
                    }
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
    async fn has_image(&self, image_id: &str) -> Result<bool, ProverError> {
        let status = self
            .retry(|| async { Ok(self.client.has_img(image_id).await?) }, "check image")
            .await?;
        Ok(status)
    }

    async fn upload_input(&self, input: Vec<u8>) -> Result<String, ProverError> {
        self.retry(|| async { Ok(self.client.upload_input(input.clone()).await?) }, "upload input")
            .await
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<(), ProverError> {
        self.retry(
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
        self.retry_only(
            || async {
                let preflight_id: SessionId = self
                    .retry(
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
            },
            "preflight",
            |err| matches!(err, ProverError::ProverInternalError(_)),
        )
        .await
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String, ProverError> {
        self.retry(
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
        tracing::debug!("Created session for prove stark: {proof_id}");
        self.wait_for_stark(&proof_id).await
    }

    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult, ProverError> {
        tracing::debug!("Waiting for stark proof {} to complete", proof_id);
        let proof_id = SessionId::new(proof_id.into());

        let poller = StatusPoller {
            poll_sleep_ms: self.status_poll_ms,
            retry_counts: self.status_poll_retry_count,
        };

        poller.poll_with_retries_session_id(&proof_id, &self.client).await
    }

    async fn cancel_stark(&self, proof_id: &str) -> Result<(), ProverError> {
        // TODO this is a temporary workaround to cancel a job in Bento. This should be implemented
        // and migrated to use just the Bonsai API in future versions.
        match self.prover_type {
            ProverType::Bonsai => {
                tracing::debug!("Cancelling Bonsai stark session {}", proof_id);
                let session_id = SessionId::new(proof_id.into());
                session_id.stop(&self.client).await?;
                Ok(())
            }
            ProverType::Bento => {
                tracing::debug!("Cancelling Bento job {}", proof_id);
                // Create postgres connection for Bento cancellation
                match create_pg_pool().await {
                    Ok(pool) => {
                        let mut tx: Transaction<'_, Postgres> = match pool.begin().await {
                            Ok(tx) => tx,
                            Err(e) => {
                                tracing::error!("Failed to begin transaction: {}", e);
                                return Err(ProverError::ProvingFailed(format!(
                                    "Failed to begin transaction: {}",
                                    e
                                )));
                            }
                        };
                        if let Err(e) =
                            sqlx::query("UPDATE jobs SET state = 'failed' WHERE id = $1::uuid")
                                .bind(proof_id)
                                .execute(&mut *tx)
                                .await
                        {
                            tracing::error!("Failed to update job state: {}", e);
                            return Err(ProverError::ProvingFailed(format!(
                                "Failed to update job: {}",
                                e
                            )));
                        }

                        if let Err(e) = sqlx::query("DELETE FROM task_deps WHERE job_id = $1::uuid")
                            .bind(proof_id)
                            .execute(&mut *tx)
                            .await
                        {
                            tracing::error!("Failed to delete task dependencies: {}", e);
                            return Err(ProverError::ProvingFailed(format!(
                                "Failed to delete task deps: {}",
                                e
                            )));
                        }

                        if let Err(e) = sqlx::query("DELETE FROM tasks WHERE job_id = $1::uuid")
                            .bind(proof_id)
                            .execute(&mut *tx)
                            .await
                        {
                            tracing::error!("Failed to delete tasks: {}", e);
                            return Err(ProverError::ProvingFailed(format!(
                                "Failed to delete tasks: {}",
                                e
                            )));
                        }

                        if let Err(e) = tx.commit().await {
                            tracing::error!("Failed to commit transaction: {}", e);
                            return Err(ProverError::ProvingFailed(format!(
                                "Failed to commit transaction: {}",
                                e
                            )));
                        }

                        tracing::info!("Successfully cancelled Bento job {}", proof_id);
                        Ok(())
                    }
                    Err(e) => {
                        tracing::error!("Failed to connect to PostgreSQL: {}", e);
                        Err(ProverError::ProvingFailed(format!(
                            "Failed to connect to postgres: {}",
                            e
                        )))
                    }
                }
            }
        }
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let receipt = self
            .retry(|| async { Ok(self.client.receipt_download(&session_id).await?) }, "get receipt")
            .await?;
        Ok(Some(bincode::deserialize(&receipt)?))
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>, ProverError> {
        let session_id = SessionId { uuid: proof_id.into() };
        let journal = self
            .retry(
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
        let proof_id = self
            .retry(
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
        let status = self
            .retry(|| async { Ok(snark_id.status(&self.client).await?) }, "get status of snark")
            .await?;

        let Some(output) = status.output else { return Ok(None) };
        let receipt_buf = self
            .retry(|| async { Ok(self.client.download(&output).await?) }, "download snark output")
            .await?;

        Ok(Some(receipt_buf))
    }
}

async fn create_pg_pool() -> Result<sqlx::PgPool, sqlx::Error> {
    let user = std::env::var("POSTGRES_USER").unwrap_or_else(|_| "worker".to_string());
    let password = std::env::var("POSTGRES_PASSWORD").unwrap_or_else(|_| "password".to_string());
    let db = std::env::var("POSTGRES_DB").unwrap_or_else(|_| "taskdb".to_string());
    let host = match std::env::var("POSTGRES_HOST").unwrap_or_else(|_| "postgres".to_string()) {
        host if host != "postgres" => host,
        // Use local connection for postgres, as "postgres" not compatible with docker
        _ => "127.0.0.1".to_string(),
    };

    let port = std::env::var("POSTGRES_PORT").unwrap_or_else(|_| "5432".to_string());

    let connection_string = format!("postgres://{}:{}@{}:{}/{}", user, password, host, port, db);

    sqlx::PgPool::connect(&connection_string).await
}
