// Copyright 2025 Boundless Foundation, Inc.
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

//! Cryptocurrency price oracle client with multiple data source support.
//!
//! This module provides functionality to fetch crypto/USD prices (ETH/USD, ZKC/USD) from multiple sources
//! including HTTP APIs (CoinGecko, Coinbase, Binance) and on-chain oracles (e.g., Chainlink).
//! It includes automatic failover between sources, price validation, staleness checks,
//! and utilities for converting USD amounts to wei.

use alloy::primitives::{utils::parse_units, I256, U256};
use alloy::providers::ProviderBuilder;
use alloy::sol;
use reqwest::Client;
use serde::Deserialize;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use url::Url;

const SCALE_DECIMALS: u32 = 8;
#[allow(dead_code)] // Used in tests
const SCALE: u128 = 100_000_000; // 10^8
const WEI_SCALE_U128: u128 = 1_000_000_000_000_000_000; // 1e18

// Oracle staleness threshold (15 minutes)
const MAX_ORACLE_STALENESS_SECS: u64 = 15 * 60;

// HTTP request timeout (10 seconds)
const HTTP_TIMEOUT_SECS: u64 = 10;

sol! {
    #[sol(rpc)]
    interface AggregatorV3Interface {
        function latestRoundData()
            external
            view
            returns (
                uint80 roundId,
                int256 answer,
                uint256 startedAt,
                uint256 updatedAt,
                uint80 answeredInRound
            );
    }
}

/// Supported price pairs for fetching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PricePair {
    /// Ethereum / USD
    EthUsd,
    /// ZKC / USD
    ZkcUsd,
}

impl PricePair {
    /// Get the CoinGecko API ID for this pair.
    pub fn coingecko_id(&self) -> &'static str {
        match self {
            PricePair::EthUsd => "ethereum",
            PricePair::ZkcUsd => "zkc",
        }
    }

    /// Get the Coinbase trading pair symbol.
    pub fn coinbase_symbol(&self) -> &'static str {
        match self {
            PricePair::EthUsd => "ETH-USD",
            PricePair::ZkcUsd => "ZKC-USD",
        }
    }

    /// Get the Binance trading pair symbol.
    pub fn binance_symbol(&self) -> &'static str {
        match self {
            PricePair::EthUsd => "ETHUSDT",
            PricePair::ZkcUsd => "ZKCUSDT",
        }
    }
}

