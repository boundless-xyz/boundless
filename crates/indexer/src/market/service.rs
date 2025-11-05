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

use std::{cmp::min, collections::{HashMap, HashSet}, sync::Arc};

use crate::{
    db::{
        market::{AnyDb, HourlyMarketSummary},
        DbError, DbObj, TxMetadata,
    },
    market::{cache::CacheStorage, pricing::compute_percentiles},
};
use ::boundless_market::contracts::pricing::price_at_time;
use ::boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    EIP712DomainSaltless, IBoundlessMarket, RequestId,
};
use ::boundless_market::order_stream_client::{OrderStreamClient, SortDirection};
use alloy::{
    eips::{BlockId, BlockNumberOrTag},
    network::{AnyNetwork, Ethereum},
    primitives::{Address, B256, U256},
    providers::{
        Identity, Provider, ProviderBuilder, RootProvider, fillers::{ChainIdFiller, FillProvider, JoinFill}
    },
    rpc::types::{Filter, Log},
    signers::local::PrivateKeySigner,
    sol_types::SolEvent,
    transports::{RpcError, TransportErrorKind},
};
use std::str::FromStr;
use anyhow::{anyhow, Context};
use futures_util::future::try_join_all;
use sqlx::Row;
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

const SECONDS_PER_HOUR: u64 = 3600;
const SECONDS_PER_DAY: u64 = 86400;
const SECONDS_PER_WEEK: u64 = 604800;
const HOURLY_AGGREGATION_RECOMPUTE_HOURS: u64 = 6;
const DAILY_AGGREGATION_RECOMPUTE_DAYS: u64 = 6;
const WEEKLY_AGGREGATION_RECOMPUTE_WEEKS: u64 = 6;
const MONTHLY_AGGREGATION_RECOMPUTE_MONTHS: u64 = 6;
const GET_BLOCK_RECEIPTS_CHUNK_SIZE: usize = 250;
const GET_BLOCK_BY_NUMBER_CHUNK_SIZE: usize = 250;
const BLOCK_QUERY_SLEEP: u64 = 1;

/// Helper functions for calculating period boundaries
/// Returns the start of the calendar day (00:00:00 UTC) for a given timestamp
fn get_day_start(timestamp: u64) -> u64 {
    (timestamp / SECONDS_PER_DAY) * SECONDS_PER_DAY
}

/// Returns the start of the calendar week (Monday 00:00:00 UTC) for a given timestamp
/// Uses ISO 8601 standard where Monday is the first day of the week
fn get_week_start(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc, Weekday};
    
    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    let weekday = dt.weekday();
    
    // Calculate days to subtract to get to Monday
    let days_from_monday = match weekday {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    
    let monday = dt - chrono::Duration::days(days_from_monday);
    let monday_start = monday.date_naive().and_hms_opt(0, 0, 0).unwrap();
    monday_start.and_utc().timestamp() as u64
}

/// Returns the start of the calendar month (1st day 00:00:00 UTC) for a given timestamp
fn get_month_start(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc};
    
    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    let month_start = Utc
        .with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
        .unwrap();
    month_start.timestamp() as u64
}

/// Returns the start of the next calendar day
fn get_next_day(timestamp: u64) -> u64 {
    get_day_start(timestamp) + SECONDS_PER_DAY
}

/// Returns the start of the next calendar week
fn get_next_week(timestamp: u64) -> u64 {
    get_week_start(timestamp) + SECONDS_PER_WEEK
}

/// Returns the start of the next calendar month
fn get_next_month(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc};
    
    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    
    // Add one month
    let next_month = if dt.month() == 12 {
        Utc.with_ymd_and_hms(dt.year() + 1, 1, 1, 0, 0, 0).unwrap()
    } else {
        Utc.with_ymd_and_hms(dt.year(), dt.month() + 1, 1, 0, 0, 0).unwrap()
    };
    
    next_month.timestamp() as u64
}

/// Event signatures for market events that are indexed.
const MARKET_EVENT_SIGNATURES: &[B256] = &[
    IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH,
    IBoundlessMarket::RequestLocked::SIGNATURE_HASH,
    IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH,
    IBoundlessMarket::ProofDelivered::SIGNATURE_HASH,
    IBoundlessMarket::ProverSlashed::SIGNATURE_HASH,
    IBoundlessMarket::Deposit::SIGNATURE_HASH,
    IBoundlessMarket::Withdrawal::SIGNATURE_HASH,
    IBoundlessMarket::CollateralDeposit::SIGNATURE_HASH,
    IBoundlessMarket::CollateralWithdrawal::SIGNATURE_HASH,
    IBoundlessMarket::CallbackFailed::SIGNATURE_HASH,
];

type ProviderWallet = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;
type AnyNetworkProvider = FillProvider<JoinFill<JoinFill<Identity, JoinFill<alloy::providers::fillers::GasFiller, JoinFill<alloy::providers::fillers::BlobGasFiller, JoinFill<alloy::providers::fillers::NonceFiller, ChainIdFiller>>>>, ChainIdFiller>, RootProvider<AnyNetwork>, AnyNetwork>;

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
pub struct IndexerService<P, ANP> {
    pub boundless_market: BoundlessMarketService<P>,
    pub provider: P,
    // Any network provider enables us to call get_block_receipts for any network, not just Ethereum.
    // E.g. for OP it contains logic to support L1 -> L2 deposit receipts in the response for get_block_receipts.
    pub any_network_provider: ANP,
    // Provider dedicated to get_logs calls
    pub logs_provider: P,
    pub db: DbObj,
    pub domain: EIP712DomainSaltless,
    pub config: IndexerServiceConfig,
    // Mapping from transaction hash to TxMetadata
    pub tx_hash_to_metadata: HashMap<B256, TxMetadata>,
    // Mapping from block number to timestamp
    pub block_num_to_timestamp: HashMap<u64, u64>,
    // Optional order stream client for fetching off-chain orders
    pub order_stream_client: Option<OrderStreamClient>,
    // Optional cache storage for logs and transaction metadata
    pub cache_storage: Option<Arc<dyn CacheStorage>>,
    // Chain ID for cache keys
    pub chain_id: u64,
}

#[derive(Clone, Copy, Debug)]
pub enum TransactionFetchStrategy {
    /// Use eth_getBlockReceipts - more efficient, fewer RPC calls
    BlockReceipts,
    /// Use eth_getTransactionByHash - compatible with all providers
    TransactionByHash,
}

