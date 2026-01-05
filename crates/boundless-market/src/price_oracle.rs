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
        let price_str = resp
            .get(self.pair.coingecko_id())
            .and_then(|v| v.get("usd"))
            .and_then(|v| v.as_str())
            .ok_or(EthPriceError::InvalidData("missing price in CoinGecko response"))?;

        Self::price_str_to_scaled_u256(price_str)
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
}
