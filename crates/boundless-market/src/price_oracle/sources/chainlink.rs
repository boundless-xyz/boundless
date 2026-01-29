use crate::price_oracle::{scale_price_from_i256, PriceOracle, PriceOracleError, PriceQuote, PriceSource, TradingPair};
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
        NamedChain::Mainnet => Ok(FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8)),
        NamedChain::Base => Ok(FeedInfo::new(address!("0x71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70"), 8)),
        _ => Err(PriceOracleError::Internal(format!("unsupported chain: {:?}", chain))),
    }
}

/// Chainlink price source
pub struct ChainlinkSource<P> {
    provider: P,
    eth_usd_feed: FeedInfo,
}

impl<P> ChainlinkSource<P> {
    /// Create a new Chainlink source with a custom feed
    pub fn new(provider: P, eth_usd_feed: FeedInfo) -> Self {
        Self { provider, eth_usd_feed }
    }

    /// Create a new Chainlink source for a named chain
    pub fn for_chain(provider: P, chain: NamedChain) -> Result<Self, PriceOracleError> {
        let eth_usd_feed = eth_usd_feed(chain)?;
        Ok(Self { provider, eth_usd_feed })
    }
}

impl<P: Provider + Clone> ChainlinkSource<P> {
    async fn fetch_price(&self, feed: FeedInfo) -> Result<PriceQuote, PriceOracleError> {
        let contract = AggregatorV3Interface::new(feed.address, &self.provider);

        let round_data = contract
            .latestRoundData()
            .call()
            .await?;

        let price = scale_price_from_i256(round_data.answer, feed.decimals as u32)?;

        let timestamp: u64 = round_data
            .updatedAt
            .try_into()
            .map_err(|_| PriceOracleError::Internal("timestamp conversion failed".to_string()))?;

        Ok(PriceQuote::new(price, timestamp))
    }
}

impl<P: Provider + Clone> PriceSource for ChainlinkSource<P> {
    fn name(&self) -> &'static str {
        "Chainlink"
    }
}

#[async_trait::async_trait]
impl<P: Provider + Clone> PriceOracle for ChainlinkSource<P> {
    async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
        match pair {
            TradingPair::EthUsd => self.fetch_price(self.eth_usd_feed).await,
            TradingPair::ZkcUsd => Err(PriceOracleError::UnsupportedPair(TradingPair::ZkcUsd)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{U256, I256};
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

        let tuple = (round_id_u80, answer_i256, started_at_u256, updated_at_u256, answered_in_round_u80);
        let encoded = tuple.abi_encode();
        format!("0x{}", hex::encode(encoded))
    }

    #[tokio::test]
    async fn test_eth_usd_price_success() {
        let server = MockServer::start();

        // Mock the eth_call for latestRoundData
        // ETH price: $2500.50 with 8 decimals = 250050000000
        let encoded = encode_latest_round_data(
            1234,           // roundId
            250050000000,   // answer (price with 8 decimals)
            1706547100,     // startedAt
            1706547200,     // updatedAt
            1234,           // answeredInRound
        );

        server.mock(|when, then| {
            when.method(POST)
                .path("/");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "result": encoded
                }));
        });

        let provider = ProviderBuilder::new()
            .connect_http(server.base_url().parse().unwrap());

        let feed = FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8);
        let source = ChainlinkSource::new(provider, feed);

        let quote = source.get_price(TradingPair::EthUsd).await.unwrap();

        assert_eq!(quote.price, U256::from(250050000000u128));
        assert_eq!(quote.timestamp, 1706547200);
    }

    #[tokio::test]
    async fn test_handles_rpc_error() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(POST)
                .path("/");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "error": {
                        "code": -32000,
                        "message": "execution reverted"
                    }
                }));
        });

        let provider = ProviderBuilder::new()
            .connect_http(server.base_url().parse().unwrap());

        let feed = FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8);
        let source = ChainlinkSource::new(provider, feed);

        let result = source.get_price(TradingPair::EthUsd).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_unsupported_pair_zkc() {
        let server = MockServer::start();

        let provider = ProviderBuilder::new()
            .connect_http(server.base_url().parse().unwrap());

        let feed = FeedInfo::new(address!("0x5f4eC3Df9cbd43714FE2740f5E3616155c5b8419"), 8);
        let source = ChainlinkSource::new(provider, feed);

        let result = source.get_price(TradingPair::ZkcUsd).await;
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::UnsupportedPair(_))));
    }

    // Integration test (requires RPC URL and network access)
    #[tokio::test]
    #[ignore]
    async fn test_mainnet_eth_usd_price() -> anyhow::Result<()> {
        let rpc_url = std::env::var("ETH_RPC_URL").expect("ETH_RPC_URL env var required");

        let provider = ProviderBuilder::new().connect_http(rpc_url.parse()?);

        let source = ChainlinkSource::for_chain(provider, NamedChain::Mainnet)?;

        let quote = source.get_price(TradingPair::EthUsd).await?;

        println!("{:?}", quote);

        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }
}