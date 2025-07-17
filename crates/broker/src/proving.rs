// Copyright 2025 RISC Zero, Inc.
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

use std::future::pending;
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
    Order, OrderStateChange, OrderStatus,
};
use anyhow::{Context, Result};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Error)]
pub enum ProvingErr {
    #[error("{code} Proving failed after retries: {0:?}", code = self.code())]
    ProvingFailed(anyhow::Error),

    #[error("{code} Request fulfilled by another prover", code = self.code())]
    ExternallyFulfilled,

    #[error("{code} Proving timed out", code = self.code())]
    ProvingTimedOut,

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedError(#[from] anyhow::Error),
}

impl_coded_debug!(ProvingErr);

impl CodedError for ProvingErr {
    fn code(&self) -> &str {
        match self {
            ProvingErr::ProvingFailed(_) => "[B-PRO-501]",
            ProvingErr::ExternallyFulfilled => "[B-PRO-502]",
            ProvingErr::ProvingTimedOut => "[B-PRO-503]",
            ProvingErr::UnexpectedError(_) => "[B-PRO-500]",
        }
    }
}

#[derive(Clone)]
pub struct ProvingService {
    db: DbObj,
    prover: ProverObj,
    config: ConfigLock,
    order_state_tx: tokio::sync::broadcast::Sender<OrderStateChange>,
}

impl ProvingService {
    pub async fn new(
        db: DbObj,
        prover: ProverObj,
        config: ConfigLock,
        order_state_tx: tokio::sync::broadcast::Sender<OrderStateChange>,
    ) -> Result<Self> {
        Ok(Self { db, prover, config, order_state_tx })
    }

    async fn cancel_stark_session(&self, proof_id: &str, order_id: &str, reason: &str) {
        if let Err(err) = self.prover.cancel_stark(proof_id).await {
            tracing::warn!(
                "Failed to cancel proof {} for {} order {}: {}",
                proof_id,
                reason,
                order_id,
                err
            );
        }
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

    async fn get_or_create_stark_session(&self, order: Order) -> Result<String> {
        let order_id = order.id();

        // Get the proof_id - either from existing order or create new proof
        match order.proof_id.clone() {
            Some(existing_proof_id) => {
                tracing::debug!("Using existing proof {existing_proof_id} for order {order_id}");
                Ok(existing_proof_id)
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
                    format!("Failed to set order {order_id} proof id: {proof_id}")
                })?;

                Ok(proof_id)
            }
        }
    }

    pub async fn monitor_proof_with_timeout(
        &self,
        order: Order,
    ) -> Result<OrderStatus, ProvingErr> {
        let order_id = order.id();
        let request_id = order.request.id;

        let proof_id = order.proof_id.as_ref().context("Order should have proof ID")?;

        let timeout_duration = {
            let expiry_timestamp_secs =
                order.expire_timestamp.expect("Order should have expiry set");
            let now = crate::now_timestamp();
            Duration::from_secs(expiry_timestamp_secs.saturating_sub(now))
        };
        // Only subscribe to order state events for FulfillAfterLockExpire orders
        let mut order_state_rx = if matches!(
            order.fulfillment_type,
            crate::FulfillmentType::FulfillAfterLockExpire
        ) {
            let rx = self.order_state_tx.subscribe();

            // Check if the order has already been fulfilled before starting proof
            match self.db.is_request_fulfilled(request_id).await {
                Ok(true) => {
                    tracing::debug!(
                        "Order {} (request {}) was already fulfilled, skipping proof",
                        order_id,
                        request_id
                    );
                    self.cancel_stark_session(proof_id, &order_id, "externally fulfilled").await;
                    return Err(ProvingErr::ExternallyFulfilled);
                }
                Ok(false) => Some(rx),
                Err(e) => {
                    tracing::warn!(
                        "Failed to check fulfillment status for order {}, will continue proving: {e:?}",
                        order_id,
                    );
                    Some(rx)
                }
            }
        } else {
            None
        };

        let monitor_task = self.monitor_proof_internal(
            &order_id,
            proof_id,
            order.is_groth16(),
            order.compressed_proof_id,
        );
        tokio::pin!(monitor_task);

        // Note: this timeout may not exactly match the order expiry exactly due to
        // discrepancy between wall clock and monotonic clock from the timeout,
        // but this time, along with aggregation and submission time, should never
        // exceed the actual order expiry.
        let timeout_future = tokio::time::sleep(timeout_duration);
        tokio::pin!(timeout_future);

        let order_status = loop {
            tokio::select! {
                // Proof monitoring completed
                res = &mut monitor_task => {
                    break res.with_context(|| {
                        format!("Monitoring proof failed for order {order_id}, proof_id: {proof_id}")
                    }).map_err(ProvingErr::ProvingFailed)?;
                }
                // Timeout occurred
                _ = &mut timeout_future => {
                    tracing::debug!(
                        "Proving timed out for order {}, cancelling proof {}",
                        order_id,
                        proof_id
                    );
                    self.cancel_stark_session(proof_id, &order_id, "timed out").await;
                    return Err(ProvingErr::ProvingTimedOut);
                }
                // External fulfillment notification (only active for FulfillAfterLockExpire orders)
                Some(recv_res) = async {
                    match &mut order_state_rx {
                        Some(rx) => Some(rx.recv().await),
                        None => pending::<Option<Result<OrderStateChange, tokio::sync::broadcast::error::RecvError>>>().await,
                    }
                } => {
                    match recv_res {
                        Ok(OrderStateChange::Fulfilled { request_id: fulfilled_request_id }) if fulfilled_request_id == request_id => {
                            tracing::debug!(
                                "Order {} (request {}) was fulfilled by another prover, cancelling proof {}",
                                order_id,
                                request_id,
                                proof_id
                            );
                            self.cancel_stark_session(proof_id, &order_id, "externally fulfilled").await;
                            return Err(ProvingErr::ExternallyFulfilled);
                        }
                        Ok(_) => {
                            // Fulfillment for a different request, continue monitoring
                        }
                        Err(_) => {
                            // Channel closed or lagged, continue monitoring
                            tracing::trace!("Fulfillment channel error, continuing proof monitoring");
                        }
                    }
                }
            }
        };

        Ok(order_status)
    }

