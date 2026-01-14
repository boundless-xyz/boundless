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

use std::{
    collections::{hash_map, HashMap, HashSet},
    future::Future,
    pin::Pin,
};

use alloy::{eips::BlockNumberOrTag, primitives::U256, providers::Provider};
use alloy_primitives::{address, Address};
use anyhow::{bail, Context, Result};
use derive_builder::Builder;
use futures::future::try_join_all;
use rand::Rng;
use risc0_zkvm::serde::from_slice;
use url::Url;

use crate::{
    client::ClientBuilder,
    contracts::RequestInputType,
    indexer_client::{AggregationGranularity, IndexerClient},
    Deployment, GuestEnv, RequestId, RequestInput,
};

/// Price percentiles (p10 and p99) for lock price per cycle
#[derive(Debug, Clone)]
pub struct PricePercentiles {
    /// 10th percentile lock price per cycle (in wei)
    pub p10: U256,
    /// 25th percentile lock price per cycle (in wei)
    pub p25: U256,
    /// 50th percentile (median) lock price per cycle (in wei)
    pub p50: U256,
    /// 75th percentile lock price per cycle (in wei)
    pub p75: U256,
    /// 90th percentile lock price per cycle (in wei)
    pub p90: U256,
    /// 95th percentile lock price per cycle (in wei)
    pub p95: U256,
    /// 99th percentile lock price per cycle (in wei)
    pub p99: U256,
}

/// Trait for providers that can supply market price ranges.
///
/// This trait allows for flexible integration with different price data sources,
/// making it easy to test or swap implementations.
pub trait PriceProvider {
    /// Fetch the current market price percentiles.
    ///
    /// # Returns
    ///
    /// Returns a boxed future that resolves to a `Result<PricePercentiles>` containing
    /// the 10th, 25th, 50th, 75th, 90th, 95th, and 99th percentiles. Returns an error if the price data
    /// cannot be fetched or parsed.
    ///
    /// Note: The future must be Send to allow use in multi-threaded contexts
    /// like `tokio::spawn`.
    fn price_percentiles(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PricePercentiles>> + Send + '_>>;
}

/// Type alias for a thread-safe, shareable price provider.
///
/// This is the standard type used throughout the codebase for storing and passing
/// price providers, as it satisfies the `Send + Sync` requirements needed for
/// use in multi-threaded contexts and `Arc` sharing.
pub type PriceProviderArc = std::sync::Arc<dyn PriceProvider + Send + Sync>;

/// Standard price provider that uses a default and an optional fallback price provider.
pub struct StandardPriceProvider<
    D: PriceProvider + Clone + Send + 'static,
    F: PriceProvider + Clone + Send + 'static,
> {
    default: D,
    fallback: Option<F>,
}

impl<D: PriceProvider + Clone + Send + 'static, F: PriceProvider + Clone + Send + 'static>
    StandardPriceProvider<D, F>
{
    /// Create a new standard price provider.
    ///
    /// # Parameters
    ///
    /// * `default`: The default price provider to use for fetching prices.
    /// * `fallback`: The fallback price provider to use as a fallback.
    ///
    /// # Returns
    ///
    /// A new [StandardPriceProvider].
    ///
    /// # Example
    ///
    /// ```rust
    /// use boundless_market::price_provider::{StandardPriceProvider, IndexerClient};
    /// use boundless_market::indexer_client::IndexerClient;
    /// use url::Url;
    ///
    /// let indexer_client = IndexerClient::new(Url::parse("https://indexer.boundless.com").unwrap());
    /// let price_provider = StandardPriceProvider::new(indexer_client);
    /// ```
    ///
    pub fn new(default: D) -> Self {
        Self { default, fallback: None }
    }

    /// Set the fallback price provider.
    ///
    /// # Parameters
    ///
    /// * `fallback`: The fallback price provider to use as a fallback.
    ///
    /// # Returns
    ///
    /// A new [StandardPriceProvider] with the fallback price provider set.
    ///
    /// # Example
    ///
    /// ```rust
    /// use boundless_market::price_provider::{StandardPriceProvider, IndexerClient, MarketPricing};
    /// use boundless_market::indexer_client::IndexerClient;
    /// use boundless_market::market_pricing::MarketPricing;
    /// use url::Url;
    ///
    /// let indexer_client = IndexerClient::new(Url::parse("https://indexer.boundless.com").unwrap());
    /// let market_pricing = MarketPricing::new(Url::parse("https://sepolia.rpc.com").unwrap(), MarketPricingConfig::default());
    /// let price_provider = StandardPriceProvider::new(indexer_client).with_fallback(market_pricing);
    /// ```
    ///
    pub fn with_fallback(self, fallback: F) -> Self {
        Self { default: self.default, fallback: Some(fallback) }
    }
}

impl<D: PriceProvider + Clone + Send + 'static, F: PriceProvider + Clone + Send + 'static>
    PriceProvider for StandardPriceProvider<D, F>
{
    fn price_percentiles(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PricePercentiles>> + Send + '_>> {
        let default = self.default.clone();
        let fallback = self.fallback.clone();
        Box::pin(async move {
            // Await the default future first, moving it into the async block
            let default_result = {
                let default_fut = default.price_percentiles();
                default_fut.await
            };
            match default_result {
                Ok(price_percentiles) => Ok(price_percentiles),
                Err(e) => {
                    if let Some(fallback_provider) = fallback {
                        // Await the fallback future, moving it into the async block
                        let fallback_fut = fallback_provider.price_percentiles();
                        fallback_fut.await
                    } else {
                        Err(e)
                    }
                }
            }
        })
    }
}

impl PriceProvider for IndexerClient {
    fn price_percentiles(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PricePercentiles>> + Send + '_>> {
        let client = self.clone();
        Box::pin(async move { client.get_prices_percentiles(AggregationGranularity::Weekly).await })
    }
}

impl PriceProvider for MarketPricing {
    fn price_percentiles(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<PricePercentiles>> + Send + '_>> {
        Box::pin(self.clone().query_market_pricing())
    }
}

#[non_exhaustive]
#[derive(Clone, Debug)]
enum RequestorType {
    OrderGenerator,
    Signal,
}

#[derive(Clone, Debug)]
struct RequestorMap {
    requestors: hash_map::HashMap<Address, RequestorType>,
}

impl RequestorMap {
    fn new(requestors: hash_map::HashMap<Address, RequestorType>) -> Self {
        Self { requestors }
    }

