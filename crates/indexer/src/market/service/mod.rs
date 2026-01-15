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

mod aggregation;
mod chain_data;
mod core;
mod cycle_counts;
mod execution;
mod log_processors;
mod status;

// Re-export aggregation helper for use in backfill
pub use aggregation::sum_hourly_aggregates_into_base;

// Re-export execute_requests for integration tests
pub use execution::execute_requests;

use std::{collections::HashMap, sync::Arc};

use crate::{
    db::{market::MarketDb, DbError, DbObj, TxMetadata},
    market::caching::CacheStorage,
};
use ::boundless_market::contracts::{
    boundless_market::{BoundlessMarketService, MarketError},
    EIP712DomainSaltless, IBoundlessMarket,
};
use ::boundless_market::order_stream_client::OrderStreamClient;
use alloy::{
    network::AnyNetwork,
    primitives::{Address, B256},
    providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::local::PrivateKeySigner,
    sol_types::SolEvent,
    transports::{RpcError, TransportErrorKind},
};
use anyhow::anyhow;
use thiserror::Error;
use tokio::time::Duration;
use url::Url;

pub const SECONDS_PER_HOUR: u64 = 3600;
pub const SECONDS_PER_DAY: u64 = 86400;
pub const SECONDS_PER_WEEK: u64 = 604800;
// TODO: Debug why all times don't work with hourly aggregation recompute hours of 2.
const HOURLY_AGGREGATION_RECOMPUTE_HOURS: u64 = 6;
const DAILY_AGGREGATION_RECOMPUTE_DAYS: u64 = 2;
const WEEKLY_AGGREGATION_RECOMPUTE_WEEKS: u64 = 2;
const MONTHLY_AGGREGATION_RECOMPUTE_MONTHS: u64 = 2;
const GET_BLOCK_RECEIPTS_CHUNK_SIZE: usize = 250;
const GET_BLOCK_BY_NUMBER_CHUNK_SIZE: usize = 250;
const BLOCK_QUERY_SLEEP: u64 = 1;

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
type AnyNetworkProvider = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<
                alloy::providers::fillers::GasFiller,
                JoinFill<
                    alloy::providers::fillers::BlobGasFiller,
                    JoinFill<alloy::providers::fillers::NonceFiller, ChainIdFiller>,
                >,
            >,
        >,
        ChainIdFiller,
    >,
    RootProvider<AnyNetwork>,
    AnyNetwork,
>;

#[derive(Error, Debug)]
pub enum ServiceError {
    #[error("Database error: {0}")]
    DatabaseError(#[from] DbError),

    #[error("Database query error in {1}: {0}")]
    DatabaseQueryError(DbError, String),

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

pub trait DbResultExt<T> {
    fn with_db_context(self, context: &str) -> Result<T, ServiceError>;
}

impl<T> DbResultExt<T> for Result<T, DbError> {
    fn with_db_context(self, context: &str) -> Result<T, ServiceError> {
        self.map_err(|e| ServiceError::DatabaseQueryError(e, context.to_string()))
    }
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
    pub aggregation_interval: Duration,
    pub retries: u32,
    pub batch_size: u64,
    pub cache_uri: Option<String>,
    pub tx_fetch_strategy: TransactionFetchStrategy,
    pub execution_config: Option<IndexerServiceExecutionConfig>,
}

#[derive(Clone)]
pub struct IndexerServiceExecutionConfig {
    pub execution_interval: Duration,
    pub bento_api_url: String,
    pub bento_api_key: String,
    pub bento_retry_count: u64,
    pub bento_retry_sleep_ms: u64,
    pub max_concurrent_executing: u32,
    pub max_status_queries: u32,
    pub max_iterations: u32,
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
        let db: DbObj = Arc::new(MarketDb::new(db_conn, None, false).await?);
        let domain = boundless_market.eip712_domain().await?;
        let tx_hash_to_metadata = HashMap::new();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get chain id: {}", e)))?;

        // Initialize cache storage if URI provided
        let cache_storage = if let Some(uri) = &config.cache_uri {
            match crate::market::caching::cache_storage_from_uri(uri).await {
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

        Ok(Self {
            boundless_market,
            provider,
            any_network_provider,
            logs_provider,
            db,
            domain,
            config,
            tx_hash_to_metadata,
            block_num_to_timestamp: HashMap::new(),
            order_stream_client: None,
            cache_storage,
            chain_id,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn new_with_order_stream(
        rpc_url: Url,
        logs_rpc_url: Url,
        private_key: &PrivateKeySigner,
        boundless_market_address: Address,
        db_conn: &str,
        config: IndexerServiceConfig,
        order_stream_url: Url,
        order_stream_api_key: Option<String>,
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
        let db: DbObj = Arc::new(MarketDb::new(db_conn, None, false).await?);
        let domain = boundless_market.eip712_domain().await?;
        let tx_hash_to_metadata = HashMap::new();
        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| ServiceError::Error(anyhow!("Failed to get chain id: {}", e)))?;
        let order_stream_client = Some(match order_stream_api_key {
            Some(api_key) => OrderStreamClient::new_with_api_key(
                order_stream_url,
                boundless_market_address,
                chain_id,
                api_key,
            ),
            None => OrderStreamClient::new(order_stream_url, boundless_market_address, chain_id),
        });

        // Initialize cache storage if URI provided
        let cache_storage = if let Some(uri) = &config.cache_uri {
            match crate::market::caching::cache_storage_from_uri(uri).await {
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
            tracing::debug!("No cache storage configured. Using in-memory cache only.");
            None
        };

        Ok(Self {
            boundless_market,
            any_network_provider,
            provider,
            logs_provider,
            db,
            domain,
            config,
            tx_hash_to_metadata,
            block_num_to_timestamp: HashMap::new(),
            order_stream_client,
            cache_storage,
            chain_id,
        })
    }
}
