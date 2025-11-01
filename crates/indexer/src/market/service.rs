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

use std::{cmp::min, collections::HashMap, sync::Arc};

use crate::{
    db::{
        market::{AnyDb, HourlyMarketSummary},
        DbError, DbObj, TxMetadata,
    },
    market::pricing::{compute_percentiles, encode_percentiles_to_bytes, price_at_time},
};
use ::boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    EIP712DomainSaltless, RequestId,
};
use ::boundless_market::order_stream_client::OrderStreamClient;
use alloy::{
    eips::BlockNumberOrTag,
    network::{Ethereum, TransactionResponse},
    primitives::{Address, B256, U256},
    providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::types::Log,
    signers::local::PrivateKeySigner,
    transports::{RpcError, TransportErrorKind},
};
use std::str::FromStr;
use anyhow::{anyhow, Context};
use sqlx::Row;
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

const MAX_BATCH_SIZE: u64 = 500;

type ProviderWallet = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

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

    #[error("Error: {0}")]
    Error(#[from] anyhow::Error),

    #[error("Maximum retries reached")]
    MaxRetries,

    #[error("Request not expired")]
    RequestNotExpired,
}

#[derive(Clone)]
pub struct IndexerService<P> {
    pub boundless_market: BoundlessMarketService<P>,
    pub db: DbObj,
    pub domain: EIP712DomainSaltless,
    pub config: IndexerServiceConfig,
    // Mapping from transaction hash to TxMetadata
    pub cache: HashMap<B256, TxMetadata>,
    // Optional order stream client for fetching off-chain orders
    pub order_stream_client: Option<OrderStreamClient>,
}

#[derive(Clone)]
pub struct IndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
}

impl IndexerService<ProviderWallet> {
    pub async fn new(
        rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url);
        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let db: DbObj = Arc::new(AnyDb::new(db_conn).await?);
        let domain = boundless_market.eip712_domain().await?;
        let cache = HashMap::new();

        Ok(Self { boundless_market, db, domain, config, cache, order_stream_client: None })
    }

    pub async fn new_with_order_stream(
        rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
        order_stream_url: Url,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url.clone());
        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let db: DbObj = Arc::new(AnyDb::new(db_conn).await?);
        let domain = boundless_market.eip712_domain().await?;
        let cache = HashMap::new();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get chain id: {}", e)))?;
        let order_stream_client =
            Some(OrderStreamClient::new(order_stream_url, boundless_market_address, chain_id));

        Ok(Self { boundless_market, db, domain, config, cache, order_stream_client })
    }
}