    fn count_cycles(&self, address: Address, input: RequestInput) -> Option<u64> {
        const SIGNAL_REQUESTOR_MIN_CYCLES: u64 = 50_000_000_000;
        const SIGNAL_REQUESTOR_MAX_CYCLES: u64 = 54_000_000_000;

        let requestor_type = self.requestors.get(&address);
        match requestor_type {
            Some(RequestorType::OrderGenerator) => try_extract_cycle_count(&input),
            Some(RequestorType::Signal) => {
                let mut rng = rand::rng();
                let random_cycles =
                    rng.random_range(SIGNAL_REQUESTOR_MIN_CYCLES..=SIGNAL_REQUESTOR_MAX_CYCLES);
                Some(random_cycles)
            }
            None => None,
        }
    }
}

/// Market pricing provider that uses an RPC URL and a configuration.
///
/// # Parameters
///
/// * `rpc_url`: The RPC URL to use for connecting to the Boundless Market.
/// * `config`: The configuration for the market pricing provider.
///
/// # Returns
///
/// A new [MarketPricing].
///
/// # Example
///
/// ```
/// use boundless_market::price_provider::{MarketPricing, MarketPricingConfig};
/// use url::Url;
///
/// let market_pricing = MarketPricing::new(Url::parse("https://sepolia.rpc.com").unwrap(), MarketPricingConfig::default());
/// ```
///
#[derive(Clone, Debug)]
pub struct MarketPricing {
    /// The RPC URL to use for connecting to the Boundless Market.
    pub rpc_url: Url,
    /// The configuration for the market pricing provider.
    pub config: MarketPricingConfig,
}

/// Configuration for the market pricing provider.
///
/// # Parameters
///
/// * `deployment`: The deployment to use for the market pricing provider.
/// * `event_query_chunk_size`: The size of the chunk to use for querying events.
/// * `market_price_blocks_to_query`: The number of blocks to query for market prices.
/// * `tx_timeout`: The timeout for the market pricing provider.
///
/// # Returns
///
/// A new [MarketPricingConfig].
///
/// # Example
///
/// ```
/// use boundless_market::price_provider::{MarketPricingConfig};
/// use boundless_market::Deployment;
///
/// let config = MarketPricingConfig::builder()
///     .deployment(Deployment::default())
///     .event_query_chunk_size(100)
///     .market_price_blocks_to_query(30000)
///     .tx_timeout(std::time::Duration::from_secs(300))
///     .build()
///     .unwrap();
/// ```
///
#[non_exhaustive]
#[derive(Clone, Builder, Debug)]
pub struct MarketPricingConfig {
    /// The deployment to use for the market pricing provider.
    #[builder(setter(into, strip_option), default)]
    deployment: Option<Deployment>,
    /// The size of the chunk to use for querying events.
    #[builder(default = "100")]
    event_query_chunk_size: u64,
    /// The number of blocks to query for market prices.
    #[builder(default = "30000")]
    market_price_blocks_to_query: u64,
    /// The timeout for the market pricing provider.
    #[builder(default = "std::time::Duration::from_secs(300)")]
    timeout: std::time::Duration,
}

impl MarketPricingConfig {
    /// Create a new market pricing configuration builder.
    ///
    /// # Returns
    ///
    /// A new [MarketPricingConfigBuilder].
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::price_provider::{MarketPricingConfig, MarketPricingConfigBuilder};
    /// use boundless_market::Deployment;
    ///
    /// let config = MarketPricingConfig::builder()
    ///     .deployment(Deployment::default())
    ///     .event_query_chunk_size(100)
    ///     .market_price_blocks_to_query(30000)
    ///     .timeout(std::time::Duration::from_secs(300))
    ///     .build()
    ///     .unwrap();
    /// ```
    ///
    pub fn builder() -> MarketPricingConfigBuilder {
        MarketPricingConfigBuilder::default()
    }
}

impl Default for MarketPricingConfig {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for MarketPricingConfig")
    }
}

