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
use alloy::providers::Provider;
use alloy::sol;
use alloy_chains::NamedChain;

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

/// Get the Chainlink ETH/USD feed info for a named chain
fn eth_usd_feed(chain: NamedChain) -> Result<FeedInfo, PriceOracleError> {
    match chain {
        NamedChain::Mainnet => {
            Ok(FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8))
        }
        NamedChain::Sepolia => {
            Ok(FeedInfo::new(address!("0x694AA1769357215DE4FAC081bf1f309aDC325306"), 8))
        }
        NamedChain::Base => {
            Ok(FeedInfo::new(address!("0x71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70"), 8))
        }
        NamedChain::BaseSepolia => {
            Ok(FeedInfo::new(address!("0x4aDC67696bA383F43DD60A9e78F2C97Fbbfc7cb1"), 8))
        }
        _ => Err(PriceOracleError::Internal(format!("unsupported chain: {:?}", chain))),
    }
}

/// Chainlink price source
pub struct ChainlinkSource<P> {
    pair: TradingPair,
    provider: P,
    feed: FeedInfo,
}

impl<P> ChainlinkSource<P> {
    /// Create a new Chainlink source with a custom feed
    pub fn new(pair: TradingPair, provider: P, feed: FeedInfo) -> Self {
        Self { pair, provider, feed }
    }

    /// Create a new Chainlink source for ETH/USD on a named chain
    pub fn for_eth_usd(provider: P, chain: NamedChain) -> Result<Self, PriceOracleError> {
        let feed = eth_usd_feed(chain)?;
        Ok(Self { pair: TradingPair::EthUsd, provider, feed })
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

    // Integration test (requires RPC URL and network access)
    #[tokio::test]
    #[ignore]
    async fn test_mainnet_eth_usd_price() -> anyhow::Result<()> {
        let rpc_url = std::env::var("ETH_RPC_URL").expect("ETH_RPC_URL env var required");

        let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

        let source = ChainlinkSource::for_eth_usd(provider, NamedChain::Mainnet)?;

        let rate = source.get_rate().await?;

        println!("{:?}", rate);

        assert!(rate.rate > U256::ZERO);
        assert!(rate.timestamp > 0);

        Ok(())
    }
}
