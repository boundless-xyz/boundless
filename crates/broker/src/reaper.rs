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

use std::time::Duration;

use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use boundless_market::telemetry::CompletionOutcome;

use tokio::sync::mpsc;

use crate::{
    coded_error_impl,
    config::{ConfigErr, ConfigLock},
    db::{DbError, DbObj},
    errors::{cancel_proof_and_fail, BrokerFailure, CodedError},
    order_committer::CommitmentComplete,
    provers::ProverObj,
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
    prover: ProverObj,
    chain_id: u64,
    /// Sends ProvingFailed to the OrderCommitter to free the capacity slot for expired orders.
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl ReaperTask {
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Self {
        Self { db, config, prover, chain_id, proving_completion_tx }
    }

    async fn reap_expired(&self) -> Result<(), ReaperError> {
        let (committed_grace, skipped_grace, batch_size, max_batches) = {
            let cfg = self.config.lock_all()?;
            (
                cfg.prover.reaper_grace_period_secs,
                cfg.prover.skipped_retention_grace_secs,
                cfg.prover.skipped_cleanup_batch_size,
                cfg.prover.skipped_cleanup_max_batches_per_pass,
            )
        };

        let expired_orders = self.db.get_expired_committed_orders(committed_grace.into()).await?;

        if !expired_orders.is_empty() {
            info!("[B-REAP-100] Found {} expired committed orders", expired_orders.len());
            for order in expired_orders {
                let order_id = order.id();
                debug!("Setting expired order {} to failed", order_id);
                cancel_proof_and_fail(
                    &self.prover,
                    &self.db,
                    &self.config,
                    &order,
                    &BrokerFailure::new(
                        "[B-REAP-003]",
                        "Order expired in reaper",
                        CompletionOutcome::ExpiredWhileProving,
                    ),
                    self.chain_id,
                    &self.proving_completion_tx,
                )
                .await;
            }
        }

        if batch_size == 0 {
            tracing::debug!("batch_size is 0, skipping expired orders cleanup");
            return Ok(());
        }

        let mut total_deleted: u64 = 0;
        for _ in 0..max_batches {
            let n = self.db.cleanup_expired_skipped(skipped_grace, batch_size).await?;
            total_deleted += n;
            if n < batch_size as u64 {
                break;
            }
            // Yield so other queries can acquire the single-writer connection
            // between batches.
            tokio::task::yield_now().await;
        }
        if total_deleted > 0 {
            info!(
                "[B-REAP-101] Deleted {} expired Skipped orders (max_batches={}, batch_size={})",
                total_deleted, max_batches, batch_size
            );
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

            if let Err(err) = self.reap_expired().await {
                warn!("Error in reaper pass: {}", err);
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
        db::{BrokerDb, SqliteDb},
        now_timestamp,
        provers::DefaultProver,
        FulfillmentType, Order, OrderStatus,
    };
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use chrono::Utc;
    use risc0_zkvm::sha::Digest;
    use std::sync::Arc;
    use tracing_test::traced_test;

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

    #[tokio::test]
    #[traced_test]
    async fn test_check_expired_orders_no_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, proving_completion_tx);

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
        reaper.reap_expired().await.unwrap();

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
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, proving_completion_tx);

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

        reaper.reap_expired().await.unwrap();

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
    async fn test_reaper_pass_b_deletes_expired_skipped() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut w = config.load_write().unwrap();
            w.prover.reaper_grace_period_secs = 30;
            w.prover.skipped_retention_grace_secs = 0;
            w.prover.skipped_cleanup_batch_size = 1000;
            w.prover.skipped_cleanup_max_batches_per_pass = 100;
        }
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (tx, _rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, tx);

        let now = now_timestamp();
        let past = now - 1000;
        let future = now + 1000;

        // Pass A target: committed + expired → marked Failed (existing behavior)
        let committed_expired =
            create_order_with_status_and_expiration(1, OrderStatus::PendingProving, Some(past));
        db.add_order(&committed_expired).await.unwrap();

        // Pass B targets: skipped + expired → deleted
        let skipped_expired_a =
            create_order_with_status_and_expiration(2, OrderStatus::Skipped, Some(past));
        let skipped_expired_b =
            create_order_with_status_and_expiration(3, OrderStatus::Skipped, Some(past));
        db.add_order(&skipped_expired_a).await.unwrap();
        db.add_order(&skipped_expired_b).await.unwrap();

        // Should remain
        let skipped_live =
            create_order_with_status_and_expiration(4, OrderStatus::Skipped, Some(future));
        db.add_order(&skipped_live).await.unwrap();

        // Pass A + Pass B in a single reaper invocation
        reaper.reap_expired().await.unwrap();

        // Pass A effect
        let after_committed = db.get_order(&committed_expired.id()).await.unwrap().unwrap();
        assert_eq!(after_committed.status, OrderStatus::Failed);

        // Pass B effects
        assert!(db.get_order(&skipped_expired_a.id()).await.unwrap().is_none());
        assert!(db.get_order(&skipped_expired_b.id()).await.unwrap().is_none());
        assert!(db.get_order(&skipped_live.id()).await.unwrap().is_some());
    }

    #[tokio::test]
    #[traced_test]
    async fn test_reaper_pass_b_respects_max_batches_per_pass() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut w = config.load_write().unwrap();
            w.prover.skipped_retention_grace_secs = 0;
            w.prover.skipped_cleanup_batch_size = 5;
            w.prover.skipped_cleanup_max_batches_per_pass = 2;
        }
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (tx, _rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, tx);

        let now = now_timestamp();
        let past = now - 1000;
        for i in 0..25u64 {
            let o =
                create_order_with_status_and_expiration(100 + i, OrderStatus::Skipped, Some(past));
            db.add_order(&o).await.unwrap();
        }

        // First pass: batch 5 × max 2 = 10 deleted, 15 remain
        reaper.reap_expired().await.unwrap();
        let remaining = count_skipped(&*db).await;
        assert_eq!(remaining, 15);

        // Second pass: 10 more
        reaper.reap_expired().await.unwrap();
        assert_eq!(count_skipped(&*db).await, 5);

        // Third pass: last 5
        reaper.reap_expired().await.unwrap();
        assert_eq!(count_skipped(&*db).await, 0);
    }

    #[tokio::test]
    #[traced_test]
    async fn test_reaper_pass_b_skipped_when_batch_size_zero() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut w = config.load_write().unwrap();
            w.prover.skipped_retention_grace_secs = 0;
            w.prover.skipped_cleanup_batch_size = 0;
            w.prover.skipped_cleanup_max_batches_per_pass = 100;
        }
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (tx, _rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, tx);

        let past = now_timestamp() - 1000;
        let o = create_order_with_status_and_expiration(200, OrderStatus::Skipped, Some(past));
        db.add_order(&o).await.unwrap();

        reaper.reap_expired().await.unwrap();

        // Row must remain — cleanup is disabled.
        assert!(db.get_order(&o.id()).await.unwrap().is_some());
    }

    async fn count_skipped(db: &dyn BrokerDb) -> usize {
        // We control IDs 100..125 above. Iterate and count those that exist.
        let mut n = 0;
        for i in 100..125u64 {
            let order = create_order_with_status_and_expiration(i, OrderStatus::Skipped, Some(0));
            if db.get_order(&order.id()).await.unwrap().is_some() {
                n += 1;
            }
        }
        n
    }

    #[tokio::test]
    #[traced_test]
    async fn test_check_expired_orders_all_committed_statuses() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        config.load_write().unwrap().prover.reaper_grace_period_secs = 30;
        let prover: ProverObj = Arc::new(DefaultProver::new());
        let (proving_completion_tx, _proving_completion_rx) = mpsc::channel(100);
        let reaper = ReaperTask::new(db.clone(), config, prover, 1, proving_completion_tx);

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

        reaper.reap_expired().await.unwrap();

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