impl MarketPricing {
    /// Create a new market pricing provider.
    ///
    /// # Parameters
    ///
    /// * `rpc_url`: The RPC URL to use for connecting to the Boundless Market.
    /// * `config`: The configuration for the market pricing provider.
    ///
    /// # Returns
    ///
    /// A new [MarketPricing].
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::price_provider::{MarketPricing, MarketPricingConfig};
    /// use url::Url;
    ///
    /// let config = MarketPricingConfig::builder()
    ///     .deployment(Deployment::default())
    ///     .event_query_chunk_size(100)
    ///     .market_price_blocks_to_query(30000)
    ///     .timeout(std::time::Duration::from_secs(300))
    ///     .build()
    ///     .unwrap();
    /// let market_pricing = MarketPricing::new(Url::parse("https://sepolia.rpc.com").unwrap(), config);
    /// ```
    ///
    pub fn new(rpc_url: Url, config: MarketPricingConfig) -> Self {
        Self { rpc_url, config }
    }

    // Get a list of known requestors for the deployment.
    // This is used to filter the market events for pricing.
    fn known_requestors(&self) -> Option<RequestorMap> {
        let deployment = self.config.deployment.as_ref()?;
        match deployment.market_chain_id {
            Some(11155111) => Some(RequestorMap::new(hash_map::HashMap::from([
                (
                    address!("0xc197ebe12c7bcf1d9f3b415342bdbc795425335c"),
                    RequestorType::OrderGenerator,
                ),
                (
                    address!("0xe198c6944cae382902a375b0b8673084270a7f8e"),
                    RequestorType::OrderGenerator,
                ),
            ]))),
            Some(8453) => Some(RequestorMap::new(hash_map::HashMap::from([
                (
                    address!("0xc197ebe12c7bcf1d9f3b415342bdbc795425335c"),
                    RequestorType::OrderGenerator,
                ),
                (
                    address!("0xe198c6944cae382902a375b0b8673084270a7f8e"),
                    RequestorType::OrderGenerator,
                ),
                (address!("0x734df7809c4ef94da037449c287166d114503198"), RequestorType::Signal),
            ]))),
            Some(84532) => Some(RequestorMap::new(hash_map::HashMap::from([
                (
                    address!("0xc197ebe12c7bcf1d9f3b415342bdbc795425335c"),
                    RequestorType::OrderGenerator,
                ),
                (
                    address!("0xe198c6944cae382902a375b0b8673084270a7f8e"),
                    RequestorType::OrderGenerator,
                ),
            ]))),
            _ => None,
        }
    }

