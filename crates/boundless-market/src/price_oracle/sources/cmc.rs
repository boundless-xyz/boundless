use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote, TradingPair};
use alloy::primitives::U256;
use reqwest::Client;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};
use url::Url;
use crate::price_oracle::sources::{scale_price, PriceSource};

#[derive(Deserialize, Debug)]
struct CmcResponse {
    data: HashMap<String, CmcCoinData>,
}

#[derive(Deserialize, Debug)]
struct CmcCoinData {
    quote: HashMap<String, CmcQuote>,
}

#[derive(Deserialize, Debug)]
struct CmcQuote {
    price: f64,
    last_updated: String,
}

/// CoinMarketCap price source
pub struct CoinMarketCapSource {
    client: Client,
    api_url: Url,
    api_key: String,
}

impl CoinMarketCapSource {
    /// Create a new CoinMarketCap source
    pub fn new(api_key: String, timeout: Duration) -> Result<Self, PriceOracleError> {
        let api_url = Url::parse("https://pro-api.coinmarketcap.com").unwrap();

        let client = Client::builder()
            .timeout(timeout)
            .build()?;

        Ok(Self {
            client,
            api_url,
            api_key,
        })
    }

    async fn fetch_price(&self, path: &str, id: &str, convert: &str) -> Result<PriceQuote, PriceOracleError> {
        let mut url = self.api_url.clone();
        url.set_path(path);
        url.query_pairs_mut()
            .append_pair("id", id)
            .append_pair("convert", convert);

        let response = self.client
            .get(url)
            .header("X-CMC_PRO_API_KEY", &self.api_key)
            .send()
            .await?
            .error_for_status()?;

        let data: CmcResponse = response.json().await?;

        let coin_data = data.data
            .get(id)
            .ok_or_else(|| PriceOracleError::Internal(format!("id {} not found in response", id)))?;

        let quote = coin_data.quote
            .get(convert)
            .ok_or_else(|| PriceOracleError::Internal(format!("currency {} not found in quote", convert)))?;

        let price = scale_price(quote.price)?;

        // Parse ISO 8601 timestamp to unix timestamp
        let timestamp = chrono::DateTime::parse_from_rfc3339(&quote.last_updated)
            .map_err(|e| PriceOracleError::Internal(format!("failed to parse timestamp: {}", e)))?
            .timestamp() as u64;

        Ok(PriceQuote::new(price, timestamp))
    }
}

impl PriceSource for CoinMarketCapSource {
    fn name(&self) -> &'static str {
        "CoinMarketCap"
    }
}

#[async_trait::async_trait]
impl PriceOracle for CoinMarketCapSource {
    async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
        match pair {
            TradingPair::EthUsd => self.fetch_price("/v2/cryptocurrency/quotes/latest","1027", "USD").await,
            TradingPair::ZkcUsd => self.fetch_price("/v2/cryptocurrency/quotes/latest","38371", "USD").await,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore]
    async fn test_api_eth_price() -> anyhow::Result<()> {
        let api_key = std::env::var("CMC_API_KEY")
            .expect("CMC_API_KEY env var required");

        let source = CoinMarketCapSource::new(api_key, Duration::from_secs(10))?;

        let quote = source.get_price(TradingPair::EthUsd).await?;

        println!("{:?}", quote);

        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_zkc_price() -> anyhow::Result<()> {
        let api_key = std::env::var("CMC_API_KEY")
            .expect("CMC_API_KEY env var required");

        let source = CoinMarketCapSource::new(api_key, Duration::from_secs(10))?;

        let quote = source.get_price(TradingPair::ZkcUsd).await?;

        println!("{:?}", quote);

        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }
}