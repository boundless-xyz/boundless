use crate::price_oracle::{scale_price_from_f64, PriceOracle, PriceOracleError, PriceQuote, PriceSource, TradingPair};
use alloy::primitives::U256;
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;
use url::Url;

#[derive(Deserialize)]
struct CoinGeckoPriceData {
    usd: f64,
    last_updated_at: u64,
}

type CoinGeckoPriceResponse = HashMap<String, CoinGeckoPriceData>;

/// CoinGecko price source
pub struct CoinGeckoSource {
    client: Client,
    api_url: Url,
}

impl CoinGeckoSource {
    /// Create a new CoinGecko source
    pub fn new(timeout: Duration) -> Result<Self, PriceOracleError> {
        let api_url = Url::parse("https://api.coingecko.com").unwrap();

        let client = Client::builder()
            .timeout(timeout)
            // CoinGecko requires a user-agent header for the free API
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .build()?;

        Ok(Self {
            client,
            api_url,
        })
    }

    async fn fetch_price(&self, path: &str, ids: &str, vs_currencies: &str) -> Result<PriceQuote, PriceOracleError> {
        let mut url = self.api_url.clone();
        url.set_path(path);
        url.query_pairs_mut()
            .append_pair("ids", ids)
            .append_pair("vs_currencies", vs_currencies)
            .append_pair("include_last_updated_at", "true");

        let response = self.client.get(url).send().await?.error_for_status()?;

        let data: CoinGeckoPriceResponse = response.json().await?;
        let coin_data = data
            .get(ids).ok_or_else(|| PriceOracleError::Internal(format!("coin {} not found in response", ids)))?;

        let price = scale_price_from_f64(coin_data.usd)?;

        Ok(PriceQuote::new(price, coin_data.last_updated_at))
    }
}

impl PriceSource for CoinGeckoSource {
    fn name(&self) -> &'static str {
        "CoinGecko"
    }
}

#[async_trait::async_trait]
impl PriceOracle for CoinGeckoSource {
    async fn get_price(
        &self,
        pair: TradingPair,
    ) -> Result<PriceQuote, PriceOracleError> {
        match pair {
            TradingPair::EthUsd => self.fetch_price("/api/v3/simple/price", "ethereum", "usd").await,
            TradingPair::ZkcUsd => self.fetch_price("/api/v3/simple/price", "boundless", "usd").await,
        }
    }
}

#[cfg(test)]
mod tests {
    // TODO: add mock tests
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_api_eth_price() -> anyhow::Result<()> {
        let source = CoinGeckoSource::new(
            Duration::from_secs(10),
        )?;

        let quote = source
            .get_price(TradingPair::EthUsd)
            .await?;

        println!("{:?}", quote);

        // Sanity check: ETH should be worth something
        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_zkc_price() -> anyhow::Result<()> {
        let source = CoinGeckoSource::new(
            Duration::from_secs(10),
        )?;

        let quote = source
            .get_price(TradingPair::ZkcUsd)
            .await?;

        println!("{:?}", quote);

        // Sanity check: ZKC should be worth something
        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }
}