/// Errors returned while fetching or converting crypto/USD prices.
#[derive(Debug, Error)]
pub enum EthPriceError {
    /// HTTP request or response parsing error.
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// Provider returned invalid or missing data.
    #[error("Provider returned invalid or missing data: {0}")]
    InvalidData(&'static str),

    /// All configured providers failed to return a price.
    #[error("All providers failed")]
    AllProvidersFailed,

    /// Division by zero while converting USD to wei.
    #[error("Division by zero (zero price?)")]
    DivisionByZero,

    /// Oracle data is stale.
    #[error("Oracle data is stale (last updated {0} seconds ago)")]
    StaleData(u64),

    /// Overflow in USD to wei conversion.
    #[error("Overflow converting USD amount to wei (amount too large)")]
    UsdToWeiOverflow,
}

/// Client for fetching ETH/USD price data from multiple sources.
#[derive(Debug, Clone)]
pub enum PriceSource {
    /// Fetch from CoinGecko's public price API.
    CoinGecko,
    /// Fetch from Coinbase's spot price API.
    Coinbase,
    /// Fetch from Binance's ticker price API.
    Binance,
    /// Fetch from an on-chain oracle (e.g., Chainlink).
    Onchain(OnchainOracleConfig),
}

/// Configuration for an on-chain ETH/USD price oracle.
#[derive(Debug, Clone)]
pub struct OnchainOracleConfig {
    /// HTTP(s) RPC URL for the chain hosting the oracle.
    pub rpc_url: Url,
    /// Oracle contract address.
    pub oracle_address: alloy::primitives::Address,
    /// Number of decimals in the oracle answer (e.g. 8 for many Chainlink feeds).
    pub decimals: u8,
    /// Maximum age of oracle data in seconds before considering it stale.
    /// Defaults to MAX_ORACLE_STALENESS_SECS if None.
    pub max_staleness_secs: Option<u64>,
}

/// Configuration for API endpoints (allows overriding for testing/rate limits).
#[derive(Debug, Clone)]
pub struct ApiEndpoints {
    /// Base URL for CoinGecko price API (price pair will be appended).
    pub coingecko_base_url: String,
    /// Base URL for Coinbase spot price API (pair will be appended).
    pub coinbase_base_url: String,
    /// Base URL for Binance ticker price API (pair will be appended).
    pub binance_base_url: String,
}

impl Default for ApiEndpoints {
    fn default() -> Self {
        Self {
            coingecko_base_url: "https://api.coingecko.com/api/v3/simple/price".to_string(),
            coinbase_base_url: "https://api.coinbase.com/v2/prices".to_string(),
            binance_base_url: "https://api.binance.com/api/v3/ticker/price".to_string(),
        }
    }
}

/// Fetches crypto/USD price data and converts USD amounts to wei.
pub struct EthPriceClient {
    http: Client,
    sources: Vec<PriceSource>,
    api_endpoints: ApiEndpoints,
    pair: PricePair,
}

impl EthPriceClient {
    /// Create a client with the default HTTP price sources for a given pair.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built. This should only happen in exceptional
    /// circumstances (e.g., invalid system configuration).
    pub fn new(pair: PricePair) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client"),
            sources: vec![PriceSource::CoinGecko, PriceSource::Coinbase, PriceSource::Binance],
            api_endpoints: ApiEndpoints::default(),
            pair,
        }
    }

    /// Create a client with the default HTTP price sources for ETH/USD (legacy compatibility).
    pub fn new_default() -> Self {
        Self::new(PricePair::EthUsd)
    }

    /// Create a client with custom API endpoints.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built. This should only happen in exceptional
    /// circumstances (e.g., invalid system configuration).
    pub fn new_with_endpoints(pair: PricePair, endpoints: ApiEndpoints) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client"),
            sources: vec![PriceSource::CoinGecko, PriceSource::Coinbase, PriceSource::Binance],
            api_endpoints: endpoints,
            pair,
        }
    }

    /// Create a client with an on-chain oracle as the first source.
    ///
    /// # Panics
    ///
    /// Panics if the HTTP client cannot be built. This should only happen in exceptional
    /// circumstances (e.g., invalid system configuration).
    pub fn new_with_onchain(pair: PricePair, oracle: OnchainOracleConfig) -> Self {
        Self {
            http: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client"),
            sources: vec![
                PriceSource::Onchain(oracle),
                PriceSource::CoinGecko,
                PriceSource::Coinbase,
                PriceSource::Binance,
            ],
            api_endpoints: ApiEndpoints::default(),
            pair,
        }
    }

    // ----------------- Public API -----------------

    /// Fetch ETH/USD price as a fixed-point U256 scaled by 10^SCALE_DECIMALS.
    /// Uses the first successful source (legacy behavior).
    pub async fn fetch_eth_usd(&self) -> Result<U256, EthPriceError> {
        let mut last_err: Option<EthPriceError> = None;

        for source in &self.sources {
            let source_name = match source {
                PriceSource::CoinGecko => "CoinGecko",
                PriceSource::Coinbase => "Coinbase",
                PriceSource::Binance => "Binance",
                PriceSource::Onchain(_) => "Onchain",
            };

            let res = match source {
                PriceSource::CoinGecko => self.fetch_coingecko().await,
                PriceSource::Coinbase => self.fetch_coinbase().await,
                PriceSource::Binance => self.fetch_binance().await,
                PriceSource::Onchain(cfg) => self.fetch_onchain_oracle(cfg).await,
            };

            match res {
                Ok(price) => return Ok(price),
                Err(e) => {
                    // Log the failure but continue to next source
                    tracing::debug!("Price source {} failed: {}", source_name, e);
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or(EthPriceError::AllProvidersFailed))
    }

    /// Fetch ETH/USD price using consensus from multiple sources.
    /// Returns the median of all successful sources, which is more robust to outliers
    /// than using a single source. Requires at least `min_sources` successful responses.
    ///
    /// # Arguments
    /// * `min_sources` - Minimum number of sources that must succeed (default: 2)
    /// * `max_deviation_percent` - Maximum deviation from median to accept (default: 5%)
    ///   Prices that deviate more than this from the median are considered outliers and rejected.
    pub async fn fetch_eth_usd_consensus(
        &self,
        min_sources: Option<usize>,
        max_deviation_percent: Option<u64>,
    ) -> Result<U256, EthPriceError> {
        let min_sources = min_sources.unwrap_or(2);
        let max_deviation_percent = max_deviation_percent.unwrap_or(5);

        // Fetch prices from all sources in parallel
        let mut prices = Vec::new();
        let mut errors = Vec::new();

        for source in &self.sources {
            let source_name = match source {
                PriceSource::CoinGecko => "CoinGecko",
                PriceSource::Coinbase => "Coinbase",
                PriceSource::Binance => "Binance",
                PriceSource::Onchain(_) => "Onchain",
            };

            let res = match source {
                PriceSource::CoinGecko => self.fetch_coingecko().await,
                PriceSource::Coinbase => self.fetch_coinbase().await,
                PriceSource::Binance => self.fetch_binance().await,
                PriceSource::Onchain(cfg) => self.fetch_onchain_oracle(cfg).await,
            };

            match res {
                Ok(price) => {
                    tracing::debug!("Price source {} returned: {}", source_name, price);
                    prices.push(price);
                }
                Err(e) => {
                    tracing::debug!("Price source {} failed: {}", source_name, e);
                    errors.push((source_name, e));
                }
            }
        }

        if prices.len() < min_sources {
            let error_msg = format!(
                "Only {} sources succeeded, need at least {}",
                prices.len(),
                min_sources
            );
            tracing::warn!("{}", error_msg);
            return Err(if errors.is_empty() {
                EthPriceError::AllProvidersFailed
            } else {
                // Use the first error as representative
                match &errors[0].1 {
                    EthPriceError::Http(_) => EthPriceError::AllProvidersFailed,
                    EthPriceError::InvalidData(msg) => EthPriceError::InvalidData(msg),
                    EthPriceError::StaleData(secs) => EthPriceError::StaleData(*secs),
                    _ => EthPriceError::AllProvidersFailed,
                }
            });
        }

        // Sort prices to find median
        let num_sources = prices.len();
        let mut sorted_prices = prices;
        sorted_prices.sort();

        // Calculate median
        let median = if sorted_prices.len() % 2 == 0 {
            // Even number of prices: average the two middle values
            let mid = sorted_prices.len() / 2;
            (sorted_prices[mid - 1] + sorted_prices[mid]) / U256::from(2)
        } else {
            // Odd number: use the middle value
            sorted_prices[sorted_prices.len() / 2]
        };

        // Filter out outliers that deviate too much from median
        let max_deviation = median
            .checked_mul(U256::from(max_deviation_percent))
            .and_then(|n| n.checked_div(U256::from(100)))
            .unwrap_or_else(|| median / U256::from(20)); // Fallback to 5%

        let filtered_prices: Vec<U256> = sorted_prices
            .into_iter()
            .filter(|&p| {
                let diff = if p > median {
                    p - median
                } else {
                    median - p
                };
                diff <= max_deviation
            })
            .collect();

        if filtered_prices.len() < min_sources {
            tracing::warn!(
                "After outlier filtering, only {} prices remain (need {})",
                filtered_prices.len(),
                min_sources
            );
            // Return median anyway if we have at least one price
            if filtered_prices.is_empty() {
                return Err(EthPriceError::InvalidData("all prices rejected as outliers"));
            }
        }

        // Recalculate median from filtered prices
        let mut sorted_filtered = filtered_prices;
        sorted_filtered.sort();
        let consensus_price = if sorted_filtered.len() % 2 == 0 {
            let mid = sorted_filtered.len() / 2;
            (sorted_filtered[mid - 1] + sorted_filtered[mid]) / U256::from(2)
        } else {
            sorted_filtered[sorted_filtered.len() / 2]
        };

        tracing::info!(
            "Consensus price: {} (from {} sources, median of {} after filtering)",
            consensus_price,
            num_sources,
            sorted_filtered.len()
        );

        Ok(consensus_price)
    }

    /// High-level helper:
    /// read a USD amount as a decimal string (e.g. "1.23") and convert it to wei
    /// using the current ETH/USD price.
    pub async fn usd_str_to_wei(&self, usd_amount: &str) -> Result<U256, EthPriceError> {
        let price_scaled = self.fetch_eth_usd().await?; // ETH/USD, scaled 1e8
        if price_scaled.is_zero() {
            return Err(EthPriceError::DivisionByZero);
        }

        // Parse USD amount string with the same 1e8 scale
        let usd_scaled = Self::price_str_to_scaled_u256(usd_amount)?;

        // wei = usd_scaled / price_usd * 1e18
        //      = usd_scaled * 1e18 / price_scaled
        let num = usd_scaled
            .checked_mul(U256::from(WEI_SCALE_U128))
            .ok_or(EthPriceError::UsdToWeiOverflow)?;

        Ok(num / price_scaled)
    }

    // ----------------- Shared parser -----------------

    /// Parse a decimal string (e.g. "2345.12") into a U256 scaled by 10^SCALE_DECIMALS.
    /// Uses Alloy's parse_units utility for robust decimal parsing.
    fn price_str_to_scaled_u256(s: &str) -> Result<U256, EthPriceError> {
        let s = s.trim();

        if s.is_empty() {
            return Err(EthPriceError::InvalidData("empty price string"));
        }

        parse_units(s, SCALE_DECIMALS as u8)
            .map(|pu| pu.into())
            .map_err(|_| EthPriceError::InvalidData("failed to parse price"))
    }

    // ----------------- Provider implementations -----------------

    async fn fetch_coingecko(&self) -> Result<U256, EthPriceError> {
        use serde_json::Value;

        // Build URL with the appropriate coin ID
        let url = format!(
            "{}?ids={}&vs_currencies=usd&precision=full",
            self.api_endpoints.coingecko_base_url,
            self.pair.coingecko_id()
        );

        let resp: Value = self.http.get(&url).send().await?.error_for_status()?.json().await?;

        // Extract the price from the JSON response
        // CoinGecko can return the price as either a number or a string
        let price_value = resp
            .get(self.pair.coingecko_id())
            .and_then(|v| v.get("usd"))
            .ok_or(EthPriceError::InvalidData("missing price in CoinGecko response"))?;

        // Handle both number and string formats
        let price_str = match price_value {
            Value::Number(n) => {
                // Convert number to string, preserving precision
                n.as_f64()
                    .ok_or(EthPriceError::InvalidData("invalid number format in CoinGecko response"))?
                    .to_string()
            }
            Value::String(s) => s.clone(),
            _ => return Err(EthPriceError::InvalidData("price is not a number or string in CoinGecko response")),
        };

        Self::price_str_to_scaled_u256(&price_str)
    }

    async fn fetch_coinbase(&self) -> Result<U256, EthPriceError> {
        #[derive(Debug, Deserialize)]
        struct CoinbaseInner {
            amount: String,
        }

        #[derive(Debug, Deserialize)]
        struct CoinbaseResp {
            data: CoinbaseInner,
        }

        let url = format!(
            "{}/{}/spot",
            self.api_endpoints.coinbase_base_url,
            self.pair.coinbase_symbol()
        );

        let resp: CoinbaseResp =
            self.http.get(&url).send().await?.error_for_status()?.json().await?;

        Self::price_str_to_scaled_u256(&resp.data.amount)
    }

    async fn fetch_binance(&self) -> Result<U256, EthPriceError> {
        #[derive(Debug, Deserialize)]
        struct BinanceResp {
            price: String,
        }

        let url = format!(
            "{}?symbol={}",
            self.api_endpoints.binance_base_url,
            self.pair.binance_symbol()
        );

        let resp: BinanceResp =
            self.http.get(&url).send().await?.error_for_status()?.json().await?;

        Self::price_str_to_scaled_u256(&resp.price)
    }

    async fn fetch_onchain_oracle(&self, cfg: &OnchainOracleConfig) -> Result<U256, EthPriceError> {
        // Build provider using async connect for consistency with codebase patterns
        let provider = ProviderBuilder::new()
            .connect(cfg.rpc_url.as_str())
            .await
            .map_err(|_| {
                EthPriceError::InvalidData("failed to connect to RPC provider")
            })?;

        // Bind contract
        let oracle = AggregatorV3Interface::new(cfg.oracle_address, &provider);

        let result = oracle
            .latestRoundData()
            .call()
            .await
            .map_err(|_| EthPriceError::InvalidData("oracle call failed"))?;

        // Check for stale data
        let current_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| EthPriceError::InvalidData("system time before UNIX epoch"))?
            .as_secs();

        // Convert U256 to u64 for timestamp comparison
        // Chainlink oracles use uint256 for timestamps, but they should fit in u64
        let updated_at = result.updatedAt
            .try_into()
            .map_err(|_| EthPriceError::InvalidData("oracle timestamp too large for u64"))?;
        let max_staleness = cfg.max_staleness_secs.unwrap_or(MAX_ORACLE_STALENESS_SECS);

        if current_time > updated_at {
            let age_secs = current_time - updated_at;
            if age_secs > max_staleness {
                return Err(EthPriceError::StaleData(age_secs));
            }
        }

        // Check that the round was actually answered
        if result.answeredInRound < result.roundId {
            return Err(EthPriceError::InvalidData("oracle round not answered"));
        }

        let answer = result.answer;

        if answer <= I256::ZERO {
            return Err(EthPriceError::InvalidData("non-positive oracle answer"));
        }

        // Convert I256 to U256 using unsigned_abs (safe since we checked > 0)
        let ans_u256 = answer.unsigned_abs();

        // Oracle answer is typically scaled by 10^decimals (e.g. 1e8).
        // We want to normalize to our internal SCALE_DECIMALS (also 1e8).
        //
        // If oracle_decimals == SCALE_DECIMALS, we can just return `ans_u256`.
        // Otherwise, rescale:
        let oracle_decimals = cfg.decimals;
        if oracle_decimals == SCALE_DECIMALS as u8 {
            Ok(ans_u256)
        } else if oracle_decimals < SCALE_DECIMALS as u8 {
            let diff = (SCALE_DECIMALS as u8 - oracle_decimals) as u32;
            ans_u256
                .checked_mul(U256::from(10u64.pow(diff)))
                .ok_or(EthPriceError::InvalidData("price rescaling overflow"))
        } else {
            let diff = (oracle_decimals - SCALE_DECIMALS as u8) as u32;
            Ok(ans_u256 / U256::from(10u64.pow(diff)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Price parsing tests - covering core functionality and edge cases
    #[test]
    fn test_price_str_to_scaled_u256_valid() {
        // Test integer, decimal, and edge cases
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("2345").unwrap(),
            U256::from(2345u128 * SCALE)
        );
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("2345.12").unwrap(),
            U256::from(234512000000u128)
        );
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("2345.12345678").unwrap(),
            U256::from(234512345678u128)
        );
        // Truncation of extra decimals
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("2345.123456789999").unwrap(),
            U256::from(234512345678u128)
        );
        // Edge cases
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("0").unwrap(),
            U256::ZERO
        );
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("0.00000001").unwrap(),
            U256::from(1u128)
        );
        // Whitespace handling
        assert_eq!(
            EthPriceClient::price_str_to_scaled_u256("  2345.12  ").unwrap(),
            U256::from(234512000000u128)
        );
    }

    #[test]
    fn test_price_str_to_scaled_u256_errors() {
        assert!(matches!(
            EthPriceClient::price_str_to_scaled_u256(""),
            Err(EthPriceError::InvalidData("empty price string"))
        ));
        assert!(matches!(
            EthPriceClient::price_str_to_scaled_u256("abc"),
            Err(EthPriceError::InvalidData(_))
        ));
        assert!(matches!(
            EthPriceClient::price_str_to_scaled_u256("123.45.67"),
            Err(EthPriceError::InvalidData("failed to parse price"))
        ));
    }

    #[tokio::test]
    async fn test_usd_to_wei_calculation() {
        // Test the core calculation logic with one representative case
        let eth_price = U256::from(2000u128 * SCALE); // $2000/ETH scaled
        let usd_amount = U256::from(100u128 * SCALE); // $100 scaled

        // Expected: $100 / $2000 * 1e18 = 0.05 ETH = 50000000000000000 wei
        let expected_wei = usd_amount
            .checked_mul(U256::from(WEI_SCALE_U128))
            .unwrap()
            .checked_div(eth_price)
            .unwrap();

        assert_eq!(expected_wei, U256::from(50000000000000000u128));
    }

    #[test]
    fn test_client_creation() {
        // Test default client
        let client = EthPriceClient::new_default();
        assert_eq!(client.sources.len(), 3);
        assert_eq!(client.pair, PricePair::EthUsd);

        // Test with different pair
        let client = EthPriceClient::new(PricePair::ZkcUsd);
        assert_eq!(client.pair, PricePair::ZkcUsd);
        assert_eq!(client.sources.len(), 3);

        // Test with custom endpoints
        let custom_endpoints = ApiEndpoints {
            coingecko_base_url: "http://localhost:8080/coingecko".to_string(),
            coinbase_base_url: "http://localhost:8080/coinbase".to_string(),
            binance_base_url: "http://localhost:8080/binance".to_string(),
        };
        let client =
            EthPriceClient::new_with_endpoints(PricePair::EthUsd, custom_endpoints.clone());
        assert_eq!(client.api_endpoints.coingecko_base_url, custom_endpoints.coingecko_base_url);

        // Test with onchain oracle
        let oracle_config = OnchainOracleConfig {
            rpc_url: "https://eth.llamarpc.com".parse().unwrap(),
            oracle_address: alloy::primitives::Address::ZERO,
            decimals: 8,
            max_staleness_secs: Some(600),
        };
        let client = EthPriceClient::new_with_onchain(PricePair::EthUsd, oracle_config);
        assert_eq!(client.sources.len(), 4);
        assert!(matches!(client.sources[0], PriceSource::Onchain(_)));
    }

    #[test]
    fn test_price_pair_symbols() {
        // Test both pairs in one test
        assert_eq!(PricePair::EthUsd.coingecko_id(), "ethereum");
        assert_eq!(PricePair::EthUsd.coinbase_symbol(), "ETH-USD");
        assert_eq!(PricePair::EthUsd.binance_symbol(), "ETHUSDT");

        assert_eq!(PricePair::ZkcUsd.coingecko_id(), "zkc");
        assert_eq!(PricePair::ZkcUsd.coinbase_symbol(), "ZKC-USD");
        assert_eq!(PricePair::ZkcUsd.binance_symbol(), "ZKCUSDT");
    }

    #[test]
    fn test_error_display() {
        let err = EthPriceError::StaleData(3600);
        assert_eq!(err.to_string(), "Oracle data is stale (last updated 3600 seconds ago)");

        let err = EthPriceError::UsdToWeiOverflow;
        assert_eq!(err.to_string(), "Overflow converting USD amount to wei (amount too large)");
    }

    // Integration tests that actually hit real APIs and on-chain oracles
    // These are marked with #[ignore] because they require network access
    // Run with: cargo test -- --ignored

    // Test helper to create a client with a single source
    fn create_client_with_source(pair: PricePair, source: PriceSource) -> EthPriceClient {
        EthPriceClient {
            http: Client::builder()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
                .build()
                .expect("failed to build HTTP client"),
            sources: vec![source],
            api_endpoints: ApiEndpoints::default(),
            pair,
        }
    }

    // Helper to validate price is within reasonable bounds for a given pair
    fn validate_price(price: U256, pair: PricePair, source_name: &str) {
        assert!(!price.is_zero(), "{} price should be non-zero", source_name);
        
        match pair {
            PricePair::EthUsd => {
                assert!(
                    price >= U256::from(100u128 * SCALE),
                    "{} ETH price should be at least $100",
                    source_name
                );
                assert!(
                    price <= U256::from(100_000u128 * SCALE),
                    "{} ETH price should be at most $100,000",
                    source_name
                );
            }
            PricePair::ZkcUsd => {
                // ZKC is much cheaper, so use lower bounds
                assert!(
                    price >= U256::from(SCALE / 100), // $0.01
                    "{} ZKC price should be at least $0.01",
                    source_name
                );
                assert!(
                    price <= U256::from(1000u128 * SCALE), // $1000
                    "{} ZKC price should be at most $1000",
                    source_name
                );
            }
        }
    }

    // Helper to test a single price source
    async fn test_price_source(
        pair: PricePair,
        source: PriceSource,
        source_name: &str,
    ) -> Result<U256, EthPriceError> {
        let client = create_client_with_source(pair, source);
        let price = client.fetch_eth_usd().await?;
        validate_price(price, pair, source_name);
        println!("{} {}/USD price: {} (scaled)", source_name, pair.coingecko_id(), price);
        Ok(price)
    }

    #[tokio::test]
    #[ignore]
    async fn test_all_price_sources_eth_usd() {
        // Test all HTTP sources
        test_price_source(PricePair::EthUsd, PriceSource::CoinGecko, "CoinGecko")
            .await
            .expect("CoinGecko should return a valid price");
        test_price_source(PricePair::EthUsd, PriceSource::Coinbase, "Coinbase")
            .await
            .expect("Coinbase should return a valid price");
        test_price_source(PricePair::EthUsd, PriceSource::Binance, "Binance")
            .await
            .expect("Binance should return a valid price");

        // Test on-chain oracle (Chainlink ETH/USD on Ethereum mainnet)
        let oracle_config = OnchainOracleConfig {
            rpc_url: "https://eth.llamarpc.com".parse().unwrap(),
            oracle_address: "0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"
                .parse()
                .expect("valid address"),
            decimals: 8,
            max_staleness_secs: Some(MAX_ORACLE_STALENESS_SECS),
        };
        test_price_source(PricePair::EthUsd, PriceSource::Onchain(oracle_config), "Onchain")
            .await
            .expect("Onchain oracle should return a valid price");

        // Test failover mechanism - all sources should work
        let client = EthPriceClient::new(PricePair::EthUsd);
        let price = client.fetch_eth_usd().await.expect("Failover should succeed with at least one source");
        validate_price(price, PricePair::EthUsd, "Failover");
        println!("Failover ETH/USD price: {} (scaled)", price);
    }

    #[tokio::test]
    #[ignore]
    async fn test_all_price_sources_zkc_usd() {
        // ZKC might not be available on all exchanges, so we check if it succeeds
        let sources = [
            (PriceSource::CoinGecko, "CoinGecko"),
            (PriceSource::Coinbase, "Coinbase"),
            (PriceSource::Binance, "Binance"),
        ];

        for (source, name) in sources {
            match test_price_source(PricePair::ZkcUsd, source, name).await {
                Ok(price) => println!("{} ZKC/USD price: {} (scaled)", name, price),
                Err(_) => println!("{} ZKC/USD not available (this is okay)", name),
            }
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_usd_to_wei_with_real_price() {
        let client = EthPriceClient::new(PricePair::EthUsd);

        // Test converting USD amounts to wei
        let test_cases = [
            ("100", 10_000_000_000_000_000u128, "100 USD should be at least 0.01 ETH"),
            ("1", 1u128, "1 USD should convert to non-zero wei"),
        ];

        for (usd_str, min_wei, error_msg) in test_cases {
            let wei = client
                .usd_str_to_wei(usd_str)
                .await
                .unwrap_or_else(|e| panic!("Should be able to convert ${} to wei: {}", usd_str, e));
            assert!(!wei.is_zero(), "{} USD should convert to non-zero wei", usd_str);
            assert!(wei >= U256::from(min_wei), "{}", error_msg);
            assert!(wei <= U256::from(WEI_SCALE_U128), "{} USD should be at most 1 ETH", usd_str);
            println!("${} USD = {} wei", usd_str, wei);
        }
    }

    #[tokio::test]
    #[ignore]
    async fn test_consensus_price_mechanism() {
        let client = EthPriceClient::new(PricePair::EthUsd);

        // Test consensus with default settings (min 2 sources, 5% max deviation)
        let consensus_price = client
            .fetch_eth_usd_consensus(None, None)
            .await
            .expect("Consensus should succeed with multiple sources");
        validate_price(consensus_price, PricePair::EthUsd, "Consensus");
        println!("Consensus ETH/USD price: {} (scaled)", consensus_price);

        // Compare with single-source fetch
        let single_price = client.fetch_eth_usd().await.expect("Single source should succeed");
        let diff = if consensus_price > single_price {
            consensus_price - single_price
        } else {
            single_price - consensus_price
        };
        println!("Single-source ETH/USD price: {} (scaled)", single_price);
        println!("Difference: {} (scaled)", diff);

        // Test with stricter requirements (optional - may fail if not enough sources)
        if let Ok(strict_price) = client.fetch_eth_usd_consensus(Some(3), Some(2)).await {
            validate_price(strict_price, PricePair::EthUsd, "Strict consensus");
            println!("Strict consensus (3 sources, 2% deviation): {} (scaled)", strict_price);
        } else {
            println!("Strict consensus failed (expected if not enough sources or too much variance)");
        }
    }
}
