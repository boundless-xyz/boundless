// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use async_trait::async_trait;
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::{
    config::{ConfigErr, ConfigLock},
    db::{DbError, DbObj},
    errors::CodedError,
    task::{RetryRes, RetryTask, SupervisorErr},
};

#[derive(Error, Debug)]
pub enum ReaperError {
    #[error("{code} DB error: {0}", code = self.code())]
    DbError(#[from] DbError),

    #[error("{code} Config error {0}", code = self.code())]
    ConfigReadErr(#[from] ConfigErr),

    #[error("{code} Failed to update expired order status: {0}", code = self.code())]
    UpdateFailed(anyhow::Error),
}

impl CodedError for ReaperError {
    fn code(&self) -> &str {
        match self {
            ReaperError::DbError(_) => "[B-REAP-001]",
            ReaperError::ConfigReadErr(_) => "[B-REAP-002]",
            ReaperError::UpdateFailed(_) => "[B-REAP-003]",
        }
    }
}

#[derive(Clone)]
pub struct ReaperTask {
    db: DbObj,
    config: ConfigLock,
}

impl ReaperTask {
    pub fn new(db: DbObj, config: ConfigLock) -> Self {
        Self { db, config }
    }

    async fn check_expired_orders(&self) -> Result<(), ReaperError> {
        let grace_period = {
            let config = self.config.lock_all()?;
            config.prover.reaper_grace_period_secs
        };

        let expired_orders = self.db.get_expired_committed_orders(grace_period.into()).await?;

        if !expired_orders.is_empty() {
            info!("Found {} expired committed orders", expired_orders.len());

            for order in expired_orders {
                let order_id = order.id();
                debug!("Setting expired order {} to failed", order_id);

                match self.db.set_order_failure(&order_id, "Order expired").await {
                    Ok(()) => {
                        warn!("Order {} has expired, marked as failed", order_id);
                    }
                    Err(err) => {
                        error!("Failed to update status for expired order {}: {}", order_id, err);
                        return Err(ReaperError::UpdateFailed(err.into()));
                    }
                }
            }
        }

        Ok(())
    }

    async fn run_reaper_loop(&self) -> Result<(), ReaperError> {
        let interval = {
            let config = self.config.lock_all()?;
            config.prover.reaper_interval_secs
        };

        loop {
            if let Err(err) = self.check_expired_orders().await {
                warn!("Error checking expired orders: {}", err);
            }

            tokio::time::sleep(Duration::from_secs(interval.into())).await;
        }
    }
}

#[async_trait]
impl RetryTask for ReaperTask {
    type Error = ReaperError;

    fn spawn(&self) -> RetryRes<Self::Error> {
        let this = self.clone();
        Box::pin(async move {
            match this.run_reaper_loop().await {
                Ok(_) => Ok(()),
                Err(err) => Err(SupervisorErr::Recover(err)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::SqliteDb, now_timestamp, FulfillmentType, Order, OrderStatus};
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Offer, Predicate, PredicateType, ProofRequest, RequestId, RequestInput, RequestInputType,
        Requirements,
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
                Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                "http://risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    biddingStart: 0,
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
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
            proving_started_at: None,
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn test_check_expired_orders_no_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let reaper = ReaperTask::new(db.clone(), config);

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
    async fn test_check_expired_orders_with_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let reaper = ReaperTask::new(db.clone(), config);

        let current_time = now_timestamp();
        let past_time = current_time - 100;
        let future_time = current_time + 100;

        let expired_order1 = create_order_with_status_and_expiration(
            1,
            OrderStatus::PendingProving,
            Some(past_time),
        );
        let expired_order2 =
            create_order_with_status_and_expiration(2, OrderStatus::Aggregating, Some(past_time));
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
        assert_eq!(stored_expired1.error_msg, Some("Order expired".to_string()));
        assert_eq!(stored_expired2.status, OrderStatus::Failed);
        assert_eq!(stored_expired2.error_msg, Some("Order expired".to_string()));

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
    async fn test_check_expired_orders_all_committed_statuses() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let reaper = ReaperTask::new(db.clone(), config);

        let current_time = now_timestamp();
        let past_time = current_time - 100;

        // Test all committed statuses that should be checked for expiration
        let statuses = [
            OrderStatus::PendingProving,
            OrderStatus::Proving,
            OrderStatus::PendingAgg,
            OrderStatus::Aggregating,
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
            assert_eq!(stored_order.error_msg, Some("Order expired".to_string()));
        }
    }
}
