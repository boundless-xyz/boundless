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

use crate::price_oracle::{
    scale_price_from_i256, ExchangeRate, PriceOracle, PriceOracleError, TradingPair,
};
use alloy::primitives::{address, Address};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use alloy_chains::NamedChain;
use core::time::Duration;

sol! {
    #[sol(rpc)]
    interface AggregatorV3Interface {
        function latestRoundData() external view returns (
            uint80 roundId,
            int256 answer,
            uint256 startedAt,
            uint256 updatedAt,
            uint80 answeredInRound
        );
    }
}

/// Chainlink ETH/USD feed on Ethereum mainnet.
const ETH_USD_MAINNET_FEED: Address = address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419");
const ETH_USD_MAINNET_DECIMALS: u8 = 8;

/// Chainlink feed information
#[derive(Debug, Clone, Copy)]
pub struct FeedInfo {
    /// Feed contract address
    pub address: Address,
    /// Number of decimals in the feed price
    pub decimals: u8,
}

impl FeedInfo {
    /// Create a new FeedInfo
    pub const fn new(address: Address, decimals: u8) -> Self {
        Self { address, decimals }
    }
}

/// Chainlink price source (single provider)
pub struct ChainlinkSource<P> {
    pair: TradingPair,
    provider: P,
    feed: FeedInfo,
}

/// Get the Chainlink ETH/USD feed info for a named chain (if available).
/// Returns `None` for chains without a native Chainlink deployment.
fn eth_usd_feed(chain: NamedChain) -> Option<FeedInfo> {
    match chain {
        NamedChain::Mainnet => Some(FeedInfo::new(ETH_USD_MAINNET_FEED, ETH_USD_MAINNET_DECIMALS)),
        NamedChain::Sepolia => {
            Some(FeedInfo::new(address!("0x694AA1769357215DE4FAC081bf1f309aDC325306"), 8))
        }
        NamedChain::Base => {
            Some(FeedInfo::new(address!("0x71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70"), 8))
        }
        NamedChain::BaseSepolia => {
            Some(FeedInfo::new(address!("0x4aDC67696bA383F43DD60A9e78F2C97Fbbfc7cb1"), 8))
        }
        _ => None,
    }
}

impl<P> ChainlinkSource<P> {
    /// Create a new Chainlink source with a custom feed
    pub fn new(pair: TradingPair, provider: P, feed: FeedInfo) -> Self {
        Self { pair, provider, feed }
    }

    /// Create a Chainlink source for ETH/USD using the chain's native feed (if available).
    /// Returns `None` for chains without a Chainlink deployment (e.g. Taiko).
    pub fn for_eth_usd(provider: P, chain: NamedChain) -> Option<Self> {
        eth_usd_feed(chain).map(|feed| Self { pair: TradingPair::EthUsd, provider, feed })
    }
}

impl<P: Provider + Clone> ChainlinkSource<P> {
    async fn fetch_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        let contract = AggregatorV3Interface::new(self.feed.address, &self.provider);

        let round_data = contract.latestRoundData().call().await?;

        let rate = scale_price_from_i256(round_data.answer, self.feed.decimals as u32)?;

        let timestamp: u64 = round_data
            .updatedAt
            .try_into()
            .map_err(|_| PriceOracleError::Internal("timestamp conversion failed".to_string()))?;

        Ok(ExchangeRate::new(self.pair, rate, timestamp))
    }
}

#[async_trait::async_trait]
impl<P: Provider + Clone> PriceOracle for ChainlinkSource<P> {
    fn pair(&self) -> TradingPair {
        self.pair
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        self.fetch_rate().await
    }

    fn name(&self) -> String {
        format!("ChainlinkSource({})", self.pair)
    }
}

/// Provider type used by the mainnet fallback source.
type MainnetProvider = alloy::providers::fillers::FillProvider<
    alloy::providers::fillers::JoinFill<
        alloy::providers::Identity,
        alloy::providers::fillers::ChainIdFiller,
    >,
    alloy::providers::RootProvider,
>;

fn build_mainnet_provider(
    rpc_url: &str,
    timeout: Duration,
) -> Result<MainnetProvider, PriceOracleError> {
    let url: url::Url = rpc_url.parse().map_err(|e| {
        PriceOracleError::ConfigError(format!("Invalid Chainlink RPC URL '{rpc_url}': {e}"))
    })?;
    let client = alloy::transports::http::reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| PriceOracleError::ConfigError(format!("Failed to build HTTP client: {e}")))?;
    Ok(ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(alloy::providers::fillers::ChainIdFiller::default())
        .connect_reqwest(client, url))
}

/// Public Ethereum mainnet RPC endpoints for the Chainlink mainnet fallback source.
/// User-configured `rpc_url` is prepended to this list. Tried in order on failure.
pub const CHAINLINK_PUBLIC_RPC_URLS: &[&str] = &[
    "https://ethereum-rpc.publicnode.com",
    "https://mainnet.gateway.tenderly.co",
    "https://1rpc.io/eth",
    "https://eth.drpc.org",
    "https://eth.merkle.io",
    "https://eth.llamarpc.com",
];

/// Chainlink ETH/USD source that reads from Ethereum mainnet with RPC fallback.
///
/// Tries each configured RPC URL in order; returns the first successful response.
pub struct ChainlinkMainnetSource {
    sources: Vec<ChainlinkSource<MainnetProvider>>,
}