    /// Query market prices from the Boundless Market.
    async fn query_market_pricing(self) -> Result<PricePercentiles> {
        let Some(requestor_list) = self.known_requestors() else {
            bail!("No known requestors for deployment");
        };
        // Build market client
        let timeout = self.config.timeout;
        let client = ClientBuilder::new()
            .with_rpc_url(self.rpc_url.clone())
            .with_timeout(timeout)
            .with_deployment(self.config.deployment.clone())
            .build()
            .await
            .context("Failed to create market client")?;

        // Get current block and calculate range
        let current_block = client.provider().get_block_number().await?;
        let start_block = current_block.saturating_sub(self.config.market_price_blocks_to_query);

        // Query RequestLocked events in chunks
        let mut locked_logs = Vec::new();
        let mut chunk_start = start_block;
        while chunk_start < current_block {
            let chunk_end = (chunk_start + self.config.event_query_chunk_size).min(current_block);

            let locked_filter = client
                .boundless_market
                .instance()
                .RequestLocked_filter()
                .from_block(chunk_start)
                .to_block(chunk_end);

            let mut chunk_logs =
                locked_filter.query().await.context("Failed to query RequestLocked events")?;

            locked_logs.append(&mut chunk_logs);
            chunk_start = chunk_end + 1;
        }

        // Query RequestFulfilled events in chunks
        let mut fulfilled_logs = Vec::new();
        let mut chunk_start = start_block;
        while chunk_start < current_block {
            let chunk_end = (chunk_start + self.config.event_query_chunk_size).min(current_block);

            let fulfilled_filter = client
                .boundless_market
                .instance()
                .RequestFulfilled_filter()
                .from_block(chunk_start)
                .to_block(chunk_end);

            let mut chunk_logs = fulfilled_filter
                .query()
                .await
                .context("Failed to query RequestFulfilled events")?;

            fulfilled_logs.append(&mut chunk_logs);
            chunk_start = chunk_end + 1;
        }

        // Build map of fulfilled requests with their block numbers
        let mut fulfilled_map: HashMap<U256, u64> = HashMap::new();
        for (event, log_meta) in &fulfilled_logs {
            let request_id: U256 = event.requestId;
            if let Some(block_number) = log_meta.block_number {
                fulfilled_map.insert(request_id, block_number);
            }
        }

        // Check if all logs have block_timestamp available. Some RPC providers don't return
        // timestamps for eth_getLogs queries.
        let all_logs_have_timestamp = fulfilled_logs.iter().all(|(_, log_meta)| {
            log_meta.block_number.is_some() && log_meta.block_timestamp.is_some()
        }) && locked_logs.iter().all(|(_, log_meta)| {
            log_meta.block_number.is_some() && log_meta.block_timestamp.is_some()
        });

        // Build block_timestamps map
        let mut block_timestamps: HashMap<u64, u64> = HashMap::new();

        if all_logs_have_timestamp {
            // Use timestamps directly from log metadata - no RPC calls needed
            for (_, log_meta) in &fulfilled_logs {
                if let (Some(block_num), Some(timestamp)) =
                    (log_meta.block_number, log_meta.block_timestamp)
                {
                    block_timestamps.insert(block_num, timestamp);
                }
            }
            for (_, log_meta) in &locked_logs {
                if let (Some(block_num), Some(timestamp)) =
                    (log_meta.block_number, log_meta.block_timestamp)
                {
                    block_timestamps.insert(block_num, timestamp);
                }
            }
        } else {
            // Some logs are missing timestamps, fetch them via concurrent RPC calls
            // Collect unique block numbers from both event types
            let mut block_numbers = HashSet::new();
            for (_, log_meta) in &fulfilled_logs {
                if let Some(block_num) = log_meta.block_number {
                    block_numbers.insert(block_num);
                }
            }
            for (_, log_meta) in &locked_logs {
                if let Some(block_num) = log_meta.block_number {
                    block_numbers.insert(block_num);
                }
            }

            if !block_numbers.is_empty() {
                let block_numbers: Vec<_> = block_numbers.into_iter().collect();

                // Fetch timestamps for blocks using concurrent futures
                // Process in chunks to avoid overwhelming the RPC
                const CHUNK_SIZE: usize = 100;
                for chunk in block_numbers.chunks(CHUNK_SIZE) {
                    let provider = client.provider();
                    let futures: Vec<_> = chunk
                        .iter()
                        .map(|&block_num| {
                            let provider = provider.clone();
                            async move {
                                let block = provider
                                    .get_block_by_number(BlockNumberOrTag::Number(block_num))
                                    .await?;
                                Ok::<_, anyhow::Error>((block_num, block))
                            }
                        })
                        .collect();

                    let results = try_join_all(futures).await?;

                    // Process results
                    for (block_num, block) in results {
                        match block {
                            Some(block) => {
                                block_timestamps.insert(block_num, block.header.timestamp);
                            }
                            None => {
                                bail!("Block {} not found", block_num);
                            }
                        }
                    }
                }
            }
        }

        // Process locked orders
        let mut prices_per_cycle = Vec::new();

        let requestors = requestor_list.requestors.clone();
        for (event, log_meta) in &locked_logs {
            let request_id: U256 = event.requestId;
            let requestor = RequestId::from_lossy(event.request.id).addr;

            // Filter: only priority requestors
            if !requestors.contains_key(&requestor) {
                continue;
            }

            // Filter: only fulfilled orders - get block number and then timestamp
            let fulfilled_block_number = match fulfilled_map.get(&request_id) {
                Some(&block_num) => block_num,
                None => continue,
            };
            let fulfilled_timestamp = match block_timestamps.get(&fulfilled_block_number) {
                Some(&timestamp) => timestamp,
                None => continue,
            };

            // Get block timestamp for locked order from fetched timestamps
            let lock_block_number = match log_meta.block_number {
                Some(block_num) => block_num,
                None => continue,
            };
            let lock_timestamp = match block_timestamps.get(&lock_block_number) {
                Some(&timestamp) => timestamp,
                None => continue,
            };

            // Calculate lock deadline
            let lock_deadline = event.request.offer.lock_deadline();

            // Filter: only orders fulfilled before lockExpiry
            if fulfilled_timestamp > lock_deadline {
                continue;
            }

            // Calculate price at lock time using Offer.price_at
            let locked_price = match event.request.offer.price_at(lock_timestamp) {
                Ok(price) => price,
                Err(_) => {
                    bail!(
                        "Failed to calculate price at lock time for request ID 0x{:x}",
                        request_id
                    );
                }
            };

            // Extract or estimate cycle count
            let cycles = match requestor_list.count_cycles(requestor, event.request.input.clone()) {
                Some(cycles) => cycles,
                None => continue,
            };

            // Calculate price per cycle
            if locked_price > U256::ZERO {
                let price_per_cycle = locked_price / U256::from(cycles);
                prices_per_cycle.push(price_per_cycle);
            }
        }

        if prices_per_cycle.is_empty() {
            bail!("No valid market data found for pricing");
        }

        // Sort prices for percentile calculations
        prices_per_cycle.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = prices_per_cycle.len() as f64;
        // Calculate (10th percentile)
        let percentile_10_idx = (n * 0.1) as usize;
        let percentile_10 = prices_per_cycle[percentile_10_idx.min(n as usize - 1)];

        // Calculate 25th percentile
        let percentile_25_idx = (n * 0.25) as usize;
        let percentile_25 = prices_per_cycle[percentile_25_idx.min(n as usize - 1)];

        // Calculate 50th percentile
        let percentile_50_idx = (n * 0.5) as usize;
        let percentile_50 = prices_per_cycle[percentile_50_idx.min(n as usize - 1)];

        // Calculate 75th percentile
        let percentile_75_idx = (n * 0.75) as usize;
        let percentile_75 = prices_per_cycle[percentile_75_idx.min(n as usize - 1)];

        // Calculate 90th percentile
        let percentile_90_idx = (n * 0.9) as usize;
        let percentile_90 = prices_per_cycle[percentile_90_idx.min(n as usize - 1)];

        // Calculate 95th percentile
        let percentile_95_idx = (n * 0.95) as usize;
        let percentile_95 = prices_per_cycle[percentile_95_idx.min(n as usize - 1)];

        // Calculate 99th percentile
        let percentile_99_idx = (n * 0.99) as usize;
        let percentile_99 = prices_per_cycle[percentile_99_idx.min(n as usize - 1)];

        Ok(PricePercentiles {
            p10: percentile_10,
            p25: percentile_25,
            p50: percentile_50,
            p75: percentile_75,
            p90: percentile_90,
            p95: percentile_95,
            p99: percentile_99,
        })
    }
}

