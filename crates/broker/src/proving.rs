// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use crate::{
    config::ConfigLock,
    db::DbObj,
    errors::CodedError,
    futures_retry::retry,
    impl_coded_debug,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    utils::cancel_proof_and_fail_order,
    Order, OrderStatus,
};
use anyhow::{Context, Result};
use thiserror::Error;

#[derive(Error)]
pub enum ProvingErr {
    #[error("{code} Proving failed after retries: {0:?}", code = self.code())]
    ProvingFailed(anyhow::Error),

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),
}

impl_coded_debug!(ProvingErr);

impl CodedError for ProvingErr {
    fn code(&self) -> &str {
        match self {
            ProvingErr::ProvingFailed(_) => "[B-PRO-501]",
            ProvingErr::UnexpectedError(_) => "[B-PRO-500]",
        }
    }
}

#[derive(Clone)]
pub struct ProvingService {
    db: DbObj,
    prover: ProverObj,
    config: ConfigLock,
}

impl ProvingService {
    pub async fn new(db: DbObj, prover: ProverObj, config: ConfigLock) -> Result<Self> {
        Ok(Self { db, prover, config })
    }

    async fn monitor_proof_internal(
        &self,
        order_id: &str,
        stark_proof_id: &str,
        is_groth16: bool,
        snark_proof_id: Option<String>,
    ) -> Result<OrderStatus> {
        let proof_res = self
            .prover
            .wait_for_stark(stark_proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        if is_groth16 && snark_proof_id.is_none() {
            let compressed_proof_id =
                self.prover.compress(stark_proof_id).await.context("Failed to compress proof")?;
            self.db
                .set_order_compressed_proof_id(order_id, &compressed_proof_id)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set order {order_id} compressed proof id: {compressed_proof_id}"
                    )
                })?;
        };

        let status = match is_groth16 {
            false => OrderStatus::PendingAgg,
            true => OrderStatus::SkipAggregation,
        };

        tracing::info!(
            "Customer Proof complete for proof_id: {stark_proof_id}, order_id: {order_id} cycles: {} time: {}",
            proof_res.stats.total_cycles,
            proof_res.elapsed_time,
        );

        Ok(status)
    }

    pub async fn monitor_proof_with_timeout(&self, order: Order) -> Result<OrderStatus> {
        let order_id = order.id();

        // Get the proof_id - either from existing order or create new proof
        let proof_id = match order.proof_id.clone() {
            Some(existing_proof_id) => {
                tracing::debug!(
                    "Monitoring existing proof {existing_proof_id} for order {order_id}"
                );
                existing_proof_id
            }
            None => {
                // This is a new order that needs proving
                tracing::info!("Proving order {order_id}");

                // If the ID's are not present then upload them now
                // Mostly hit by skipping pre-flight
                let image_id = match order.image_id.as_ref() {
                    Some(val) => val.clone(),
                    None => {
                        crate::storage::upload_image_uri(&self.prover, &order.request, &self.config)
                            .await
                            .context("Failed to upload image")?
                    }
                };

                let input_id = match order.input_id.as_ref() {
                    Some(val) => val.clone(),
                    None => {
                        crate::storage::upload_input_uri(&self.prover, &order.request, &self.config)
                            .await
                            .context("Failed to upload input")?
                    }
                };

                let proof_id = self
                    .prover
                    .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
                    .await
                    .context("Failed to prove customer proof STARK order")?;

                tracing::debug!("Order {order_id} being proved, proof id: {proof_id}");

                self.db.set_order_proof_id(&order_id, &proof_id).await.with_context(|| {
                    format!("Failed to set order {order_id} proof id: {}", proof_id)
                })?;

                proof_id
            }
        };

        let timeout_duration = {
            let expiry_timestamp_secs =
                order.expire_timestamp.expect("Order should have expiry set");
            let now = crate::now_timestamp();
            Duration::from_secs(expiry_timestamp_secs.saturating_sub(now))
        };

        let monitor_task = self.monitor_proof_internal(
            &order_id,
            &proof_id,
            order.is_groth16(),
            order.compressed_proof_id,
        );

        // Note: this timeout may not exactly match the order expiry exactly due to
        // discrepancy between wall clock and monotonic clock from the timeout,
        // but this time, along with aggregation and submission time, should never
        // exceed the actual order expiry.
        let order_status = match tokio::time::timeout(timeout_duration, monitor_task).await {
            Ok(result) => result.context("Monitoring proof failed")?,
            Err(_) => {
                tracing::debug!(
                    "Proving timed out for order {}, cancelling proof {}",
                    order_id,
                    proof_id
                );
                if let Err(err) = self.prover.cancel_stark(&proof_id).await {
                    tracing::warn!(
                        "Failed to cancel proof {} for timed out order {}: {}",
                        proof_id,
                        order_id,
                        err
                    );
                }
                return Err(anyhow::anyhow!("Proving timed out"));
            }
        };

        Ok(order_status)
    }

    async fn prove_and_update_db(&self, order: Order) {
        let order_id = order.id();

        let (proof_retry_count, proof_retry_sleep_ms) = {
            let config = self.config.lock_all().unwrap();
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

        let result = retry(
            proof_retry_count,
            proof_retry_sleep_ms,
            || async { self.monitor_proof_with_timeout(order.clone()).await },
            "monitor_proof_with_timeout",
        )
        .await;

        match result {
            Ok(order_status) => {
                tracing::info!("Successfully completed proof monitoring for order {order_id}");

                if let Err(e) = self.db.set_aggregation_status(&order_id, order_status).await {
                    tracing::error!("Failed to set aggregation status for order {order_id}: {e:?}");
                }
            }
            Err(err) => {
                let proving_err = ProvingErr::ProvingFailed(err);
                tracing::error!(
                    "FATAL: Order {} failed to prove after {} retries: {proving_err:?}",
                    order_id,
                    proof_retry_count
                );

                handle_order_failure(&self.db, &order_id, "Proving failed").await;
            }
        }
    }

    pub async fn find_and_monitor_proofs(&self) -> Result<(), ProvingErr> {
        let current_proofs =
            self.db.get_active_proofs().await.context("Failed to get active proofs")?;

        tracing::info!("Found {} proofs currently proving", current_proofs.len());
        let now = crate::now_timestamp();
        for order in current_proofs {
            let order_id = order.id();
            if order.expire_timestamp.unwrap() < now {
                tracing::warn!("Order {} had expired on proving task start", order_id);
                if let Some(proof_id) = &order.proof_id {
                    cancel_proof_and_fail_order(
                        &self.prover,
                        &self.db,
                        proof_id,
                        &order_id,
                        "Order expired on startup",
                    )
                    .await;
                } else {
                    handle_order_failure(&self.db, &order_id, "Order expired on startup").await;
                }
            }
            let prove_serv = self.clone();

            if order.proof_id.is_none() {
                tracing::error!("Order in status Proving missing proof_id: {order_id}");
                handle_order_failure(&prove_serv.db, &order_id, "Proving status missing proof_id")
                    .await;
                continue;
            }

            // TODO: Manage these tasks in a joinset?
            // They should all be fail-able without triggering a larger failure so it should be
            // fine.
            tokio::spawn(async move { prove_serv.prove_and_update_db(order).await });
        }

        Ok(())
    }
}

