// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{cmp::min, sync::Arc};

use alloy::{
    consensus::Transaction,
    network::{Ethereum, EthereumWallet, TransactionResponse},
    primitives::{Address, U256},
    providers::{
        fillers::{ChainIdFiller, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
    sol_types::SolCall,
    transports::{RpcError, TransportErrorKind},
};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer, BalanceAlertProvider},
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        IBoundlessMarket::{self},
    },
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
use db::{DbError, DbObj, SqliteDb};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

mod db;

type ProviderWallet = NonceProvider<
    JoinFill<JoinFill<Identity, ChainIdFiller>, DynamicGasFiller>,
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

    #[error("Transaction decoding error: {0}")]
    TransactionDecodingError(#[from] alloy::sol_types::Error),

    #[error("Block number not found")]
    BlockNumberNotFound,

    #[error("BlockTimestamp not found for block: {0}")]
    BlockTimestampNotFound(u64),

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
    pub tx_timeout: Duration,
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

        let signer_address = wallet.default_signer().address();
        let balance_alerts_layer = BalanceAlertLayer::new(BalanceAlertConfig {
            watch_address: signer_address,
            warn_threshold: config.balance_warn_threshold,
            error_threshold: config.balance_error_threshold,
        });

        let dynamic_gas_filler = DynamicGasFiller::new(0.2, 0.05, 2.0, signer_address);
        let base_provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .filler(dynamic_gas_filler)
            .layer(balance_alerts_layer)
            .connect_http(rpc_url);
        let provider = NonceProvider::new(base_provider, wallet.clone());

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller)
                .with_timeout(config.tx_timeout);

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
                            | ServiceError::TransactionDecodingError(_)
                            | ServiceError::BlockNumberNotFound
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
                            | ServiceError::RpcError(_)
                            | ServiceError::BlockTimestampNotFound(_) => {
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

            let sender = tx.from();

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
                "Processing locked event from sender: {:?} for request: 0x{:x} found at block {}",
                sender,
                log.requestId,
                log_data.block_number.unwrap_or(0)
            );

            let request = IBoundlessMarket::lockRequestCall::abi_decode(tx.input())?.request;
            let expires_at = request.expires_at();
            let lock_expires_at = request.offer.biddingStart + request.offer.lockTimeout as u64;

            self.add_order(log.requestId, expires_at, lock_expires_at).await?;
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

        for (log, log_data) in logs {
            tracing::debug!(
                "Processing slashed event for request: 0x{:x} found at block {}",
                log.requestId,
                log_data.block_number.unwrap_or(0)
            );
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

        for (log, log_data) in logs {
            tracing::debug!(
                "Processing fulfilled event for request: 0x{:x} found at block {}",
                log.requestId,
                log_data.block_number.unwrap_or(0)
            );
            let current_ts = if let Some(current_ts) = log_data.block_timestamp {
                current_ts
            } else {
                let bn = log_data.block_number.ok_or(ServiceError::BlockNumberNotFound)?;
                self.block_timestamp(bn).await?
            };
            let (_, lock_expires_at) = match self.db.get_order(log.requestId).await? {
                Some(order_data) => order_data,
                None => {
                    tracing::warn!(
                        "Order not found in database for fulfilled request: 0x{:x}, skipping",
                        log.requestId
                    );
                    continue;
                }
            };
            if current_ts <= lock_expires_at {
                tracing::debug!(
                    "Request was fulfilled before lock expired. Removing from db: 0x{:x}",
                    log.requestId
                );
                self.remove_order(log.requestId).await?;
            } else {
                tracing::debug!(
                    "Request was fulfilled after lock expired. Not removing from db: 0x{:x}",
                    log.requestId
                );
            }
        }

        Ok(())
    }

    // Insert request into database
    async fn add_order(
        &self,
        request_id: U256,
        expires_at: u64,
        lock_expires_at: u64,
    ) -> Result<(), ServiceError> {
        tracing::debug!("Adding new request: 0x{:x} expiring at {}", request_id, expires_at);
        Ok(self.db.add_order(request_id, expires_at, lock_expires_at).await?)
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
            tracing::debug!("About to slash expired request: 0x{:x}", request_id);
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
                            "Request was either fulfilled before lock expiry, or has already been slashed, removing 0x{:x}, reason: {}",
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
                        tracing::error!("Failed to slash request 0x{:x}: {}", request_id, err);
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
            .get_block_by_number(block_number.into())
            .await?
            .ok_or_else(|| ServiceError::BlockTimestampNotFound(block_number))?
            .header
            .timestamp)
    }
}