impl ChainlinkMainnetSource {
    /// Create a Chainlink source for ETH/USD using multiple Ethereum mainnet RPC URLs.
    pub fn eth_usd_mainnet(
        rpc_urls: &[String],
        timeout: Duration,
    ) -> Result<Self, PriceOracleError> {
        if rpc_urls.is_empty() {
            return Err(PriceOracleError::ConfigError(
                "At least one Chainlink RPC URL is required".to_string(),
            ));
        }
        let feed = FeedInfo::new(ETH_USD_MAINNET_FEED, ETH_USD_MAINNET_DECIMALS);
        let sources = rpc_urls
            .iter()
            .map(|url| {
                let provider = build_mainnet_provider(url, timeout)?;
                Ok(ChainlinkSource::new(TradingPair::EthUsd, provider, feed))
            })
            .collect::<Result<Vec<_>, PriceOracleError>>()?;
        Ok(Self { sources })
    }
}

#[async_trait::async_trait]
impl PriceOracle for ChainlinkMainnetSource {
    fn pair(&self) -> TradingPair {
        TradingPair::EthUsd
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        let mut last_err = None;
        for source in &self.sources {
            match source.fetch_rate().await {
                Ok(rate) => return Ok(rate),
                Err(e) => {
                    tracing::debug!("Chainlink RPC failed, trying next: {e}");
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            PriceOracleError::Internal("No Chainlink RPC URLs configured".to_string())
        }))
    }

    fn name(&self) -> String {
        format!("ChainlinkMainnet(ETH/USD, {} RPCs)", self.sources.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{I256, U256};
    use alloy::providers::ProviderBuilder;
    use httpmock::prelude::*;

    /// Helper to encode latestRoundData response
    /// Returns: (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound)
    fn encode_latest_round_data(
        round_id: u64,
        answer: i128,
        started_at: u64,
        updated_at: u64,
        answered_in_round: u64,
    ) -> String {
        use alloy::sol_types::SolValue;

        let round_id_u80 = U256::from(round_id);
        let answer_i256 = I256::try_from(answer).unwrap();
        let started_at_u256 = U256::from(started_at);
        let updated_at_u256 = U256::from(updated_at);
        let answered_in_round_u80 = U256::from(answered_in_round);

        let tuple =
            (round_id_u80, answer_i256, started_at_u256, updated_at_u256, answered_in_round_u80);
        let encoded = tuple.abi_encode();
        format!("0x{}", hex::encode(encoded))
    }

    #[tokio::test]
    async fn test_eth_usd_price_success() {
        let server = MockServer::start();

        // Mock the eth_call for latestRoundData
        // ETH price: $2500.50 with 8 decimals = 250050000000
        let encoded = encode_latest_round_data(
            1234,         // roundId
            250050000000, // answer (price with 8 decimals)
            1706547100,   // startedAt
            1706547200,   // updatedAt
            1234,         // answeredInRound
        );

        server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": encoded
                }),
            );
        });

        let provider = ProviderBuilder::new().connect_http(server.base_url().parse().unwrap());

        let feed = FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8);
        let source = ChainlinkSource::new(TradingPair::EthUsd, provider, feed);

        let rate = source.get_rate().await.unwrap();

        assert_eq!(rate.pair, TradingPair::EthUsd);
        assert_eq!(rate.rate, U256::from(250050000000u128));
        assert_eq!(rate.timestamp, 1706547200);
    }

    #[tokio::test]
    async fn test_handles_rpc_error() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {
                        "code": -32000,
                        "message": "execution reverted"
                    }
                }),
            );
        });

        let provider = ProviderBuilder::new().connect_http(server.base_url().parse().unwrap());

        let feed = FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8);
        let source = ChainlinkSource::new(TradingPair::EthUsd, provider, feed);

        let result = source.get_rate().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mainnet_source_single_url() {
        let server = MockServer::start();

        let encoded = encode_latest_round_data(1234, 250050000000, 1706547100, 1706547200, 1234);

        server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": encoded }));
        });

        let source =
            ChainlinkMainnetSource::eth_usd_mainnet(&[server.base_url()], Duration::from_secs(5))
                .unwrap();
        let rate = source.get_rate().await.unwrap();
        assert_eq!(rate.rate, U256::from(250050000000u128));
    }

    #[tokio::test]
    async fn test_mainnet_source_fallback() {
        let bad_server = MockServer::start();
        let good_server = MockServer::start();

        bad_server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(500);
        });

        let encoded = encode_latest_round_data(1234, 250050000000, 1706547100, 1706547200, 1234);
        good_server.mock(|when, then| {
            when.method(POST).path("/");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": encoded }));
        });

        let source = ChainlinkMainnetSource::eth_usd_mainnet(
            &[bad_server.base_url(), good_server.base_url()],
            Duration::from_secs(5),
        )
        .unwrap();
        let rate = source.get_rate().await.unwrap();
        assert_eq!(rate.rate, U256::from(250050000000u128));
    }

    #[tokio::test]
    async fn test_mainnet_source_empty_urls() {
        let result = ChainlinkMainnetSource::eth_usd_mainnet(&[], Duration::from_secs(5));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_rpc_url() {
        let result = ChainlinkMainnetSource::eth_usd_mainnet(
            &["not a url".to_string()],
            Duration::from_secs(5),
        );
        assert!(result.is_err());
    }

    // Integration test (requires network access)
    #[tokio::test]
    #[ignore]
    async fn test_mainnet_eth_usd_price() -> anyhow::Result<()> {
        let source = ChainlinkMainnetSource::eth_usd_mainnet(
            &[
                "https://ethereum-rpc.publicnode.com".to_string(),
                "https://eth.drpc.org".to_string(),
            ],
            Duration::from_secs(10),
        )?;

        let rate = source.get_rate().await?;
        println!("{:?}", rate);

        assert!(rate.rate > U256::ZERO);
        assert!(rate.timestamp > 0);

        Ok(())
    }
}
