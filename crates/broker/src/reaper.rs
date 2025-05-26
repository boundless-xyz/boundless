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

pub struct ReaperTask {
    db: DbObj,
    config: ConfigLock,
}

impl ReaperTask {
    pub fn new(db: DbObj, config: ConfigLock) -> Self {
        Self { db, config }
    }

    async fn check_expired_orders(&self) -> Result<(), ReaperError> {
        let expired_orders = self.db.get_expired_committed_orders().await?;

        if !expired_orders.is_empty() {
            info!("Found {} expired committed orders", expired_orders.len());

            for order in expired_orders {
                let order_id = order.id();
                debug!("Setting expired order {} to failed", order_id);

                match self.db.set_order_failure(&order_id, "Order expired").await {
                    Ok(_) => {
                        info!("Successfully marked order {} as failed due to expiration", order_id)
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
            config.prover.reaper_interval
        };

        loop {
            if let Err(err) = self.check_expired_orders().await {
                warn!("Error checking expired orders: {}", err);
                // Continue the loop even on error
            }

            tokio::time::sleep(Duration::from_secs(interval)).await;
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

impl Clone for ReaperTask {
    fn clone(&self) -> Self {
        Self { db: self.db.clone(), config: self.config.clone() }
    }
}
