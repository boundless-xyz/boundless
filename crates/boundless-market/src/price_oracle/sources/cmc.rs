use crate::price_oracle::{
    scale_price_from_f64, PriceOracle, PriceOracleError, PriceQuote, PriceSource, TradingPair,
};
use reqwest::Client;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};
use url::Url;

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
    pair: TradingPair,
}

impl CoinMarketCapSource {
    /// Create a new CoinMarketCap source for a specific trading pair
    pub fn new(
        pair: TradingPair,
        api_key: String,
        timeout: Duration,
    ) -> Result<Self, PriceOracleError> {
        let api_url = Url::parse("https://pro-api.coinmarketcap.com").unwrap();

        let client = Client::builder().timeout(timeout).build()?;

        Ok(Self { client, api_url, api_key, pair })
    }

    /// Configure the API URL for testing
    pub fn with_api_url(mut self, url: Url) -> Self {
        self.api_url = url;
        self
    }

    async fn fetch_price(
        &self,
        path: &str,
        id: &str,
        convert: &str,
    ) -> Result<PriceQuote, PriceOracleError> {
        let mut url = self.api_url.clone();
        url.set_path(path);
        url.query_pairs_mut().append_pair("id", id).append_pair("convert", convert);

        let response = self
            .client
            .get(url)
            .header("X-CMC_PRO_API_KEY", &self.api_key)
            .send()
            .await?
            .error_for_status()?;

        let data: CmcResponse = response.json().await?;

        let coin_data = data.data.get(id).ok_or_else(|| {
            PriceOracleError::Internal(format!("id {} not found in response", id))
        })?;

        let quote = coin_data.quote.get(convert).ok_or_else(|| {
            PriceOracleError::Internal(format!("currency {} not found in quote", convert))
        })?;

        let price = scale_price_from_f64(quote.price)?;

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
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
        match self.pair {
            TradingPair::EthUsd => {
                self.fetch_price("/v2/cryptocurrency/quotes/latest", "1027", "USD").await
            }
            TradingPair::ZkcUsd => {
                self.fetch_price("/v2/cryptocurrency/quotes/latest", "38371", "USD").await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use httpmock::prelude::*;

    #[tokio::test]
    async fn test_eth_usd_price_success() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v2/cryptocurrency/quotes/latest")
                .query_param("id", "1027")
                .query_param("convert", "USD")
                .header("X-CMC_PRO_API_KEY", "test-api-key");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "data": {
                        "1027": {
                            "quote": {
                                "USD": {
                                    "price": 2500.50,
                                    "last_updated": "2026-01-29T12:10:00.000Z"
                                }
                            }
                        }
                    }
                }),
            );
        });

        let source = CoinMarketCapSource::new(
            TradingPair::EthUsd,
            "test-api-key".to_string(),
            Duration::from_secs(10),
        )
        .unwrap()
        .with_api_url(server.base_url().parse().unwrap());

        let quote = source.get_price().await.unwrap();

        mock.assert();
        assert_eq!(quote.price, U256::from(250050000000u128)); // 2500.50 * 1e8
                                                               // Verify timestamp is parsed correctly from ISO 8601
        assert_eq!(quote.timestamp, 1769688600);
    }

    #[tokio::test]
    async fn test_zkc_usd_price_success() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v2/cryptocurrency/quotes/latest")
                .query_param("id", "38371")
                .query_param("convert", "USD")
                .header("X-CMC_PRO_API_KEY", "test-api-key");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "data": {
                        "38371": {
                            "quote": {
                                "USD": {
                                    "price": 0.123456,
                                    "last_updated": "2026-01-29T12:30:00.000Z"
                                }
                            }
                        }
                    }
                }),
            );
        });

        let source = CoinMarketCapSource::new(
            TradingPair::ZkcUsd,
            "test-api-key".to_string(),
            Duration::from_secs(10),
        )
        .unwrap()
        .with_api_url(server.base_url().parse().unwrap());

        let quote = source.get_price().await.unwrap();

        mock.assert();
        assert_eq!(quote.price, U256::from(12345600u128)); // 0.123456 * 1e8
        assert_eq!(quote.timestamp, 1769689800);
    }

    #[tokio::test]
    async fn test_handles_http_error() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(GET).path("/v2/cryptocurrency/quotes/latest");
            then.status(401);
        });

        let source = CoinMarketCapSource::new(
            TradingPair::EthUsd,
            "invalid-key".to_string(),
            Duration::from_secs(10),
        )
        .unwrap()
        .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_price().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handles_missing_id() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(GET).path("/v2/cryptocurrency/quotes/latest").query_param("id", "1027");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "data": {
                        "1": {
                            "quote": {
                                "USD": {
                                    "price": 50000.0,
                                    "last_updated": "2024-01-29T12:00:00.000Z"
                                }
                            }
                        }
                    }
                }),
            );
        });

        let source = CoinMarketCapSource::new(
            TradingPair::EthUsd,
            "test-api-key".to_string(),
            Duration::from_secs(10),
        )
        .unwrap()
        .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_price().await;
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::Internal(_))));
    }

    #[tokio::test]
    async fn test_handles_missing_currency() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(GET).path("/v2/cryptocurrency/quotes/latest").query_param("id", "1027");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "data": {
                        "1027": {
                            "quote": {
                                "EUR": {
                                    "price": 2300.0,
                                    "last_updated": "2024-01-29T12:00:00.000Z"
                                }
                            }
                        }
                    }
                }),
            );
        });

        let source = CoinMarketCapSource::new(
            TradingPair::EthUsd,
            "test-api-key".to_string(),
            Duration::from_secs(10),
        )
        .unwrap()
        .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_price().await;
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::Internal(_))));
    }

    // Integration tests (require API key and network access)
    #[tokio::test]
    #[ignore]
    async fn test_api_eth_price() -> anyhow::Result<()> {
        let api_key = std::env::var("CMC_API_KEY").expect("CMC_API_KEY env var required");

        let source =
            CoinMarketCapSource::new(TradingPair::EthUsd, api_key, Duration::from_secs(10))?;

        let quote = source.get_price().await?;

        println!("{:?}", quote);

        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_zkc_price() -> anyhow::Result<()> {
        let api_key = std::env::var("CMC_API_KEY").expect("CMC_API_KEY env var required");

        let source =
            CoinMarketCapSource::new(TradingPair::ZkcUsd, api_key, Duration::from_secs(10))?;

        let quote = source.get_price().await?;

        println!("{:?}", quote);

        assert!(quote.price > U256::ZERO);
        assert!(quote.timestamp > 0);

        Ok(())
    }
}
