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

//! Integration tests for price oracle (run with --ignored flag)
//! These tests make real network calls to APIs and RPC endpoints
//!
//! Run with:
//! ```bash
//! RUST_LOG=debug ETH_RPC_URL="https://..." CMC_API_KEY="your_key" \
//!   cargo test -p boundless-market price_oracle::integration_tests -- --ignored --test-threads=1 --nocapture
//! ```

#![cfg(test)]

use crate::price_oracle::{
    config::{
        ChainlinkConfig, CoinGeckoConfig, CoinMarketCapConfig, OffChainConfig, OnChainConfig,
        PriceOracleConfig, PriceValue,
    },
    AggregationMode, ExchangeRate, TradingPair,
};
use alloy::providers::ProviderBuilder;
use alloy_chains::NamedChain;

// Price bounds for sanity checks
const ETH_MIN_USD: f64 = 100.0;
const ETH_MAX_USD: f64 = 10000.0;
const ZKC_MIN_USD: f64 = 0.001;
const ZKC_MAX_USD: f64 = 10.0;

/// Get RPC URL from environment or fallback to public RPC
fn get_rpc_url() -> String {
    std::env::var("ETH_RPC_URL").unwrap_or_else(|_| "https://ethereum.publicnode.com".to_string())
}

/// Get CoinMarketCap API key from environment
fn get_cmc_api_key() -> Option<String> {
    std::env::var("CMC_API_KEY").ok()
}

/// Assert price is within reasonable bounds for the trading pair
fn assert_price_reasonable(quote: ExchangeRate, pair: TradingPair) {
    let price_usd = quote.rate_to_f64();

    let (min, max, pair_name) = match pair {
        TradingPair::EthUsd => (ETH_MIN_USD, ETH_MAX_USD, "ETH/USD"),
        TradingPair::ZkcUsd => (ZKC_MIN_USD, ZKC_MAX_USD, "ZKC/USD"),
    };

    assert!(
        price_usd >= min && price_usd <= max,
        "{} price ${:.2} out of reasonable range [${:.2}, ${:.2}]",
        pair_name,
        price_usd,
        min,
        max
    );
}

/// Assert timestamp is recent (within max_age_secs)
fn assert_timestamp_recent(timestamp: u64, max_age_secs: u64) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    assert!(
        timestamp > now.saturating_sub(max_age_secs),
        "Timestamp {} is too old (now: {}, max age: {}s)",
        timestamp,
        now,
        max_age_secs
    );
}

/// Build a config with all 3 sources (Chainlink, CoinGecko, CMC)
fn build_config(mode: AggregationMode) -> PriceOracleConfig {
    let cmc_api_key = get_cmc_api_key()
        .expect("CMC_API_KEY environment variable is required for integration tests");

    PriceOracleConfig {
        eth_usd: PriceValue::Auto,
        zkc_usd: PriceValue::Auto,
        refresh_interval_secs: 60,
        timeout_secs: 10,
        aggregation_mode: mode,
        min_sources: 1,
        max_secs_without_price_update: 600,
        onchain: Some(OnChainConfig { chainlink: Some(ChainlinkConfig { enabled: true }) }),
        offchain: Some(OffChainConfig {
            coingecko: Some(CoinGeckoConfig { enabled: true, api_key: None }),
            cmc: Some(CoinMarketCapConfig { enabled: true, api_key: cmc_api_key }),
        }),
    }
}

#[tokio::test]
#[ignore] // Requires network and CMC_API_KEY
async fn test_composite_priority_mode() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    let rpc_url = get_rpc_url();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let config = build_config(AggregationMode::Priority);
    let oracles = config.build(NamedChain::Mainnet, provider)?;

    // Fetch ETH/USD price
    let quote = oracles.get_rate(TradingPair::EthUsd).await?;
    println!("Priority mode ETH/USD: ${:.2}", quote.rate_to_f64());
    assert_price_reasonable(quote, TradingPair::EthUsd);
    assert_timestamp_recent(quote.timestamp, 3600);

    // Fetch ZKC/USD price
    let quote_zkc = oracles.get_rate(TradingPair::ZkcUsd).await?;
    println!("Priority mode ZKC/USD: ${:.6}", quote_zkc.rate_to_f64());
    assert_price_reasonable(quote_zkc, TradingPair::ZkcUsd);
    assert_timestamp_recent(quote_zkc.timestamp, 3600);

    Ok(())
}

#[tokio::test]
#[ignore] // Requires network and CMC_API_KEY
async fn test_composite_median_mode() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    let rpc_url = get_rpc_url();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let config = build_config(AggregationMode::Median);
    let oracles = config.build(NamedChain::Mainnet, provider)?;

    // Fetch ETH/USD price
    let quote = oracles.get_rate(TradingPair::EthUsd).await?;
    println!("Median mode ETH/USD: ${:.2}", quote.rate_to_f64());
    assert_price_reasonable(quote, TradingPair::EthUsd);
    assert_timestamp_recent(quote.timestamp, 3600);

    // Fetch ZKC/USD price
    let quote_zkc = oracles.get_rate(TradingPair::ZkcUsd).await?;
    println!("Median mode ZKC/USD: ${:.6}", quote_zkc.rate_to_f64());
    assert_price_reasonable(quote_zkc, TradingPair::ZkcUsd);
    assert_timestamp_recent(quote_zkc.timestamp, 3600);

    Ok(())
}

#[tokio::test]
#[ignore] // Requires network and CMC_API_KEY
async fn test_composite_average_mode() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    let rpc_url = get_rpc_url();
    let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

    let config = build_config(AggregationMode::Average);
    let oracles = config.build(NamedChain::Mainnet, provider)?;

    // Fetch ETH/USD price
    let quote = oracles.get_rate(TradingPair::EthUsd).await?;
    println!("Average mode ETH/USD: ${:.2}", quote.rate_to_f64());
    assert_price_reasonable(quote, TradingPair::EthUsd);
    assert_timestamp_recent(quote.timestamp, 3600);

    Ok(())
}

#[tokio::test]
#[ignore] // Requires network and CMC_API_KEY
async fn test_all_aggregation_modes_price_consistency() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    let rpc_url = get_rpc_url();

    // Test all three modes
    let modes = [AggregationMode::Priority, AggregationMode::Median, AggregationMode::Average];

    let mut prices = Vec::new();

    for mode in modes {
        let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);
        let config = build_config(mode);
        let oracles = config.build(NamedChain::Mainnet, provider)?;

        let quote = oracles.get_rate(TradingPair::EthUsd).await?;
        println!("{:?} mode ETH/USD: ${:.2}", mode, quote.rate_to_f64());

        assert_price_reasonable(quote, TradingPair::EthUsd);
        prices.push(quote.rate_to_f64());
    }

    // All prices should be relatively close (within 10% of each other)
    let max_price = prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min_price = prices.iter().cloned().fold(f64::INFINITY, f64::min);
    let spread_pct = (max_price - min_price) / min_price * 100.0;

    println!("Price spread: {:.2}% (min: ${:.2}, max: ${:.2})", spread_pct, min_price, max_price);

    assert!(spread_pct < 10.0, "Price spread {:.2}% exceeds 10% threshold", spread_pct);

    Ok(())
}