impl RetryTask for ProvingService {
    type Error = ProvingErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let proving_service_copy = self.clone();
        Box::pin(async move {
            tracing::info!("Starting proving service");

            // First search the DB for any existing dangling proofs and kick off their concurrent
            // monitors
            proving_service_copy.find_and_monitor_proofs().await.map_err(SupervisorErr::Fault)?;

            // Start monitoring for new proofs
            loop {
                // TODO: parallel_proofs management
                // we need to query the Bento/Bonsai backend and constrain the number of running
                // parallel proofs currently bonsai does not have this feature but
                // we could add it to both to support it. Alternatively we could
                // track it in our local DB but that could de-sync from the proving-backend so
                // its not ideal
                let order_res = proving_service_copy
                    .db
                    .get_proving_order()
                    .await
                    .context("Failed to get proving order")
                    .map_err(ProvingErr::UnexpectedError)
                    .map_err(SupervisorErr::Recover)?;

                if let Some(order) = order_res {
                    let prov_serv = proving_service_copy.clone();
                    tokio::spawn(async move { prov_serv.prove_and_update_db(order).await });
                }

                // TODO: configuration
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        })
    }
}

async fn handle_order_failure(db: &DbObj, order_id: &str, failure_reason: &'static str) {
    if let Err(inner_err) = db.set_order_failure(order_id, failure_reason).await {
        tracing::error!("Failed to set order {order_id} failure: {inner_err:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::SqliteDb,
        now_timestamp,
        provers::{encode_input, DefaultProver},
        FulfillmentType, OrderStatus,
    };
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Offer, Predicate, PredicateType, ProofRequest, RequestInput, RequestInputType, Requirements,
    };
    use boundless_market_test_utils::{ECHO_ELF, ECHO_ID};
    use chrono::Utc;
    use risc0_zkvm::sha::Digest;
    use std::sync::Arc;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn prove_order() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        let proving_service =
            ProvingService::new(db.clone(), prover, config.clone()).await.unwrap();

        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::PendingProving,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: ProofRequest {
                id: U256::ZERO,
                requirements: Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                imageUrl: "http://risczero.com/image".into(),
                input: RequestInput {
                    inputType: RequestInputType::Inline,
                    data: Default::default(),
                },
                offer: Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: now_timestamp(),
                    rampUpPeriod: 1,
                    lockTimeout: 100,
                    timeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 3600), // 1 hour from now
            client_sig: Bytes::new(),
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            proving_started_at: None,
        };

        db.add_order(&order).await.unwrap();

        proving_service.prove_and_update_db(order.clone()).await;

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
    }

    #[tokio::test]
    #[traced_test]
    async fn resume_proving() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        let prover: ProverObj = Arc::new(DefaultProver::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        // pre-prove the stark so it already exists before the service comes up
        let proof_id = prover.prove_stark(&image_id, &input_id, vec![]).await.unwrap();

        let proving_service =
            ProvingService::new(db.clone(), prover, config.clone()).await.unwrap();

        let order_id = U256::ZERO;
        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::Proving,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: ProofRequest {
                id: order_id,
                requirements: Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                imageUrl: "http://risczero.com/image".into(),
                input: RequestInput {
                    inputType: RequestInputType::Inline,
                    data: Default::default(),
                },
                offer: Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: now_timestamp(),
                    rampUpPeriod: 1,
                    timeout: 100,
                    lockTimeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: Some(proof_id.clone()),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 3600), // 1 hour from now
            client_sig: Bytes::new(),
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            proving_started_at: None,
        };
        db.add_order(&order).await.unwrap();

        proving_service.find_and_monitor_proofs().await.unwrap();

        // Sleep long enough for the tokio tasks to pickup and complete the order in the DB
        loop {
            let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Proving {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
        assert_eq!(order.proof_id, Some(proof_id));

        assert!(logs_contain("Found 1 proofs currently proving"));
    }
}