fn try_extract_cycle_count(input: &RequestInput) -> Option<u64> {
    // Check if inline input
    if input.inputType != RequestInputType::Inline {
        tracing::debug!("Skipping URL-based input for cycle count extraction");
        return None;
    }

    // Decode GuestEnv
    match GuestEnv::decode(&input.data) {
        Ok(guest_env) => {
            // Convert stdin bytes to u32 words for risc0 deserialization
            // risc0 serde uses u32 words, need to convert from bytes
            match bytemuck::try_cast_slice::<u8, u32>(&guest_env.stdin) {
                Ok(words) => {
                    // Decode first u64 from stdin words
                    match from_slice::<u64, u32>(words) {
                        Ok(cycle_count) => {
                            tracing::trace!("Successfully decoded cycle count: {}", cycle_count);
                            Some(cycle_count)
                        }
                        Err(e) => {
                            tracing::debug!("Failed to decode cycle count from stdin: {}", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("Failed to convert stdin bytes to u32 words: {}", e);
                    None
                }
            }
        }
        Err(e) => {
            tracing::debug!("Failed to decode GuestEnv: {}", e);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_pricing_config_default() {
        let config = MarketPricingConfig::default();
        assert_eq!(config.event_query_chunk_size, 100);
        assert_eq!(config.market_price_blocks_to_query, 30000);
        assert_eq!(config.timeout, std::time::Duration::from_secs(300));
    }
}
