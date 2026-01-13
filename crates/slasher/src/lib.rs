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

use std::sync::Arc;

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{Address, B256, U256},
    providers::{
        fillers::{ChainIdFiller, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
    transports::{RpcError, TransportErrorKind},
};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer, BalanceAlertProvider},
    contracts::boundless_market::{BoundlessMarketService, MarketError},
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
};
use db::{DbError, DbObj, PgDb};
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

pub mod db;

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

    #[error("Slash reverted for request 0x{0:x}, tx_hash: {1:?}")]
    SlashRevert(U256, B256),
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
    pub max_block_range: u64,
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

        let dynamic_gas_filler = DynamicGasFiller::new(
            20,
            boundless_market::dynamic_gas_filler::PriorityMode::Medium,
            signer_address,
        );
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

        let db: DbObj = Arc::new(PgDb::new(db_conn).await.map_err(ServiceError::DatabaseError)?);

        Ok(Self { boundless_market, db, config })
    }
}

impl<P> SlashService<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub async fn run(
        self,
        starting_block: Option<u64>,
        ignore_last_processed_block: Option<bool>,
    ) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);
        let current_block = self.current_block().await?;
        let last_processed_block = self.get_last_processed_block().await?;
        tracing::debug!("Last processed block fetched from db: {:?}", last_processed_block);

        // Determine the starting block to process from
        // If ignore_last_processed_block is true, use the provided starting block
        // Otherwise, use last_processed_block, if present in the db.
        // If no last_prcessed_block (e.g. first run), use starting block
        let mut from_block: u64 = match (
            ignore_last_processed_block,
            starting_block,
            last_processed_block,
        ) {
            (Some(true), Some(starting_block), _) => {
                tracing::debug!("Ignoring last processed block and using provided starting block {} for starting block", starting_block);
                starting_block
            }
            (_, _, Some(last_processed_block)) => {
                tracing::debug!(
                    "Using last processed block {} for starting block",
                    last_processed_block
                );
                last_processed_block
            }
            (_, Some(starting_block), None) => {
                tracing::debug!(
                    "No last processed block. Using provided starting block {} for starting block",
                    starting_block
                );
                starting_block
            }
            _ => {
                tracing::debug!("No last processed block and no starting block provided. Using current block {} for starting block", current_block);
                current_block
            }
        };

        tracing::info!("Starting from block {}", from_block);

        let mut attempt = 0;
        loop {
            interval.tick().await;

            match self.current_block().await {
                Ok(to_block) => {
                    if to_block < from_block {
                        continue;
                    }

                    // Cap the processing range to max_block_range
                    let chunk_to =
                        std::cmp::min(from_block + self.config.max_block_range - 1, to_block);

                    if chunk_to < to_block {
                        tracing::info!(
                            "Processing blocks from {} to {} (chunked, current block: {})",
                            from_block,
                            chunk_to,
                            to_block,
                        );
                    } else {
                        tracing::info!("Processing blocks from {} to {}", from_block, chunk_to);
                    }

                    match self.process_blocks(from_block, chunk_to).await {
                        Ok(_) => {
                            attempt = 0;
                            from_block = chunk_to + 1;
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
                                    chunk_to,
                                    e
                                );
                                return Err(e);
                            }
                            // Recoverable errors
                            ServiceError::BoundlessMarketError(_)
                            | ServiceError::SlashRevert(_, _)
                            | ServiceError::EventQueryError(_)
                            | ServiceError::RpcError(_)
                            | ServiceError::BlockTimestampNotFound(_) => {
                                attempt += 1;
                                tracing::warn!(
                                    "Failed to process blocks from {} to {}: {:?}, attempt number {}",
                                    from_block,
                                    chunk_to,
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
        self.db.set_last_block(block_number).await?;
        tracing::info!("Updated last processed block to {block_number}");
        Ok(())
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

        for (event, log_data) in logs {
            let prover = event.prover;

            // Skip if sender is in the skip list
            if self.config.skip_addresses.contains(&prover) {
                tracing::info!(
                    "Skipping locked event from prover: {:?} for request: 0x{:x}",
                    prover,
                    event.requestId
                );
                continue;
            }

            tracing::debug!(
                "Processing locked event from prover: {:?} for request: 0x{:x} found at block {:?}",
                prover,
                event.requestId,
                log_data.block_number
            );

            let request = event.request.clone();
            let expires_at = request.expires_at();
            let lock_expires_at = request.offer.rampUpStart + request.offer.lockTimeout as u64;

            self.add_order(event.requestId, expires_at, lock_expires_at).await?;
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
                Err(MarketError::RequestIsSlashed(request_id)) => {
                    tracing::warn!("Request 0x{:x} is already slashed, removing", request_id);
                    self.remove_order(request_id).await?;
                }
                Err(MarketError::SlashRevert(tx_hash)) => {
                    // If already slashed should be caught by the error above, but double check here in case race condition
                    // caused the previous call to miss the slashing.
                    let slashed = self.boundless_market.is_slashed(request_id).await?;
                    if slashed {
                        tracing::warn!("Tx 0x{:x} reverted when slashing request 0x{:x}. Request is already slashed, removing", tx_hash, request_id);
                        self.remove_order(request_id).await?;
                    } else {
                        // Only warn as we've seen eventual consistency issues where the request actually was slashed.
                        // Logic will retry and should succeed in this case. If retrys fail, it will error out.
                        tracing::warn!("Tx 0x{:x} for request 0x{:x} reverted and request is not slashed already", tx_hash, request_id);
                        return Err(ServiceError::SlashRevert(request_id, tx_hash));
                    }
                }
                Err(MarketError::LogNotEmitted(tx_hash, err)) => {
                    let slashed = self.boundless_market.is_slashed(request_id).await?;
                    if slashed {
                        tracing::warn!("Tx 0x{:x} did not emit expected Slashed event for request 0x{:x} [{}]. Request is already slashed, removing", tx_hash, request_id, err);
                        self.remove_order(request_id).await?;
                    } else {
                        tracing::warn!("Tx 0x{:x} for request 0x{:x} did not emit expected Slashed event [{}]. Request is not slashed already", tx_hash, request_id, err);
                        return Err(ServiceError::SlashRevert(request_id, tx_hash));
                    }
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
                    } else if err_msg.contains("RequestIsNotLocked") {
                        tracing::error!(
                            "Request 0x{:x} was marked for slashing but was not locked. Removing.",
                            request_id
                        );
                        self.remove_order(request_id).await?;
                    } else {
                        // Any other error should be RPC related so we can retry
                        // Only warn as logic will retry. If retrys fail, it will error out.
                        tracing::warn!("Failed to slash request 0x{:x}: {}", request_id, err);
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