#[derive(Clone)]
pub struct IndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
    pub batch_size: u64,
    pub cache_uri: Option<String>,
    pub tx_fetch_strategy: TransactionFetchStrategy,
}

impl IndexerService<ProviderWallet, AnyNetworkProvider> {
    pub async fn new(
        rpc_url: Url,
        logs_rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let any_network_provider: AnyNetworkProvider = ProviderBuilder::new()
            .network::<AnyNetwork>()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url.clone());
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url.clone());
        let logs_provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(logs_rpc_url);
        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let db: DbObj = Arc::new(AnyDb::new(db_conn).await?);
        let domain = boundless_market.eip712_domain().await?;
        let tx_hash_to_metadata = HashMap::new();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get chain id: {}", e)))?;

        // Initialize cache storage if URI provided
        let cache_storage = if let Some(uri) = &config.cache_uri {
            match crate::market::cache::cache_storage_from_uri(uri).await {
                Ok(storage) => {
                    tracing::info!("Cache storage initialized with URI: {}", uri);
                    Some(Arc::from(storage))
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize cache storage from URI '{}': {}", uri, e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { boundless_market, provider, any_network_provider, logs_provider, db, domain, config, tx_hash_to_metadata, block_num_to_timestamp: HashMap::new(), order_stream_client: None, cache_storage, chain_id })
    }

    pub async fn new_with_order_stream(
        rpc_url: Url,
        logs_rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
        order_stream_url: Url,
    ) -> Result<Self, ServiceError> {
        let caller = private_key.address();
        let any_network_provider: AnyNetworkProvider = ProviderBuilder::new()
            .network::<AnyNetwork>()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url.clone());
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(rpc_url.clone());
        let logs_provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_http(logs_rpc_url);
        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let db: DbObj = Arc::new(AnyDb::new(db_conn).await?);
        let domain = boundless_market.eip712_domain().await?;
        let tx_hash_to_metadata = HashMap::new();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get chain id: {}", e)))?;
        let order_stream_client =
            Some(OrderStreamClient::new(order_stream_url, boundless_market_address, chain_id));

        // Initialize cache storage if URI provided
        let cache_storage = if let Some(uri) = &config.cache_uri {
            match crate::market::cache::cache_storage_from_uri(uri).await {
                Ok(storage) => {
                    tracing::info!("Cache storage initialized with URI: {}", uri);
                    Some(Arc::from(storage))
                }
                Err(e) => {
                    tracing::warn!("Failed to initialize cache storage from URI '{}': {}", uri, e);
                    None
                }
            }
        } else {
            None
        };

        Ok(Self { boundless_market, any_network_provider, provider, logs_provider, db, domain, config, tx_hash_to_metadata, block_num_to_timestamp: HashMap::new(), order_stream_client, cache_storage, chain_id })
    }
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub async fn run(&mut self, starting_block: Option<u64>, end_block: Option<u64>) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);

        let mut from_block: u64 = self.starting_block(starting_block).await?;
        
        // Validate end_block if provided
        if let Some(end) = end_block {
            if end < from_block {
                return Err(ServiceError::Error(anyhow::anyhow!(
                    "End block {} is less than starting block {}",
                    end,
                    from_block
                )));
            }
            let current_block = self.current_block().await?;
            if end > current_block {
                return Err(ServiceError::Error(anyhow::anyhow!(
                    "End block {} is greater than current block {}",
                    end,
                    current_block
                )));
            }
            tracing::info!("Starting indexer at block {} (will stop at block {})", from_block, end);
        } else {
            tracing::info!("Starting indexer at block {}", from_block);
        }

        let mut attempt = 0;
        loop {
            interval.tick().await;

            // Determine the maximum block we can process up to
            let max_block = match self.current_block().await {
                Ok(to_block) => {
                    // If end_block is specified, use the minimum of current block and end_block
                    if let Some(end) = end_block {
                        min(to_block, end)
                    } else {
                        to_block
                    }
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!(
                        "Failed to fetch current block: {:?}, attempt number {}",
                        e,
                        attempt
                    );
                    if attempt > self.config.retries {
                        tracing::error!("Aborting after {} consecutive attempts", attempt);
                        return Err(ServiceError::MaxRetries);
                    }
                    continue;
                }
            };

            if max_block < from_block {
                // If we've reached the end_block and processed everything, exit
                if let Some(end) = end_block {
                    if from_block > end {
                        tracing::info!("Reached end block {}, exiting", end);
                        return Ok(());
                    }
                }
                continue;
            }

            // cap to at most batch_size blocks per batch, but also respect end_block
            let batch_end = min(max_block, from_block.saturating_add(self.config.batch_size));

            tracing::info!("Processing blocks from {} to {}", from_block, batch_end);

            let start = std::time::Instant::now();
            match self.process_blocks(from_block, batch_end).await {
                Ok(_) => {
                    tracing::info!("process_blocks completed in {:?}", start.elapsed());
                    attempt = 0;
                    from_block = batch_end + 1;
                    
                    // If we've reached or passed the end_block, exit
                    if let Some(end) = end_block {
                        if from_block > end {
                            tracing::info!("Reached end block {}, exiting", end);
                            return Ok(());
                        }
                    }
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
            
            if attempt > self.config.retries {
                tracing::error!("Aborting after {} consecutive attempts", attempt);
                return Err(ServiceError::MaxRetries);
            }
        }
    }

    async fn process_blocks(&mut self, from: u64, to: u64) -> Result<(), ServiceError> {
        // Fetch all relevant logs once with a single filter
        let logs = self.fetch_logs(from, to).await?;

        // Batch fetch all transactions and blocks in parallel and populate cache
        self.fetch_tx_metadata(&logs, from, to).await?;

        // Process each event type by filtering the logs and collect touched requests
        let mut updated_requests = HashSet::new();

        updated_requests.extend(self.process_request_submitted_events(&logs).await?);
        updated_requests.extend(self.process_request_submitted_offchain(to).await?);
        updated_requests.extend(self.process_locked_events(&logs).await?);
        updated_requests.extend(self.process_proof_delivered_events(&logs).await?);
        updated_requests.extend(self.process_fulfilled_events(&logs).await?);
        updated_requests.extend(self.process_callback_failed_events(&logs).await?);
        updated_requests.extend(self.process_slashed_events(&logs).await?);

        self.process_deposit_events(&logs).await?;
        self.process_withdrawal_events(&logs).await?;
        self.process_collateral_deposit_events(&logs).await?;
        self.process_collateral_withdrawal_events(&logs).await?;

        // Find requests that expired during this block range
        updated_requests.extend(self.get_newly_expired_requests(from, to).await?);

        // Update request statuses for all touched requests
        self.update_request_statuses(updated_requests, to).await?;

        // Update the last processed block. 
        self.update_last_processed_block(to).await?;

        // Aggregate market data at all time periods after processing blocks
        self.aggregate_hourly_market_data(to).await?;
        self.aggregate_daily_market_data(to).await?;
        self.aggregate_weekly_market_data(to).await?;
        self.aggregate_monthly_market_data(to).await?;

        self.clear_cache();

        Ok(())
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        self.db.set_last_block(block_number).await?;
        tracing::info!("update_last_processed_block completed in {:?}", start.elapsed());
        Ok(())
    }

    fn compute_request_status(
        &self,
        req: crate::db::market::RequestWithEvents,
        current_timestamp: u64,
    ) -> crate::db::market::RequestStatus {
        use crate::db::market::RequestStatus;

        let request_status = if req.fulfilled_at.is_some() {
            "fulfilled"
        } else if req.slashed_at.is_some() {
            "slashed"
        } else if current_timestamp > req.expires_at {
            "expired"
        } else if req.locked_at.is_some() {
            "locked"
        } else {
            "submitted"
        };

        let updated_at = [
            req.submitted_at,
            req.locked_at,
            req.fulfilled_at,
            req.slashed_at,
        ]
        .iter()
        .filter_map(|&t| t)
        .max()
        .unwrap_or(req.created_at);

        RequestStatus {
            request_digest: req.request_digest,
            request_id: req.request_id,
            request_status: request_status.to_string(),
            source: req.source,
            client_address: req.client_address,
            prover_address: req.prover_address,
            created_at: req.created_at,
            updated_at,
            locked_at: req.locked_at,
            fulfilled_at: req.fulfilled_at,
            slashed_at: req.slashed_at,
            submit_block: req.submit_block,
            lock_block: req.lock_block,
            fulfill_block: req.fulfill_block,
            slashed_block: req.slashed_block,
            min_price: req.min_price,
            max_price: req.max_price,
            lock_collateral: req.lock_collateral,
            ramp_up_start: req.ramp_up_start,
            ramp_up_period: req.ramp_up_period,
            expires_at: req.expires_at,
            lock_end: req.lock_end,
            slash_recipient: req.slash_recipient,
            slash_transferred_amount: req.slash_transferred_amount,
            slash_burned_amount: req.slash_burned_amount,
            cycles: req.cycles,
            peak_prove_mhz: req.peak_prove_mhz,
            effective_prove_mhz: req.effective_prove_mhz,
            submit_tx_hash: req.submit_tx_hash,
            lock_tx_hash: req.lock_tx_hash,
            fulfill_tx_hash: req.fulfill_tx_hash,
            slash_tx_hash: req.slash_tx_hash,
            image_id: req.image_id,
            image_url: req.image_url,
            selector: req.selector,
            predicate_type: req.predicate_type,
            predicate_data: req.predicate_data,
            input_type: req.input_type,
            input_data: req.input_data,
            fulfill_journal: req.fulfill_journal,
            fulfill_seal: req.fulfill_seal,
        }
    }

    async fn update_request_statuses(
        &mut self,
        request_digests: HashSet<B256>,
        block_number: u64,
    ) -> Result<(), ServiceError> {
        if request_digests.is_empty() {
            return Ok(());
        }

        let current_timestamp = self.block_timestamp(block_number).await?;

        let start = std::time::Instant::now();

        tracing::debug!("Updating statuses for {} requests based on block {} timestamp {}", request_digests.len(), block_number, current_timestamp);
        
        let requests_with_events = self.db.get_requests_with_events(&request_digests).await?;

        let request_statuses: Vec<_> = requests_with_events
            .into_iter()
            .map(|req| self.compute_request_status(req, current_timestamp))
            .collect();

        self.db.upsert_request_statuses(&request_statuses).await?;

        tracing::info!(
            "update_request_statuses completed in {:?} [{} statuses updated]",
            start.elapsed(),
            request_statuses.len()
        );

        Ok(())
    }

    async fn get_newly_expired_requests(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<HashSet<B256>, ServiceError> {
        let from_timestamp = self.block_timestamp(from_block).await?;
        let to_timestamp = self.block_timestamp(to_block).await?;
        Ok(self.db.find_newly_expired_requests(from_timestamp, to_timestamp).await?)
    }

    async fn fetch_logs(&self, from: u64, to: u64) -> Result<Vec<Log>, ServiceError> {
        let start = std::time::Instant::now();

        // Try to get from cache if enabled
        if let Some(cache) = &self.cache_storage {
            match cache.get_logs(self.chain_id, from, to, MARKET_EVENT_SIGNATURES).await {
                Ok(Some(logs)) => {
                    tracing::info!(
                        "Cache hit for logs from block {} to {} [got {} logs in {:?}]",
                        from,
                        to,
                        logs.len(),
                        start.elapsed()
                    );
                    return Ok(logs);
                }
                Ok(None) => {
                    tracing::warn!("Cache miss for logs from block {} to {}", from, to);
                }
                Err(e) => {
                    tracing::warn!("Cache read error for logs from block {} to {}: {}", from, to, e);
                }
            }
        }

        let filter = Filter::new()
            .address(*self.boundless_market.instance().address())
            .from_block(from)
            .to_block(to)
            .event_signature(MARKET_EVENT_SIGNATURES.to_vec());

        tracing::info!(
            "Fetching logs from RPC: block {} to block {}",
            from,
            to
        );

        let logs = self.logs_provider.get_logs(&filter).await?;

        tracing::debug!(
            "Fetched {} total logs from block {} to block {}",
            logs.len(),
            from,
            to
        );

        // Save to cache if enabled
        if let Some(cache) = &self.cache_storage {
            if let Err(e) = cache.put_logs(self.chain_id, from, to, MARKET_EVENT_SIGNATURES, &logs).await {
                tracing::warn!("Failed to cache logs for block {} to {}: {}", from, to, e);
            }
        }

        tracing::info!("fetch_logs completed in {:?} [got {} logs]", start.elapsed(), logs.len());
        Ok(logs)
    }

    async fn process_request_submitted_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestSubmitted events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} request submitted events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestSubmitted>()
                .context("Failed to decode RequestSubmitted log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;

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

            touched_requests.insert(request_digest);
        }

        tracing::info!("process_request_submitted_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_request_submitted_offchain(&mut self, max_block: u64) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Get the block timestamp for the max block to use as upper bound for order filtering
        let block_timestamp = self.block_timestamp(max_block).await?;

        let Some(order_stream_client) = &self.order_stream_client else {
            return Ok(touched_requests);
        };

        let last_processed = self.db.get_last_order_stream_timestamp().await?;

        tracing::debug!(
            "Processing offchain orders up to block {} (timestamp: {}). Last processed timestamp: {:?}",
            max_block,
            block_timestamp,
            last_processed
        );

        const MAX_ORDERS_PER_BATCH: u64 = 500;
        let mut cursor: Option<String> = None;
        let mut latest_timestamp = last_processed;
        let mut total_orders = 0;

        // Convert block timestamp to DateTime and add 1 second to include orders at block_timestamp
        // (before parameter is exclusive, uses <)
        let before_timestamp = chrono::DateTime::from_timestamp(block_timestamp as i64 + 1, 0)
            .ok_or_else(|| ServiceError::Error(anyhow!("Invalid block timestamp")))?;

        loop {
            let response = order_stream_client
                .list_orders_v2(
                    cursor.clone(),
                    Some(MAX_ORDERS_PER_BATCH),
                    Some(SortDirection::Asc),
                    Some(before_timestamp),
                    last_processed,
                )
                .await
                .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch orders: {}", e)))?;

            if response.orders.is_empty() {
                break;
            }

            tracing::debug!("Fetched {} orders from order stream", response.orders.len());

            for order_data in &response.orders {
                let request = &order_data.order.request;
                let request_digest = order_data.order.request_digest;

                if self.db.has_proof_request(request_digest).await? {
                    tracing::warn!(
                        "Skipping order 0x{:x} - already exists in database",
                        request.id
                    );
                    continue;
                }

                let request_id = RequestId::from_lossy(request.id);
                // Off-chain orders have no associated on-chain transaction, so use sentinel values:
                // tx_hash = B256::ZERO, block_number = 0, block_timestamp = 0, transaction_index = 0
                let metadata =
                    TxMetadata::new(B256::ZERO, request_id.addr, 0, 0, 0);

                self.db
                    .add_proof_request(request_digest, request.clone(), &metadata, "offchain")
                    .await?;

                touched_requests.insert(request_digest);

                let created_at = order_data.created_at;
                if latest_timestamp.is_none() || latest_timestamp.unwrap() < created_at {
                    latest_timestamp = Some(created_at);
                }

                total_orders += 1;
            }

            // Check if there are more pages to fetch
            if !response.has_more {
                break;
            }

            // Update cursor for next page
            cursor = response.next_cursor;
        }

        if total_orders > 0 {
            tracing::info!("Processed {} offchain orders from order stream", total_orders);
        }

        if let Some(ts) = latest_timestamp {
            self.db.set_last_order_stream_timestamp(ts).await?;
        }

        tracing::info!("process_request_submitted_offchain completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_locked_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestLocked events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestLocked::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} locked events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestLocked>()
                .context("Failed to decode RequestLocked log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
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

            self.db
                .add_request_locked_event(request_digest, event.requestId, event.prover, &metadata)
                .await?;

            touched_requests.insert(request_digest);
        }

        tracing::info!("process_locked_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_proof_delivered_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for ProofDelivered events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::ProofDelivered::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} proof delivered events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::ProofDelivered>()
                .context("Failed to decode ProofDelivered log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing proof delivered event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            let request_digest = event.fulfillment.requestDigest;
            self.db
                .add_proof_delivered_event(
                    request_digest,
                    event.requestId,
                    &metadata,
                )
                .await?;
            self.db.add_fulfillment(event.fulfillment, event.prover, &metadata).await?;

            touched_requests.insert(request_digest);
        }

        tracing::info!("process_proof_delivered_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_fulfilled_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestFulfilled events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} fulfilled events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestFulfilled>()
                .context("Failed to decode RequestFulfilled log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing fulfilled event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db
                .add_request_fulfilled_event(event.requestDigest, event.requestId, &metadata)
                .await?;

            touched_requests.insert(event.requestDigest);
        }

        tracing::info!("process_fulfilled_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_slashed_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for ProverSlashed events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::ProverSlashed::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} slashed events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::ProverSlashed>()
                .context("Failed to decode ProverSlashed log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
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

            let request_digests = self.db.get_request_digests_by_request_id(event.requestId).await?;
            for digest in request_digests {
                touched_requests.insert(digest);
            }
        }

        tracing::info!("process_slashed_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn process_deposit_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        
        // Filter logs for Deposit events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::Deposit::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} deposit events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::Deposit>()
                .context("Failed to decode Deposit log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_deposit_event(event.account, event.value, &metadata).await?;
        }

        tracing::info!("process_deposit_events completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn process_withdrawal_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        
        // Filter logs for Withdrawal events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::Withdrawal::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} withdrawal events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::Withdrawal>()
                .context("Failed to decode Withdrawal log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_withdrawal_event(event.account, event.value, &metadata).await?;
        }

        tracing::info!("process_withdrawal_events completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn process_collateral_deposit_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        
        // Filter logs for CollateralDeposit events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CollateralDeposit::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} collateral deposit events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CollateralDeposit>()
                .context("Failed to decode CollateralDeposit log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing collateral deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_collateral_deposit_event(event.account, event.value, &metadata).await?;
        }

        tracing::info!("process_collateral_deposit_events completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn process_collateral_withdrawal_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        
        // Filter logs for CollateralWithdrawal events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CollateralWithdrawal::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} collateral withdrawal events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CollateralWithdrawal>()
                .context("Failed to decode CollateralWithdrawal log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing collateral withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );
            self.db.add_collateral_withdrawal_event(event.account, event.value, &metadata).await?;
        }

        tracing::info!("process_collateral_withdrawal_events completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn process_callback_failed_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for CallbackFailed events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CallbackFailed::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        tracing::debug!("Found {} callback failed events", logs.len());

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CallbackFailed>()
                .context("Failed to decode CallbackFailed log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
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

            let request_digests = self.db.get_request_digests_by_request_id(event.requestId).await?;
            for digest in request_digests {
                touched_requests.insert(digest);
            }
        }

        tracing::info!("process_callback_failed_events completed in {:?}", start.elapsed());
        Ok(touched_requests)
    }

    async fn aggregate_hourly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating hourly market data for past {} hours from block {} timestamp {}", HOURLY_AGGREGATION_RECOMPUTE_HOURS, to_block, current_time);

        // Calculate hours ago based on configured recompute window
        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);

        // Truncate to hour boundaries
        let start_hour = (hours_ago / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;
        let current_hour = (current_time / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;

        tracing::debug!(
            "Aggregating hours from {} to {} ({} hour window). Up to block timestamp: {}",
            start_hour,
            current_hour,
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            current_time
        );

        // Process each hour
        for hour_ts in (start_hour..=current_hour).step_by(SECONDS_PER_HOUR as usize) {
            let summary = self.compute_hourly_summary(hour_ts).await?;
            self.db.upsert_hourly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_hourly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn aggregate_daily_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating daily market data for past {} days from block {} timestamp {}", DAILY_AGGREGATION_RECOMPUTE_DAYS, to_block, current_time);

        // Get the current day start and calculate days ago
        let current_day_start = get_day_start(current_time);
        
        // Calculate which days to recompute
        let mut periods = Vec::new();
        let mut day_start = current_day_start;
        for _ in 0..DAILY_AGGREGATION_RECOMPUTE_DAYS {
            periods.push(day_start);
            // Go back one day
            day_start = day_start.saturating_sub(SECONDS_PER_DAY);
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} days from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_day_start
        );

        // Process each day
        for day_ts in periods {
            let day_end = get_next_day(day_ts);
            let summary = self.compute_period_summary(day_ts, day_end).await?;
            self.db.upsert_daily_market_summary(summary).await?;
        }

        tracing::info!("aggregate_daily_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn aggregate_weekly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating weekly market data for past {} weeks from block {} timestamp {}", WEEKLY_AGGREGATION_RECOMPUTE_WEEKS, to_block, current_time);

        // Get the current week start and calculate weeks ago
        let current_week_start = get_week_start(current_time);
        
        // Calculate which weeks to recompute
        let mut periods = Vec::new();
        let mut week_start = current_week_start;
        for _ in 0..WEEKLY_AGGREGATION_RECOMPUTE_WEEKS {
            periods.push(week_start);
            // Go back one week
            week_start = week_start.saturating_sub(SECONDS_PER_WEEK);
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} weeks from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_week_start
        );

        // Process each week
        for week_ts in periods {
            let week_end = get_next_week(week_ts);
            let summary = self.compute_period_summary(week_ts, week_end).await?;
            self.db.upsert_weekly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_weekly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn aggregate_monthly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating monthly market data for past {} months from block {} timestamp {}", MONTHLY_AGGREGATION_RECOMPUTE_MONTHS, to_block, current_time);

        // Get the current month start and calculate months ago
        let current_month_start = get_month_start(current_time);
        
        // Calculate which months to recompute
        let mut periods = Vec::new();
        let mut month_ts = current_month_start;
        for _ in 0..MONTHLY_AGGREGATION_RECOMPUTE_MONTHS {
            periods.push(month_ts);
            // Go back one month - need to use chrono for proper month arithmetic
            use chrono::{Datelike, TimeZone, Utc};
            let dt = Utc.timestamp_opt(month_ts as i64, 0).unwrap();
            let prev_month = if dt.month() == 1 {
                Utc.with_ymd_and_hms(dt.year() - 1, 12, 1, 0, 0, 0).unwrap()
            } else {
                Utc.with_ymd_and_hms(dt.year(), dt.month() - 1, 1, 0, 0, 0).unwrap()
            };
            month_ts = prev_month.timestamp() as u64;
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} months from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_month_start
        );

        // Process each month
        for month_ts in periods {
            let month_end = get_next_month(month_ts);
            let summary = self.compute_period_summary(month_ts, month_end).await?;
            self.db.upsert_monthly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_monthly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn compute_hourly_summary(
        &self,
        hour_timestamp: u64,
    ) -> Result<HourlyMarketSummary, ServiceError> {
        let hour_end = hour_timestamp.saturating_add(SECONDS_PER_HOUR);
        self.compute_period_summary(hour_timestamp, hour_end).await
    }

    async fn compute_period_summary(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<HourlyMarketSummary, ServiceError> {
        tracing::debug!("Computing period summary for {} to {}", period_start, period_end);

        // 1. Count total fulfilled in this period
        let total_fulfilled = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_fulfilled_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_fulfilled: {}", e)))? as u64;

        // 2. Count unique provers who locked requests in this period
        let unique_provers = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT prover_address) FROM request_locked_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch unique_provers: {}", e)))? as u64;

        // 3. Count unique requesters who submitted requests in this period
        let unique_requesters = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(DISTINCT client_address) FROM proof_requests
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch unique_requesters: {}", e)))? as u64;

        // 4. Count total requests submitted in this period
        let total_requests_submitted = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM proof_requests
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_requests_submitted: {}", e)))? as u64;

        // 5. Count total requests submitted onchain in this period
        let total_requests_submitted_onchain = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_submitted_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_requests_submitted_onchain: {}", e)))? as u64;

        // 6. Calculate offchain requests
        let total_requests_submitted_offchain = total_requests_submitted - total_requests_submitted_onchain;

        // 7. Count total requests locked in this period
        let total_requests_locked = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM request_locked_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_requests_locked: {}", e)))? as u64;

        // 8. Count total requests slashed in this period
        let total_requests_slashed = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM prover_slashed_events
             WHERE block_timestamp >= $1 AND block_timestamp < $2",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_one(self.db.pool())
        .await
        .map_err(|e| ServiceError::Error(anyhow!("Failed to fetch total_requests_slashed: {}", e)))? as u64;

        // 9. Get all locks in this period with pricing info
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
        .bind(period_start as i64)
        .bind(period_end as i64)
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
            let lock_timeout = (lock_end.saturating_sub(bidding_start)) as u32;

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

        // Compute percentiles: p10, p25, p50, p75, p90, p95, p99
        let percentiles = if !prices.is_empty() {
            let mut sorted_prices = prices;
            compute_percentiles(&mut sorted_prices, &[10, 25, 50, 75, 90, 95, 99])
        } else {
            vec![U256::ZERO; 7]
        };

        // Format U256 values as zero-padded 78-character strings (matching rewards pattern)
        fn format_u256(value: U256) -> String {
            format!("{:0>78}", value)
        }

        Ok(HourlyMarketSummary {
            period_timestamp: period_start,
            total_fulfilled,
            unique_provers_locking_requests: unique_provers,
            unique_requesters_submitting_requests: unique_requesters,
            total_fees_locked: format_u256(total_fees),
            total_collateral_locked: format_u256(total_collateral),
            p10_fees_locked: format_u256(percentiles[0]),
            p25_fees_locked: format_u256(percentiles[1]),
            p50_fees_locked: format_u256(percentiles[2]),
            p75_fees_locked: format_u256(percentiles[3]),
            p90_fees_locked: format_u256(percentiles[4]),
            p95_fees_locked: format_u256(percentiles[5]),
            p99_fees_locked: format_u256(percentiles[6]),
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
        })
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        Ok(self.boundless_market.instance().provider().get_block_number().await?)
    }

    async fn block_timestamp(&mut self, block_number: u64) -> Result<u64, ServiceError> {
        // Check in-memory cache first
        if let Some(ts) = self.block_num_to_timestamp.get(&block_number) {
            return Ok(*ts);
        }

        // Check database
        let timestamp = self.db.get_block_timestamp(block_number).await?;
        let ts = match timestamp {
            Some(ts) => {
                // Store in in-memory cache
                self.block_num_to_timestamp.insert(block_number, ts);
                ts
            }
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
                // Store in in-memory cache
                self.block_num_to_timestamp.insert(block_number, ts);
                ts
            }
        };
        Ok(ts)
    }

    fn clear_cache(&mut self) {
        self.tx_hash_to_metadata.clear();
        self.block_num_to_timestamp.clear();
    }

    // Batch fetch transaction metadata in parallel and populate the cache
    // Collects transaction hashes from logs, fetches transactions and block timestamps in parallel
    // Updates tx_hash_to_metadata and block_num_to_timestamp maps in the service object
    async fn fetch_tx_metadata(
        &mut self,
        logs: &[Log],
        from: u64,
        to: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        
        // Try to get from cache if enabled
        if let Some(cache) = &self.cache_storage {
            match cache.get_tx_metadata(self.chain_id, from, to, MARKET_EVENT_SIGNATURES).await {
                Ok(Some(metadata)) => {
                    tracing::info!(
                        "Cache hit for tx metadata from block {} to {} [got {} entries in {:?}]",
                        from,
                        to,
                        metadata.len(),
                        start.elapsed()
                    );
                    // Merge cached metadata into our map
                    self.tx_hash_to_metadata.extend(metadata);
                    return Ok(());
                }
                Ok(None) => {
                    tracing::warn!("Cache miss for tx metadata from block {} to {}", from, to);
                }
                Err(e) => {
                    tracing::warn!("Cache read error for tx metadata from block {} to {}: {}", from, to, e);
                }
            }
        }

        // Step 0: Collect unique transaction hashes from all logs
        let mut tx_hashes = HashSet::new();
        for log in logs {
            if let Some(tx_hash) = log.transaction_hash {
                tx_hashes.insert(tx_hash);
            }
        }

        if tx_hashes.is_empty() {
            tracing::debug!("No transaction hashes found in logs");
            tracing::info!("fetch_tx_metadata completed in {:?}", start.elapsed());
            return Ok(());
        }

        // Filter out transactions we already have in cache
        let missing_hashes: Vec<B256> = tx_hashes
            .iter()
            .filter(|&&tx_hash| !self.tx_hash_to_metadata.contains_key(&tx_hash))
            .copied()
            .collect();

        if missing_hashes.is_empty() {
            tracing::info!("All {} transaction hashes already in in-memory cache", tx_hashes.len());
            tracing::info!("fetch_tx_metadata completed in {:?}", start.elapsed());
            return Ok(());
        }

        tracing::debug!(
            "Fetching {} transactions in parallel ({} already cached)",
            missing_hashes.len(),
            tx_hashes.len() - missing_hashes.len()
        );

        // Fetch transaction metadata using the configured strategy
        match self.config.tx_fetch_strategy {
            TransactionFetchStrategy::BlockReceipts => {
                self.fetch_tx_metadata_via_block_receipts(logs, &missing_hashes).await?;
            }
            TransactionFetchStrategy::TransactionByHash => {
                self.fetch_tx_metadata_via_tx_by_hash(&missing_hashes).await?;
            }
        }

        tracing::debug!(
            "Successfully fetched {} transactions and {} block timestamps",
            missing_hashes.len(),
            self.block_num_to_timestamp.len()
        );

        // Save to cache if enabled
        if let Some(cache) = &self.cache_storage {
            if let Err(e) = cache.put_tx_metadata(self.chain_id, from, to, MARKET_EVENT_SIGNATURES, &self.tx_hash_to_metadata).await {
                tracing::warn!("Failed to cache tx metadata for block {} to {}: {}", from, to, e);
            }
        }

        tracing::info!("fetch_tx_metadata completed in {:?} [got {} transactions and {} block timestamps]", start.elapsed(), missing_hashes.len(), self.block_num_to_timestamp.len());
        Ok(())
    }

    // Fetch transaction metadata using eth_getBlockReceipts (optimized, fewer RPC calls)
    async fn fetch_tx_metadata_via_block_receipts(
        &mut self,
        logs: &[Log],
        missing_hashes: &[B256],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        // Step 1: Group transaction hashes by block number (from logs)
        let mut block_to_tx_hashes: HashMap<u64, Vec<B256>> = HashMap::new();
        for log in logs {
            if let (Some(tx_hash), Some(block_num)) = (log.transaction_hash, log.block_number) {
                if missing_hashes.contains(&tx_hash) {
                    block_to_tx_hashes.entry(block_num).or_default().push(tx_hash);
                }
            }
        }

        let block_numbers: Vec<u64> = block_to_tx_hashes.keys().copied().collect();

        // Step 2: Fetch block receipts for each block in parallel
        let mut receipt_map: HashMap<B256, (Address, u64, u64)> = HashMap::new();
        
        for chunk in block_numbers.chunks(GET_BLOCK_RECEIPTS_CHUNK_SIZE) {
            
            let receipt_futures: Vec<_> = chunk
                .iter()
                .map(|&block_num| {
                    let provider = self.any_network_provider.clone();
                    async move {
                        let receipts = provider.get_block_receipts(BlockId::Number(BlockNumberOrTag::Number(block_num))).await?;
                        Ok::<_, ServiceError>((block_num, receipts))
                    }
                })
                .collect();

            let start = std::time::Instant::now();
            let receipt_results = try_join_all(receipt_futures).await?;
            tracing::info!("Got {} receipts in {:?}", receipt_results.len(), start.elapsed());
            for (block_num, receipts_result) in receipt_results {
                if let Some(receipts) = receipts_result {
                    // Get the tx hashes we're looking for in this block
                    if let Some(expected_hashes) = block_to_tx_hashes.get(&block_num) {
                        for receipt in receipts {
                            let tx_hash = receipt.transaction_hash;
                            if expected_hashes.contains(&tx_hash) {
                                let from = receipt.from;
                                let tx_index = receipt.transaction_index.context(
                                    anyhow!("Transaction index not found for transaction {}", hex::encode(tx_hash))
                                )?;
                                let block_number = receipt.block_number.context(
                                    anyhow!("Block number not found for transaction {}", hex::encode(tx_hash))
                                )?;
                                receipt_map.insert(tx_hash, (from, tx_index, block_number));
                            }
                        }
                    }
                }
            }
        }

        // Verify we got all the transactions we expected
        for &tx_hash in missing_hashes.iter() {
            if !receipt_map.contains_key(&tx_hash) {
                return Err(ServiceError::Error(anyhow!(
                    "Transaction {} receipt not found",
                    hex::encode(tx_hash)
                )));
            }
        }

        // Step 3: Build a set of unique block numbers
        let mut block_numbers_set = HashSet::new();
        for log in logs {
            if let Some(bn) = log.block_number {
                if let Some(log_tx_hash) = log.transaction_hash {
                    if missing_hashes.contains(&log_tx_hash) {
                        block_numbers_set.insert(bn);
                    }
                }
            }
        }

        // Sleep in between the above block_receipt calls and the below ones to reduce rate limiting risk.
        tokio::time::sleep(Duration::from_secs(BLOCK_QUERY_SLEEP)).await;

        // Step 4: Fetch block timestamps in parallel and update service map
        if !block_numbers_set.is_empty() {
            let block_numbers_vec: Vec<u64> = block_numbers_set.into_iter().collect();
            
            // Fetch all blocks from RPC in parallel (chunked)
            for chunk in block_numbers_vec.chunks(GET_BLOCK_BY_NUMBER_CHUNK_SIZE) {
                let block_futures: Vec<_> = chunk
                    .iter()
                    .map(|&block_num| {
                        let provider = self.boundless_market.instance().provider();
                        async move {
                            let block = provider
                                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                .await?;
                            Ok::<_, ServiceError>((block_num, block))
                        }
                    })
                    .collect();
                
                let start = std::time::Instant::now();
                let block_results = try_join_all(block_futures).await?;
                tracing::info!("Got {} blocks in {:?}", block_results.len(), start.elapsed());
                for (block_num, block_result) in block_results {
                    if let Some(block) = block_result {
                        let ts = block.header.timestamp;
                        self.block_num_to_timestamp.insert(block_num, ts);
                    }
                }
            }
        }

        // Step 5: Build final map from tx_hash to TxMetadata and update service map
        for &tx_hash in missing_hashes.iter() {
            let (from, tx_index, bn) = receipt_map.get(&tx_hash)
                .copied()
                .context(anyhow!("Receipt not found for transaction {}", hex::encode(tx_hash)))?;
            
            // Get timestamp from service block_timestamps map
            let ts = self.block_num_to_timestamp.get(&bn).copied()
                .context(anyhow!("Block {} timestamp not found in map", bn))?;
            
            let meta = TxMetadata::new(tx_hash, from, bn, ts, tx_index);
            self.tx_hash_to_metadata.insert(tx_hash, meta);
        }

        tracing::info!("fetch_tx_metadata_via_block_receipts completed in {:?}", start.elapsed());
        Ok(())
    }

    // Fetch transaction metadata using eth_getTransactionByHash (fallback method)
    async fn fetch_tx_metadata_via_tx_by_hash(
        &mut self,
        missing_hashes: &[B256],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        // Step 1: Fetch all transactions in parallel and store in a map
        let mut tx_map: HashMap<B256, _> = HashMap::new();
        
        for chunk in missing_hashes.chunks(GET_BLOCK_RECEIPTS_CHUNK_SIZE) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|&tx_hash| {
                    let provider = self.boundless_market.instance().provider();
                    async move {
                        let tx = provider.get_transaction_by_hash(tx_hash).await?;
                        Ok::<_, ServiceError>((tx_hash, tx))
                    }
                })
                .collect();

            let start = std::time::Instant::now();
            let results = try_join_all(futures).await?;
            tracing::info!("Got {} transactions in {:?}", results.len(), start.elapsed());
            for (tx_hash, tx_result) in results {
                match tx_result {
                    Some(tx) => {
                        tx_map.insert(tx_hash, tx);
                    }
                    None => {
                        return Err(ServiceError::Error(anyhow!(
                            "Transaction {} not found",
                            hex::encode(tx_hash)
                        )));
                    }
                }
            }
        }

        // Step 2: Build a set of unique block numbers from all transactions
        let mut block_numbers = HashSet::new();
        for tx in tx_map.values() {
            if let Some(bn) = tx.block_number {
                block_numbers.insert(bn);
            }
        }

        // Sleep in between the above transaction calls and the below block calls to reduce rate limiting risk.
        tokio::time::sleep(Duration::from_secs(BLOCK_QUERY_SLEEP)).await;

        // Step 3: Fetch block timestamps in parallel and update service map
        if !block_numbers.is_empty() {
            let block_numbers_vec: Vec<u64> = block_numbers.into_iter().collect();
            
            // Fetch all blocks from RPC in parallel (chunked)
            for chunk in block_numbers_vec.chunks(GET_BLOCK_BY_NUMBER_CHUNK_SIZE) {
                let block_futures: Vec<_> = chunk
                    .iter()
                    .map(|&block_num| {
                        let provider = self.boundless_market.instance().provider();
                        async move {
                            let block = provider
                                .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                .await?;
                            Ok::<_, ServiceError>((block_num, block))
                        }
                    })
                    .collect();
                
                let start = std::time::Instant::now();
                let block_results = try_join_all(block_futures).await?;
                tracing::info!("Got {} blocks in {:?}", block_results.len(), start.elapsed());
                for (block_num, block_result) in block_results {
                    if let Some(block) = block_result {
                        let ts = block.header.timestamp;
                        self.block_num_to_timestamp.insert(block_num, ts);
                    }
                }
            }
        }

        // Step 4: Build final map from tx_hash to TxMetadata and update service map
        for (tx_hash, tx) in tx_map {
            let bn = tx.block_number.context("block number not found")?;
            let tx_index = tx.transaction_index.context(
                anyhow!("Transaction index not found for transaction {}", hex::encode(tx_hash))
            )?;
            
            // Get timestamp from service block_timestamps map
            let ts = self.block_num_to_timestamp.get(&bn).copied().context(anyhow!("Block {} timestamp not found in map", bn))?;
            
            let from = tx.inner.signer();
            let meta = TxMetadata::new(tx_hash, from, bn, ts, tx_index);
            self.tx_hash_to_metadata.insert(tx_hash, meta);
        }

        tracing::info!("fetch_tx_metadata_via_tx_by_hash completed in {:?}", start.elapsed());
        Ok(())
    }

    // Get transaction metadata for a single log (lookup from cache populated by fetch_tx_metadata)
    async fn get_tx_metadata(&mut self, log: Log) -> Result<TxMetadata, ServiceError> {
        let tx_hash = log.transaction_hash.context("Transaction hash not found")?;
        let meta = self.tx_hash_to_metadata.get(&tx_hash).cloned().ok_or_else(|| ServiceError::Error(anyhow!("Transaction not found: {}", hex::encode(tx_hash))))?;

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

    #[test]
    fn test_get_day_start() {
        // Test various timestamps within a day
        let day_start = 1700000000; // 2023-11-14 22:13:20 UTC
        let day_start_aligned = (day_start / SECONDS_PER_DAY) * SECONDS_PER_DAY;
        
        // All times within the same day should return the same day start
        assert_eq!(get_day_start(day_start), day_start_aligned);
        assert_eq!(get_day_start(day_start + 3600), day_start_aligned); // +1 hour
        assert_eq!(get_day_start(day_start + 86399), day_start_aligned); // last second of day
        
        // Next day should return different start
        assert_eq!(get_day_start(day_start + 86400), day_start_aligned + SECONDS_PER_DAY);
    }

    #[test]
    fn test_get_week_start() {
        use chrono::{Datelike, TimeZone, Weekday};
        
        // Test that weeks start on Monday (ISO 8601)
        // Using a known date: 2023-11-15 is a Wednesday
        let wednesday = 1700000000; // 2023-11-15 00:00:00 UTC (approximately)
        let week_start = get_week_start(wednesday);
        
        // Week start should be a Monday
        let dt = chrono::Utc.timestamp_opt(week_start as i64, 0).unwrap();
        assert_eq!(dt.weekday(), Weekday::Mon);
        
        // All days in the same week should return the same Monday
        let thursday = wednesday + 86400;
        let friday = wednesday + 2 * 86400;
        assert_eq!(get_week_start(thursday), week_start);
        assert_eq!(get_week_start(friday), week_start);
        
        // Sunday should still be in the same week (ISO 8601)
        let sunday = week_start + 6 * 86400;
        assert_eq!(get_week_start(sunday), week_start);
        
        // Next Monday should be a different week
        let next_monday = week_start + 7 * 86400;
        assert_eq!(get_week_start(next_monday), next_monday);
    }

    #[test]
    fn test_get_month_start() {
        use chrono::TimeZone;
        
        // Test mid-month timestamp
        let mid_month = chrono::Utc.with_ymd_and_hms(2023, 11, 15, 12, 30, 45).unwrap();
        let month_start = get_month_start(mid_month.timestamp() as u64);
        
        // Should return 1st of November at 00:00:00
        let expected = chrono::Utc.with_ymd_and_hms(2023, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(month_start, expected.timestamp() as u64);
        
        // Test last day of month
        let end_of_month = chrono::Utc.with_ymd_and_hms(2023, 11, 30, 23, 59, 59).unwrap();
        assert_eq!(get_month_start(end_of_month.timestamp() as u64), month_start);
        
        // Test first day of month
        let first_day = chrono::Utc.with_ymd_and_hms(2023, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(get_month_start(first_day.timestamp() as u64), month_start);
    }

    #[test]
    fn test_get_next_day() {
        let day_start = 1700000000;
        let day_start_aligned = (day_start / SECONDS_PER_DAY) * SECONDS_PER_DAY;
        
        let next_day = get_next_day(day_start_aligned);
        assert_eq!(next_day, day_start_aligned + SECONDS_PER_DAY);
        
        // Should work from any time within the day
        let mid_day = day_start_aligned + 43200; // noon
        assert_eq!(get_next_day(mid_day), day_start_aligned + SECONDS_PER_DAY);
    }

    #[test]
    fn test_get_next_week() {
        let wednesday = 1700000000;
        let week_start = get_week_start(wednesday);
        
        let next_week = get_next_week(wednesday);
        assert_eq!(next_week, week_start + SECONDS_PER_WEEK);
        
        // Should work from any day in the week
        let friday = wednesday + 2 * 86400;
        assert_eq!(get_next_week(friday), week_start + SECONDS_PER_WEEK);
    }

    #[test]
    fn test_get_next_month() {
        use chrono::TimeZone;
        
        // Test November -> December
        let november = chrono::Utc.with_ymd_and_hms(2023, 11, 15, 12, 30, 45).unwrap();
        let next_month = get_next_month(november.timestamp() as u64);
        let expected_dec = chrono::Utc.with_ymd_and_hms(2023, 12, 1, 0, 0, 0).unwrap();
        assert_eq!(next_month, expected_dec.timestamp() as u64);
        
        // Test December -> January (year rollover)
        let december = chrono::Utc.with_ymd_and_hms(2023, 12, 20, 10, 0, 0).unwrap();
        let next_month = get_next_month(december.timestamp() as u64);
        let expected_jan = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(next_month, expected_jan.timestamp() as u64);
    }

    #[test]
    fn test_month_boundaries() {
        use chrono::TimeZone;
        
        // Test months with different numbers of days
        // January (31 days)
        let jan = chrono::Utc.with_ymd_and_hms(2024, 1, 31, 23, 59, 59).unwrap();
        let next = get_next_month(jan.timestamp() as u64);
        let expected_feb = chrono::Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_feb.timestamp() as u64);
        
        // February leap year (29 days)
        let feb = chrono::Utc.with_ymd_and_hms(2024, 2, 29, 12, 0, 0).unwrap();
        let next = get_next_month(feb.timestamp() as u64);
        let expected_mar = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_mar.timestamp() as u64);
        
        // February non-leap year (28 days)
        let feb = chrono::Utc.with_ymd_and_hms(2023, 2, 28, 12, 0, 0).unwrap();
        let next = get_next_month(feb.timestamp() as u64);
        let expected_mar = chrono::Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_mar.timestamp() as u64);
    }
}
