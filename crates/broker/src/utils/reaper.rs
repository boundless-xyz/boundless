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

use std::{sync::Arc, time::Duration};

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use boundless_market::telemetry::CompletionOutcome;

use tokio::sync::mpsc;

use crate::{
    backend::BackendRouter,
    coded_error_impl,
    config::{ConfigErr, ConfigLock},
    db::{DbError, DbObj},
    errors::{handle_order_failure, BrokerFailure, CodedError},
    order_committer::CommitmentComplete,
    task::{BrokerService, SupervisorErr},
};

#[derive(Error)]
pub enum ReaperError {
    #[error("{code} DB error: {0}", code = self.code())]
    DbError(#[from] DbError),

    #[error("{code} Config error {0}", code = self.code())]
    ConfigReadErr(#[from] ConfigErr),
}

coded_error_impl!(ReaperError, "REAP",
    DbError(..)       => "001",
    ConfigReadErr(..) => "002",
);

#[derive(Clone)]
pub struct ReaperTask {
    db: DbObj,
    config: ConfigLock,
    backend: Arc<BackendRouter>,
    chain_id: u64,
    /// Sends ProvingFailed to the OrderCommitter to free the capacity slot for expired orders.
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl ReaperTask {
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        backend: Arc<BackendRouter>,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Self {
        Self { db, config, backend, chain_id, proving_completion_tx }
    }

    async fn check_expired_orders(&self) -> Result<(), ReaperError> {
        let grace_period = {
            let config = self.config.lock_all()?;
            config.prover.reaper_grace_period_secs
        };

        let expired_orders = self.db.get_expired_committed_orders(grace_period.into()).await?;

        if !expired_orders.is_empty() {
            info!("[B-REAP-100] Found {} expired committed orders", expired_orders.len());

            for order in expired_orders {
                let order_id = order.id();
                debug!("Setting expired order {} to failed", order_id);

                let failure = BrokerFailure::new(
                    "[B-REAP-003]",
                    "Order expired in reaper",
                    CompletionOutcome::ExpiredWhileProving,
                );

                handle_order_failure(
                    &self.db,
                    &order_id,
                    &failure,
                    self.chain_id,
                    &self.proving_completion_tx,
                )
                .await;

                let should_cancel = {
                    let config = self.config.lock_all()?;
                    config.market.cancel_proving_expired_orders
                };
                if should_cancel {
                    if let Err(err) = self.backend.cancel_order(&order).await {
                        warn!(
                            "[B-REAP-004] Failed to cancel backend processing for expired order {order_id}: {err:?}"
                        );
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_reaper_loop(&self, cancel_token: CancellationToken) -> Result<(), ReaperError> {
        let interval = {
            let config = self.config.lock_all()?;
            config.prover.reaper_interval_secs
        };

        loop {
            // Wait to run the reaper on startup to allow other tasks to start.
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval.into())) => {},
                _ = cancel_token.cancelled() => {
                    tracing::info!("Reaper task received cancellation, shutting down gracefully");
                    return Ok(());
                }
            }

            if let Err(err) = self.check_expired_orders().await {
                warn!("Error checking expired orders: {}", err);
            }
        }
    }
}

impl BrokerService for ReaperTask {
    type Error = ReaperError;

    async fn run(self, cancel_token: CancellationToken) -> Result<(), SupervisorErr<Self::Error>> {
        self.run_reaper_loop(cancel_token).await.map_err(SupervisorErr::Recover)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{
            Backend, BackendEntry, BatchProcessorObj, FulfillmentBatch, OrderProcessProgress,
            ProcessOrder, SubmissionPlan, VerifierUpdate,
        },
        db::SqliteDb,
        now_timestamp, FulfillmentType, Order, OrderStatus,
    };
    use alloy::primitives::{Address, Bytes, FixedBytes, U256};
    use async_trait::async_trait;
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use boundless_market::selector::ProofType;
    use chrono::Utc;
    use risc0_zkvm::sha::Digest;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tracing_test::traced_test;

    struct CancelBackend {
        id: crate::backend::BackendId,
        selectors: Vec<FixedBytes<4>>,
        cancel_calls: AtomicUsize,
    }

    impl CancelBackend {
        fn new(selector: FixedBytes<4>) -> Self {
            Self {
                id: crate::backend::BackendId::new("cancel_backend").unwrap(),
                selectors: vec![selector],
                cancel_calls: AtomicUsize::new(0),
            }
        }

        fn cancel_calls(&self) -> usize {
            self.cancel_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl Backend for CancelBackend {
        fn id(&self) -> &crate::backend::BackendId {
            &self.id
        }

        fn supported_selectors(&self) -> Vec<FixedBytes<4>> {
            self.selectors.clone()
        }

        fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType> {
            self.selectors.contains(&selector).then_some(ProofType::Any)
        }

        async fn evaluate_request(
            &self,
            _request: boundless_market::prover_utils::EvaluationRequest,
            _limits: boundless_market::prover_utils::EvaluationLimits,
        ) -> Result<
            boundless_market::prover_utils::RequestEvaluation,
            boundless_market::prover_utils::OrderPricingError,
        > {
            Err(anyhow::anyhow!("cancel backend does not evaluate requests").into())
        }

        async fn process_order(&self, _cmd: ProcessOrder) -> anyhow::Result<OrderProcessProgress> {
            anyhow::bail!("cancel backend does not process orders")
        }

        async fn cancel_order(&self, _order: &Order) -> anyhow::Result<()> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn batch_processor(&self) -> Option<BatchProcessorObj> {
            None
        }

        async fn build_fulfillments(
            &self,
            _cmd: FulfillmentBatch,
        ) -> anyhow::Result<SubmissionPlan> {
            anyhow::bail!("cancel backend does not build fulfillments")
        }

        async fn verifier_update_applied(&self, _update: &VerifierUpdate) -> anyhow::Result<bool> {
            anyhow::bail!("cancel backend does not query verifier updates")
        }

        async fn apply_verifier_update(&self, _update: &VerifierUpdate) -> anyhow::Result<()> {
            anyhow::bail!("cancel backend does not apply verifier updates")
        }
    }

    fn create_order_with_status_and_expiration(
        id: u64,
        status: OrderStatus,
        expire_timestamp: Option<u64>,
    ) -> Order {
        Order {
            status,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: ProofRequest::new(
                RequestId::new(Address::ZERO, id as u32),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "http://risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    rampUpStart: 0,
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockCollateral: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            backend_id: None,
            expire_timestamp,
            client_sig: Bytes::new(),
            lock_price: Some(U256::from(1)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            cached_id: Default::default(),
        }
    }

    fn test_backend_router() -> Arc<BackendRouter> {
        Arc::new(BackendRouter::new())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_check_expired_orders_no_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper =
            ReaperTask::new(db.clone(), config, test_backend_router(), 1, proving_completion_tx);

        let current_time = now_timestamp();
        let future_time = current_time + 100;

        // Add non-expired orders
        let order1 = create_order_with_status_and_expiration(
            1,
            OrderStatus::PendingProving,
            Some(future_time),
        );
        let order2 =
            create_order_with_status_and_expiration(2, OrderStatus::Proving, Some(future_time));

        db.add_order(&order1).await.unwrap();
        db.add_order(&order2).await.unwrap();

        // Should not fail and should not mark any orders as failed
        reaper.check_expired_orders().await.unwrap();

        let stored_order1 = db.get_order(&order1.id()).await.unwrap().unwrap();
        let stored_order2 = db.get_order(&order2.id()).await.unwrap().unwrap();

        assert_eq!(stored_order1.status, OrderStatus::PendingProving);
        assert_eq!(stored_order2.status, OrderStatus::Proving);
        assert!(stored_order1.error_msg.is_none());
        assert!(stored_order2.error_msg.is_none());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_expired_orders() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        config.load_write().unwrap().prover.reaper_grace_period_secs = 30;
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper =
            ReaperTask::new(db.clone(), config, test_backend_router(), 1, proving_completion_tx);

        let current_time = now_timestamp();
        let past_time = current_time - 100;
        let future_time = current_time + 100;

        let expired_order1 = create_order_with_status_and_expiration(
            1,
            OrderStatus::PendingProving,
            Some(past_time),
        );
        let expired_order2 =
            create_order_with_status_and_expiration(2, OrderStatus::PendingAgg, Some(past_time));
        let active_order =
            create_order_with_status_and_expiration(3, OrderStatus::Proving, Some(future_time));
        let done_order =
            create_order_with_status_and_expiration(5, OrderStatus::Done, Some(past_time));

        db.add_order(&expired_order1).await.unwrap();
        db.add_order(&expired_order2).await.unwrap();
        db.add_order(&active_order).await.unwrap();
        db.add_order(&done_order).await.unwrap();

        reaper.check_expired_orders().await.unwrap();

        // Check expired orders are marked as failed
        let stored_expired1 = db.get_order(&expired_order1.id()).await.unwrap().unwrap();
        let stored_expired2 = db.get_order(&expired_order2.id()).await.unwrap().unwrap();

        assert_eq!(stored_expired1.status, OrderStatus::Failed);
        assert_eq!(
            stored_expired1.error_msg,
            Some("[B-REAP-003] Order expired in reaper".to_string())
        );
        assert_eq!(stored_expired2.status, OrderStatus::Failed);
        assert_eq!(
            stored_expired2.error_msg,
            Some("[B-REAP-003] Order expired in reaper".to_string())
        );

        // Check non-expired orders remain unchanged
        let stored_active = db.get_order(&active_order.id()).await.unwrap().unwrap();
        let stored_done = db.get_order(&done_order.id()).await.unwrap().unwrap();

        assert_eq!(stored_active.status, OrderStatus::Proving);
        assert!(stored_active.error_msg.is_none());
        assert_eq!(stored_done.status, OrderStatus::Done);
        assert!(stored_done.error_msg.is_none());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_expired_order_cancels_registered_backend() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.prover.reaper_grace_period_secs = 30;
            config.market.cancel_proving_expired_orders = true;
        }
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);

        let current_time = now_timestamp();
        let past_time = current_time - 100;
        let order =
            create_order_with_status_and_expiration(1, OrderStatus::Proving, Some(past_time));
        let backend = Arc::new(CancelBackend::new(order.request.requirements.selector));
        let router = Arc::new(
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap(),
        );
        let reaper = ReaperTask::new(db.clone(), config, router, 1, proving_completion_tx);

        db.add_order(&order).await.unwrap();

        reaper.check_expired_orders().await.unwrap();

        assert_eq!(backend.cancel_calls(), 1);
        let stored_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(stored_order.status, OrderStatus::Failed);
        assert_eq!(
            stored_order.error_msg,
            Some("[B-REAP-003] Order expired in reaper".to_string())
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn test_check_expired_orders_all_committed_statuses() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        config.load_write().unwrap().prover.reaper_grace_period_secs = 30;
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper =
            ReaperTask::new(db.clone(), config, test_backend_router(), 1, proving_completion_tx);

        let current_time = now_timestamp();
        let past_time = current_time - 100;

        // Test all committed statuses that should be checked for expiration
        let statuses = [
            OrderStatus::PendingProving,
            OrderStatus::Proving,
            OrderStatus::PendingAgg,
            OrderStatus::SkipAggregation,
            OrderStatus::PendingSubmission,
        ];

        let mut orders = Vec::new();
        for (i, status) in statuses.iter().enumerate() {
            let order = create_order_with_status_and_expiration(i as u64, *status, Some(past_time));
            db.add_order(&order).await.unwrap();
            orders.push(order);
        }

        reaper.check_expired_orders().await.unwrap();

        // All orders should be marked as failed
        for order in orders {
            let stored_order = db.get_order(&order.id()).await.unwrap().unwrap();
            assert_eq!(stored_order.status, OrderStatus::Failed);
            assert_eq!(
                stored_order.error_msg,
                Some("[B-REAP-003] Order expired in reaper".to_string())
            );
        }
    }
}
