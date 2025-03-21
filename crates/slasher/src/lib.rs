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

use std::{cmp::min, sync::Arc};

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{Address, U256},
    providers::{
        fillers::{
            BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            WalletFiller,
        },
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::types::BlockTransactionsKind,
    signers::local::PrivateKeySigner,
    transports::{RpcError, TransportErrorKind},
};
use balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer, BalanceAlertProvider};
use boundless_market::contracts::boundless_market::{BoundlessMarketService, MarketError};
use db::{DbError, DbObj, SqliteDb};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

mod db;

type ProviderWallet = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    BalanceAlertProvider<RootProvider>,
>;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DbError),

    #[error("Boundless market error: {0}")]
    BoundlessMarketError(#[from] MarketError),

    #[error("RPC error: {0}")]
    RpcError(#[from] RpcError<TransportErrorKind>),

    #[error("Event query error: {0}")]
    EventQueryError(#[from] alloy::contract::Error),

    #[error("Insufficient funds: {0}")]
    InsufficientFunds(String),

    #[error("Maximum retries reached")]
    MaxRetries,

    #[error("Request not expired")]
    RequestNotExpired,
}

#[derive(Clone)]
pub struct SlashService<P> {
    pub boundless_market: BoundlessMarketService<P>,
    pub db: DbObj,
    pub config: SlashServiceConfig,
}

#[derive(Clone)]
pub struct SlashServiceConfig {
    pub interval: Duration,
    pub retries: u32,
    pub balance_warn_threshold: Option<U256>,
    pub balance_error_threshold: Option<U256>,
    pub skip_addresses: Vec<Address>,
}

impl SlashService<ProviderWallet> {
    pub async fn new(
        rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: SlashServiceConfig,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let wallet = EthereumWallet::from(private_key.clone());

        let balance_alerts_layer = BalanceAlertLayer::new(BalanceAlertConfig {
            watch_address: wallet.default_signer().address(),
            warn_threshold: config.balance_warn_threshold,
            error_threshold: config.balance_error_threshold,
        });

        let provider = ProviderBuilder::new()
            .layer(balance_alerts_layer)
            .wallet(wallet.clone())
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);

        let db: DbObj = Arc::new(SqliteDb::new(db_conn).await.unwrap());

        Ok(Self { boundless_market, db, config })
    }
}

impl<P> SlashService<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub async fn run(self, starting_block: Option<u64>) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);
        let current_block = self.current_block().await?;
        let last_processed_block = self.get_last_processed_block().await?.unwrap_or(current_block);
        let mut from_block = min(starting_block.unwrap_or(last_processed_block), current_block);

        let mut attempt = 0;
        loop {
            interval.tick().await;

            match self.current_block().await {
                Ok(to_block) => {
                    if to_block < from_block {
                        continue;
                    }

                    tracing::info!("Processing blocks from {} to {}", from_block, to_block);

                    match self.process_blocks(from_block, to_block).await {
                        Ok(_) => {
                            attempt = 0;
                            from_block = to_block + 1;
                        }
                        Err(e) => match e {
                            // Irrecoverable errors
                            ServiceError::DatabaseError(_)
                            | ServiceError::InsufficientFunds(_)
                            | ServiceError::MaxRetries
                            | ServiceError::RequestNotExpired => {
                                tracing::error!(
                                    "Failed to process blocks from {} to {}: {:?}",
                                    from_block,
                                    to_block,
                                    e
                                );
                                return Err(e);
                            }
                            // Recoverable errors
                            ServiceError::BoundlessMarketError(_)
                            | ServiceError::EventQueryError(_)
                            | ServiceError::RpcError(_) => {
                                attempt += 1;
                                tracing::warn!(
                                    "Failed to process blocks from {} to {}: {:?}, attempt number {}",
                                    from_block,
                                    to_block,
                                    e,
                                    attempt
                                );
                            }
                        },
                    }
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!(
                        "Failed to fetch current block: {:?}, attempt number {}",
                        e,
                        attempt
                    );
                }
            }
            if attempt > self.config.retries {
                tracing::error!("Aborting after {} consecutive attempts", attempt);
                return Err(ServiceError::MaxRetries);
            }
        }
    }

    async fn process_blocks(&self, from: u64, to: u64) -> Result<(), ServiceError> {
        // First check for new locked in requests
        self.process_locked_events(from, to).await?;

        // Then check for fulfilled/slashed events
        self.process_fulfilled_events(from, to).await?;
        self.process_slashed_events(from, to).await?;

        // Run the slashing task for expired requests
        self.process_expired_requests(to).await?;

        // Update the last processed block
        self.update_last_processed_block(to).await?;

        Ok(())
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        Ok(self.db.set_last_block(block_number).await?)
    }

    async fn process_locked_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestLocked_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} locked events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, log_data) in logs {
            // TODO(willpote): Remove, or make more resilient.
            // Note this logic is not full proof. It will not handle lockRequestWithSignature
            // nor if the lockRequest calls were for example, made via a proxy contract.
            // This is a temporary solution to avoid slashing requests from the team's broker.
            let tx_hash = log_data.transaction_hash.unwrap();
            let tx = self
                .boundless_market
                .instance()
                .provider()
                .get_transaction_by_hash(tx_hash)
                .await?
                .unwrap();

            let sender = tx.from;

            // Skip if sender is in the skip list
            if self.config.skip_addresses.contains(&sender) {
                tracing::info!(
                    "Skipping locked event from sender: {:?} for request: 0x{:x}",
                    sender,
                    log.requestId
                );
                continue;
            }

            tracing::debug!(
                "Processing locked event from sender: {:?} for request: 0x{:x}",
                sender,
                log.requestId
            );

            self.add_order(log.requestId).await?;
        }

        Ok(())
    }

    async fn process_slashed_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .ProverSlashed_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} slashed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, _) in logs {
            self.remove_order(log.requestId).await?;
        }

        Ok(())
    }

    async fn process_fulfilled_events(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestFulfilled_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::info!(
            "Found {} fulfilled events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (log, _) in logs {
            self.remove_order(log.requestId).await?;
        }

        Ok(())
    }

    // Insert request into database
    async fn add_order(&self, request_id: U256) -> Result<(), ServiceError> {
        let expiration =
            self.boundless_market.instance().requestDeadline(request_id).call().await?._0;
        tracing::debug!(
            "Adding new request: 0x{:x} expiring at block_no {}",
            request_id,
            expiration
        );
        Ok(self.db.add_order(request_id, expiration).await?)
    }

    // Remove request from database
    async fn remove_order(&self, request_id: U256) -> Result<(), ServiceError> {
        tracing::debug!("Removing request: 0x{:x}", request_id);
        Ok(self.db.remove_order(request_id).await?)
    }

    async fn process_expired_requests(&self, current_block: u64) -> Result<(), ServiceError> {
        // Find expired requests
        let expired =
            self.db.get_expired_orders(self.block_timestamp(current_block).await?).await?;

        for request_id in expired {
            match self.boundless_market.slash(request_id).await {
                Ok(_) => {
                    tracing::info!("Slashing successful for request 0x{:x}", request_id);
                    self.remove_order(request_id).await?;
                }
                Err(err) => {
                    let err_msg = err.to_string();
                    if err_msg.contains("RequestIsSlashed")
                        || err_msg.contains("RequestIsFulfilled")
                    {
                        tracing::warn!(
                            "Request already processed, removing 0x{:x}, reason: {}",
                            request_id,
                            err_msg
                        );
                        self.remove_order(request_id).await?;
                    } else if err_msg.contains("RequestIsNotExpired") {
                        // This should not happen
                        tracing::error!("Request 0x{:x} is not expired yet", request_id);
                        return Err(ServiceError::RequestNotExpired);
                    } else if err_msg.contains("insufficient funds")
                        || err_msg.contains("gas required exceeds allowance")
                    {
                        tracing::error!(
                            "Insufficient funds for slashing request 0x{:x}",
                            request_id
                        );
                        // Return as this is irrecoverable
                        return Err(ServiceError::InsufficientFunds(err_msg));
                    } else {
                        // Any other error should be RPC related so we can retry
                        tracing::error!("Failed to slash request 0x{:x}", request_id);
                        return Err(ServiceError::BoundlessMarketError(err));
                    }
                }
            }
        }

        Ok(())
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        Ok(self.boundless_market.instance().provider().get_block_number().await?)
    }

    async fn block_timestamp(&self, block_number: u64) -> Result<u64, ServiceError> {
        Ok(self
            .boundless_market
            .instance()
            .provider()
            .get_block_by_number(block_number.into(), BlockTransactionsKind::Hashes)
            .await?
            .unwrap()
            .header
            .timestamp)
    }
}