    async fn prove_and_update_db(&self, mut order: Order) {
        let order_id = order.id();

        let (proof_retry_count, proof_retry_sleep_ms) = {
            let config = self.config.lock_all().unwrap();
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

        let proof_id = match retry(
            proof_retry_count,
            proof_retry_sleep_ms,
            || async { self.get_or_create_stark_session(order.clone()).await },
            "get_or_create_stark_session",
        )
        .await
        {
            Ok(proof_id) => proof_id,
            Err(err) => {
                let proving_err = ProvingErr::ProvingFailed(err);
                tracing::error!(
                    "Failed to create stark session for order {order_id}: {proving_err:?}"
                );
                handle_order_failure(&self.db, &order_id, "Proving session create failed").await;
                return;
            }
        };

        order.proof_id = Some(proof_id);

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
            Err(ProvingErr::ExternallyFulfilled) => {
                tracing::info!("Order {order_id} was fulfilled by another prover, cancelled proof");
                handle_order_failure(&self.db, &order_id, "Externally fulfilled").await;
            }
            Err(err) => {
                tracing::error!(
                    "Order {} failed to prove after {} retries: {err:?}",
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
                cancel_proof_and_fail_order(
                    &self.prover,
                    &self.db,
                    &order,
                    "Order expired on startup",
                )
                .await;
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
    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let proving_service_copy = self.clone();
        Box::pin(async move {
            tracing::info!("Starting proving service");

            // First search the DB for any existing dangling proofs and kick off their concurrent
            // monitors
            proving_service_copy.find_and_monitor_proofs().await.map_err(SupervisorErr::Fault)?;

            // Start monitoring for new proofs
            let mut proving_interval = tokio::time::interval(Duration::from_millis(500));
            proving_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                if cancel_token.is_cancelled() {
                    tracing::debug!("Proving service received cancellation");
                    break;
                }

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

            Ok(())
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

    fn create_test_order(
        request_id: U256,
        image_id: String,
        input_id: String,
        proof_id: Option<String>,
        fulfillment_type: FulfillmentType,
        status: OrderStatus,
    ) -> Order {
        Order {
            status,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: ProofRequest {
                id: request_id,
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
                    minPrice: U256::from(2),
                    maxPrice: U256::from(4),
                    biddingStart: now_timestamp(),
                    rampUpPeriod: 1,
                    lockTimeout: 100,
                    timeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id,
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 3600), // 1 hour from now
            client_sig: Bytes::new(),
            lock_price: None,
            fulfillment_type,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            proving_started_at: None,
        }
    }

    async fn send_order_state_event(
        order_state_tx: tokio::sync::broadcast::Sender<OrderStateChange>,
        state_change: OrderStateChange,
    ) {
        for _ in 0..50 {
            // Try for up to 5 seconds
            tokio::time::sleep(Duration::from_millis(100)).await;
            if order_state_tx.send(state_change.clone()).is_ok() {
                return;
            }
        }
        panic!("Failed to send order state event within timeout");
    }

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

        let (order_state_tx, _) = tokio::sync::broadcast::channel(100);
        let proving_service =
            ProvingService::new(db.clone(), prover.clone(), config.clone(), order_state_tx)
                .await
                .unwrap();

        let order = create_test_order(
            U256::ZERO,
            image_id.clone(),
            input_id.clone(),
            None,
            FulfillmentType::LockAndFulfill,
            OrderStatus::PendingProving,
        );

        db.add_order(&order).await.unwrap();

        proving_service.prove_and_update_db(order.clone()).await;

        let order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);

        // Test that LockAndFulfill orders ignore fulfillment events
        let (order_state_tx, _) = tokio::sync::broadcast::channel(100);
        let proving_service_with_fulfillment =
            ProvingService::new(db.clone(), prover.clone(), config.clone(), order_state_tx.clone())
                .await
                .unwrap();

        let lock_and_fulfill_order = create_test_order(
            U256::from(999),
            image_id,
            input_id,
            None,
            FulfillmentType::LockAndFulfill,
            OrderStatus::PendingProving,
        );

        db.add_order(&lock_and_fulfill_order).await.unwrap();

        // Spawn fulfillment event that should be ignored
        tokio::spawn(async move {
            send_order_state_event(
                order_state_tx,
                OrderStateChange::Fulfilled { request_id: lock_and_fulfill_order.request.id },
            )
            .await
        });

        proving_service_with_fulfillment.prove_and_update_db(lock_and_fulfill_order.clone()).await;

        let final_order = db.get_order(&lock_and_fulfill_order.id()).await.unwrap().unwrap();
        assert_eq!(final_order.status, OrderStatus::PendingAgg);
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

        let (order_state_tx, _) = tokio::sync::broadcast::channel(100);
        let proving_service =
            ProvingService::new(db.clone(), prover, config.clone(), order_state_tx).await.unwrap();

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

    #[tokio::test]
    #[traced_test]
    async fn test_fulfillment_event_cancellation() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        let (order_state_tx, _) = tokio::sync::broadcast::channel(100);
        let proving_service =
            ProvingService::new(db.clone(), prover.clone(), config.clone(), order_state_tx.clone())
                .await
                .unwrap();

        let request_id = U256::from(123);
        let proof_id = prover.prove_stark(&image_id, &input_id, vec![]).await.unwrap();

        // Test 1: FulfillAfterLockExpire order cancelled by matching fulfillment event
        let order = create_test_order(
            request_id,
            image_id.clone(),
            input_id.clone(),
            Some(proof_id.clone()),
            FulfillmentType::FulfillAfterLockExpire,
            OrderStatus::Proving,
        );

        db.add_order(&order).await.unwrap();

        let proving_service_clone = proving_service.clone();
        let order_clone = order.clone();
        let monitor_task = tokio::spawn(async move {
            proving_service_clone.monitor_proof_with_timeout(order_clone).await
        });

        // Send fulfillment event for the same request - should cancel proof
        send_order_state_event(order_state_tx.clone(), OrderStateChange::Fulfilled { request_id })
            .await;

        let result = monitor_task.await.unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Request fulfilled by another prover"));

        // Test 2: FulfillAfterLockExpire order ignores different request ID
        let request_id_2 = U256::from(456);
        let different_fulfillment_id = U256::from(999);
        let proof_id_2 = prover.prove_stark(&image_id, &input_id, vec![]).await.unwrap();

        let order_2 = create_test_order(
            request_id_2,
            image_id,
            input_id,
            Some(proof_id_2.clone()),
            FulfillmentType::FulfillAfterLockExpire,
            OrderStatus::Proving,
        );

        db.add_order(&order_2).await.unwrap();

        let proving_service_clone_2 = proving_service.clone();
        let order_clone_2 = order_2.clone();
        let monitor_task_2 = tokio::spawn(async move {
            proving_service_clone_2.monitor_proof_with_timeout(order_clone_2).await
        });

        // Send fulfillment event for different request ID - should be ignored
        send_order_state_event(
            order_state_tx,
            OrderStateChange::Fulfilled { request_id: different_fulfillment_id },
        )
        .await;

        let result_2 = monitor_task_2.await.unwrap();
        assert!(result_2.is_ok());
        assert_eq!(result_2.unwrap(), OrderStatus::PendingAgg);

        assert!(logs_contain("was fulfilled by another prover"));
    }
}
