use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote, TradingPair};
use alloy::primitives::{address, Address, U256};
use alloy::providers::Provider;
use alloy::sol;
use alloy_chains::NamedChain;
use crate::price_oracle::sources::{scale_price_from_i256, PriceSource};

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
    // TODO: add mock tests

    use super::*;
    use alloy::providers::ProviderBuilder;

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