impl<P> IndexerService<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub async fn run(&mut self, starting_block: Option<u64>) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);

        let mut from_block: u64 = self.starting_block(starting_block).await?;
        tracing::info!("Starting indexer at block {}", from_block);

        let mut attempt = 0;
        loop {
            interval.tick().await;

            match self.current_block().await {
                Ok(to_block) => {
                    if to_block < from_block {
                        continue;
                    }

                    // cap to at most 500 blocks per batch
                    let batch_end = min(to_block, from_block.saturating_add(MAX_BATCH_SIZE));

                    tracing::info!("Processing blocks from {} to {}", from_block, batch_end);

                    match self.process_blocks(from_block, batch_end).await {
                        Ok(_) => {
                            attempt = 0;
                            from_block = batch_end + 1;
                        }
                        Err(e) => match e {
                            // Irrecoverable errors
                            ServiceError::DatabaseError(_)
                            | ServiceError::MaxRetries
                            | ServiceError::RequestNotExpired
                            | ServiceError::Error(_) => {
                                tracing::error!(
                                    "Failed to process blocks from {} to {}: {:?}",
                                    from_block,
                                    batch_end,
                                    e
                                );
                                return Err(e);
                            }
                            // Recoverable errors
                            ServiceError::BoundlessMarketError(_)
                            | ServiceError::EventQueryError(_)
                            | ServiceError::RpcError(_) => {
                                attempt += 1;
                                // exponential backoff with a maximum delay of 120 seconds
                                let delay =
                                    std::time::Duration::from_secs(2u64.pow(attempt - 1).min(120));
                                tracing::warn!(
                                    "Failed to process blocks from {} to {}: {:?}, attempt number {}, retrying in {}s",
                                    from_block,
                                    batch_end,
                                    e,
                                    attempt,
                                    delay.as_secs()
                                );
                                tokio::time::sleep(delay).await;
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

    async fn process_blocks(&mut self, from: u64, to: u64) -> Result<(), ServiceError> {
        self.process_request_submitted_events(from, to).await?;
        self.process_request_submitted_offchain().await?;
        self.process_locked_events(from, to).await?;
        self.process_proof_delivered_events(from, to).await?;
        self.process_fulfilled_events(from, to).await?;
        self.process_callback_failed_events(from, to).await?;
        self.process_slashed_events(from, to).await?;
        self.process_deposit_events(from, to).await?;
        self.process_withdrawal_events(from, to).await?;
        self.process_collateral_deposit_events(from, to).await?;
        self.process_collateral_withdrawal_events(from, to).await?;
        self.clear_cache();

        self.update_last_processed_block(to).await?;

        // Aggregate hourly market data after processing blocks
        self.aggregate_hourly_market_data().await?;

        Ok(())
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        Ok(self.db.set_last_block(block_number).await?)
    }

    async fn process_request_submitted_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .RequestSubmitted_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} request submitted events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;

            tracing::debug!(
                "Processing request submitted event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            let request = event.request.clone();

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            self.db.add_proof_request(request_digest, request, &metadata, "onchain").await?;
            self.db.add_request_submitted_event(request_digest, event.requestId, &metadata).await?;
        }

        Ok(())
    }

    async fn process_request_submitted_offchain(&mut self) -> Result<(), ServiceError> {
        let Some(order_stream_client) = &self.order_stream_client else {
            return Ok(());
        };

        let last_processed = self.db.get_last_order_stream_timestamp().await?;

        let current_block = self.current_block().await?;
        let block_timestamp = self.block_timestamp(current_block).await?;

        tracing::debug!(
            "Processing offchain orders. Last processed timestamp: {:?}",
            last_processed
        );

        const MAX_ORDERS_PER_BATCH: u64 = 100;
        let mut after = last_processed;
        let mut latest_timestamp = last_processed;
        let mut total_orders = 0;

        loop {
            let orders = order_stream_client
                .list_orders_by_creation(after, MAX_ORDERS_PER_BATCH)
                .await
                .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch orders: {}", e)))?;

            if orders.is_empty() {
                break;
            }

            tracing::debug!("Fetched {} orders from order stream", orders.len());

            for order_data in &orders {
                let request = &order_data.order.request;
                let request_digest = order_data.order.request_digest;

                if self.db.has_proof_request(request_digest).await? {
                    tracing::debug!(
                        "Skipping order 0x{:x} - already exists in database",
                        request.id
                    );
                    continue;
                }

                let request_id = RequestId::from_lossy(request.id);
                let metadata =
                    TxMetadata::new(B256::ZERO, request_id.addr, current_block, block_timestamp);

                self.db
                    .add_proof_request(request_digest, request.clone(), &metadata, "offchain")
                    .await?;

                let created_at = order_data.created_at;
                if latest_timestamp.is_none() || latest_timestamp.unwrap() < created_at {
                    latest_timestamp = Some(created_at);
                }

                total_orders += 1;
            }

            if (orders.len() as u64) < MAX_ORDERS_PER_BATCH {
                break;
            }

            after = orders.last().map(|o| o.created_at);
        }

        if total_orders > 0 {
            tracing::info!("Processed {} offchain orders from order stream", total_orders);
        }

        if let Some(ts) = latest_timestamp {
            self.db.set_last_order_stream_timestamp(ts).await?;
        }

        Ok(())
    }

    async fn process_locked_events(
        &mut self,
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
        tracing::debug!(
            "Found {} locked events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing request locked event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            // Get the request and calculate its digest
            let request = event.request.clone();
            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            // Check if we've already seen this request (from RequestSubmitted event)
            // If not, it must have been submitted off-chain. We add it to the database.
            let request_exists = self.db.has_proof_request(request_digest).await?;
            if !request_exists {
                tracing::debug!("Detected request locked for unseen request. Likely submitted off-chain: 0x{:x}", event.requestId);
                self.db.add_proof_request(request_digest, request, &metadata, "offchain").await?;
            }
            self.db
                .add_request_locked_event(request_digest, event.requestId, event.prover, &metadata)
                .await?;
        }

        Ok(())
    }

    async fn process_proof_delivered_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .ProofDelivered_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} proof delivered events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing proof delivered event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            self.db
                .add_proof_delivered_event(
                    event.fulfillment.requestDigest,
                    event.requestId,
                    &metadata,
                )
                .await?;
            self.db.add_fulfillment(event.fulfillment, event.prover, &metadata).await?;
        }

        Ok(())
    }

    async fn process_fulfilled_events(
        &mut self,
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
        tracing::debug!(
            "Found {} fulfilled events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing fulfilled event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db
                .add_request_fulfilled_event(event.requestDigest, event.requestId, &metadata)
                .await?;
        }

        Ok(())
    }

    async fn process_slashed_events(
        &mut self,
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
        tracing::debug!(
            "Found {} slashed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing slashed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db
                .add_prover_slashed_event(
                    event.requestId,
                    event.collateralBurned,
                    event.collateralTransferred,
                    event.collateralRecipient,
                    &metadata,
                )
                .await?;
        }

        Ok(())
    }

    async fn process_deposit_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .Deposit_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_deposit_event(event.account, event.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_withdrawal_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .Withdrawal_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_withdrawal_event(event.account, event.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_collateral_deposit_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .CollateralDeposit_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} collateral deposit events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing collateral deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_collateral_deposit_event(event.account, event.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_collateral_withdrawal_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .CollateralWithdrawal_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} collateral withdrawal events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing collateral withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_collateral_withdrawal_event(event.account, event.value, &metadata).await?;
        }

        Ok(())
    }

    async fn process_callback_failed_events(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let event_filter = self
            .boundless_market
            .instance()
            .CallbackFailed_filter()
            .from_block(from_block)
            .to_block(to_block);

        // Query the logs for the event
        let logs = event_filter.query().await?;
        tracing::debug!(
            "Found {} callback failed events from block {} to block {}",
            logs.len(),
            from_block,
            to_block
        );

        for (event, log_data) in logs {
            let metadata = self.fetch_tx_metadata(log_data).await?;
            tracing::debug!(
                "Processing callback failed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            self.db
                .add_callback_failed_event(
                    event.requestId,
                    event.callback,
                    event.error.to_vec(),
                    &metadata,
                )
                .await?;
        }

        Ok(())
    }

    async fn aggregate_hourly_market_data(&self) -> Result<(), ServiceError> {
        tracing::info!("Aggregating hourly market data for past 12 hours");

        let current_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get current time: {}", e)))?
            .as_secs() as i64;

        // Calculate 12 hours ago
        let twelve_hours_ago = current_time - (12 * 3600);

        // Truncate to hour boundaries
        let start_hour = (twelve_hours_ago / 3600) * 3600;
        let current_hour = (current_time / 3600) * 3600;

        tracing::debug!(
            "Aggregating hours from {} to {} (12 hour window)",
            start_hour,
            current_hour
        );

        // Process each hour
        for hour_ts in (start_hour..=current_hour).step_by(3600) {
            let summary = self.compute_hourly_summary(hour_ts).await?;
            self.db.upsert_hourly_market_summary(summary).await?;
        }

        tracing::info!("Hourly market data aggregation completed");
        Ok(())
    }

    async fn compute_hourly_summary(
        &self,
        hour_timestamp: i64,
    ) -> Result<HourlyMarketSummary, ServiceError> {
        let hour_end = hour_timestamp + 3600;

        // 1. Count total fulfilled in this hour
        let total_fulfilled: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM request_fulfilled_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(hour_timestamp)
        .bind(hour_end)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_fulfilled: {}", e)))?;

        // 2. Count unique provers who locked requests in this hour
        let unique_provers: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT prover_address) FROM request_locked_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(hour_timestamp)
        .bind(hour_end)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch unique_provers: {}", e)))?;

        // 3. Count unique requesters who submitted requests in this hour
        let unique_requesters: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT client_address) FROM proof_requests
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(hour_timestamp)
        .bind(hour_end)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch unique_requesters: {}", e)))?;

        // 4. Get all locks in this hour with pricing info
        let locks = sqlx::query(
            "SELECT
                pr.min_price,
                pr.max_price,
                pr.bidding_start,
                pr.ramp_up_period,
                pr.lock_end,
                pr.lock_collateral,
                rle.block_timestamp as lock_timestamp
             FROM request_locked_events rle
             JOIN proof_requests pr ON rle.request_digest = pr.request_digest
             WHERE rle.block_timestamp >= $1 AND rle.block_timestamp < $2",
        )
        .bind(hour_timestamp)
        .bind(hour_end)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch locks: {}", e)))?;

        // Compute fees and collateral
        let mut total_fees = U256::ZERO;
        let mut total_collateral = U256::ZERO;
        let mut prices = Vec::new();

        for row in locks {
            let min_price_str: String = row.get("min_price");
            let max_price_str: String = row.get("max_price");
            let bidding_start: i64 = row.get("bidding_start");
            let ramp_up_period: i32 = row.get("ramp_up_period");
            let lock_end: i64 = row.get("lock_end");
            let lock_collateral_str: String = row.get("lock_collateral");
            let lock_timestamp: i64 = row.get("lock_timestamp");

            let min_price = U256::from_str(&min_price_str)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse min_price: {}", e)))?;
            let max_price = U256::from_str(&max_price_str)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse max_price: {}", e)))?;
            let lock_collateral = U256::from_str(&lock_collateral_str)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e)))?;

            // Compute lock_timeout from lock_end and bidding_start
            let lock_timeout = (lock_end - bidding_start) as u32;

            // Compute price at lock time
            let price = price_at_time(
                min_price,
                max_price,
                bidding_start as u64,
                ramp_up_period as u32,
                lock_timeout,
                lock_timestamp as u64,
            );

            total_fees += price;
            total_collateral += lock_collateral;
            prices.push(price);
        }

        // Compute percentiles
        let p50 = if !prices.is_empty() {
            let mut sorted_prices = prices.clone();
            compute_percentiles(&mut sorted_prices, &[50])[0]
        } else {
            U256::ZERO
        };

        let percentiles = if !prices.is_empty() {
            let mut sorted_prices = prices;
            compute_percentiles(&mut sorted_prices, &[10, 20, 30, 40, 50, 60, 70, 80, 90, 100])
        } else {
            vec![U256::ZERO; 10]
        };

        let percentiles_blob = encode_percentiles_to_bytes(&percentiles);

        // Format U256 values as zero-padded 78-character strings (matching rewards pattern)
        fn format_u256(value: U256) -> String {
            format!("{:0>78}", value)
        }

        Ok(HourlyMarketSummary {
            hour_timestamp,
            total_fulfilled,
            unique_provers_locking_requests: unique_provers,
            unique_requesters_submitting_requests: unique_requesters,
            total_fees_locked: format_u256(total_fees),
            total_collateral_locked: format_u256(total_collateral),
            p50_fees_locked: format_u256(p50),
            percentile_fees_locked: percentiles_blob,
        })
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        Ok(self.boundless_market.instance().provider().get_block_number().await?)
    }

    async fn block_timestamp(&self, block_number: u64) -> Result<u64, ServiceError> {
        let timestamp = self.db.get_block_timestamp(block_number).await?;
        let ts = match timestamp {
            Some(ts) => ts,
            None => {
                tracing::debug!("Block timestamp not found in DB for block {}", block_number);
                let ts = self
                    .boundless_market
                    .instance()
                    .provider()
                    .get_block_by_number(BlockNumberOrTag::Number(block_number))
                    .await?
                    .context(anyhow!("Failed to get block by number: {}", block_number))?
                    .header
                    .timestamp;
                self.db.add_block(block_number, ts).await?;
                ts
            }
        };
        Ok(ts)
    }

    fn clear_cache(&mut self) {
        self.cache.clear();
    }

    // Fetch (and cache) metadata for a tx
    // Check if the transaction is already in the cache
    // If it is, use the cached tx metadata
    // Otherwise, fetch the transaction from the provider and cache it
    // This is to avoid making multiple calls to the provider for the same transaction
    // as delivery events may be emitted in a batch
    async fn fetch_tx_metadata(&mut self, log: Log) -> Result<TxMetadata, ServiceError> {
        let tx_hash = log.transaction_hash.context("Transaction hash not found")?;
        if let Some(meta) = self.cache.get(&tx_hash) {
            return Ok(meta.clone());
        }
        let tx = self
            .boundless_market
            .instance()
            .provider()
            .get_transaction_by_hash(tx_hash)
            .await?
            .context(anyhow!("Transaction not found: {}", hex::encode(tx_hash)))?;
        let bn = tx.block_number.context("block number not found")?;
        let ts =
            if let Some(ts) = log.block_timestamp { ts } else { self.block_timestamp(bn).await? };
        let meta = TxMetadata::new(tx_hash, tx.from(), bn, ts);
        self.cache.insert(tx_hash, meta.clone());
        Ok(meta)
    }

    // Return the last processed block from the DB is > 0;
    // otherwise, return the starting_block if set and <= current_block;
    // otherwise, return the current_block.
    async fn starting_block(&self, starting_block: Option<u64>) -> Result<u64, ServiceError> {
        let last_processed = self.get_last_processed_block().await?;
        let current_block = self.current_block().await?;
        Ok(find_starting_block(starting_block, last_processed, current_block))
    }
}

fn find_starting_block(
    starting_block: Option<u64>,
    last_processed: Option<u64>,
    current_block: u64,
) -> u64 {
    if let Some(last) = last_processed.filter(|&b| b > 0) {
        tracing::debug!("Using last processed block {} as starting block", last);
        return last;
    }

    let from = starting_block.unwrap_or(current_block);
    if from > current_block {
        tracing::warn!(
            "Starting block {} is greater than current block {}, defaulting to current block",
            from,
            current_block
        );
        current_block
    } else {
        tracing::debug!("Using {} as starting block", from);
        from
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_starting_block() {
        let starting_block = Some(100);
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = None;
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = None;
        let last_processed = Some(0);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = Some(200);
        let last_processed = None;
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100);

        let starting_block = Some(200);
        let last_processed = Some(10);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 10);
    }
}
