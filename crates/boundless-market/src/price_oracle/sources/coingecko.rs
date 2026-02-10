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
    scale_price_from_f64, ExchangeRate, PriceOracle, PriceOracleError, TradingPair,
};
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
    pair: TradingPair,
}

impl CoinGeckoSource {
    /// Create a new CoinGecko source for a specific trading pair
    pub fn new(pair: TradingPair, timeout: Duration) -> Result<Self, PriceOracleError> {
        let api_url = Url::parse("https://api.coingecko.com").unwrap();

        let client = Client::builder()
            .timeout(timeout)
            // CoinGecko requires a user-agent header for the free API
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
            .build()?;

        Ok(Self { client, api_url, pair })
    }

    /// Configure the API URL for testing
    pub fn with_api_url(mut self, url: Url) -> Self {
        self.api_url = url;
        self
    }

    async fn fetch_rate(
        &self,
        path: &str,
        ids: &str,
        vs_currencies: &str,
    ) -> Result<ExchangeRate, PriceOracleError> {
        let mut url = self.api_url.clone();
        url.set_path(path);
        url.query_pairs_mut()
            .append_pair("ids", ids)
            .append_pair("vs_currencies", vs_currencies)
            .append_pair("include_last_updated_at", "true");

        let response = self.client.get(url).send().await?.error_for_status()?;

        let data: CoinGeckoPriceResponse = response.json().await?;
        let coin_data = data.get(ids).ok_or_else(|| {
            PriceOracleError::Internal(format!("coin {} not found in response", ids))
        })?;

        let rate = scale_price_from_f64(coin_data.usd)?;

        Ok(ExchangeRate::new(self.pair, rate, coin_data.last_updated_at))
    }
}

#[async_trait::async_trait]
impl PriceOracle for CoinGeckoSource {
    fn pair(&self) -> TradingPair {
        self.pair
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        match self.pair {
            TradingPair::EthUsd => self.fetch_rate("/api/v3/simple/price", "ethereum", "usd").await,
            TradingPair::ZkcUsd => {
                self.fetch_rate("/api/v3/simple/price", "boundless", "usd").await
            }
        }
    }

    fn name(&self) -> String {
        format!("CoinGeckoPriceSource({})", self.pair)
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
                .path("/api/v3/simple/price")
                .query_param("ids", "ethereum")
                .query_param("vs_currencies", "usd")
                .query_param("include_last_updated_at", "true");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "ethereum": {
                        "usd": 2500.50,
                        "last_updated_at": 1706547200
                    }
                }),
            );
        });

        let source = CoinGeckoSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let rate = source.get_rate().await.unwrap();

        mock.assert();
        assert_eq!(rate.pair, TradingPair::EthUsd);
        assert_eq!(rate.rate, U256::from(250050000000u128)); // 2500.50 * 1e8
        assert_eq!(rate.timestamp, 1706547200);
    }

    #[tokio::test]
    async fn test_zkc_usd_price_success() {
        let server = MockServer::start();

        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/api/v3/simple/price")
                .query_param("ids", "boundless")
                .query_param("vs_currencies", "usd")
                .query_param("include_last_updated_at", "true");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "boundless": {
                        "usd": 0.123456,
                        "last_updated_at": 1706547300
                    }
                }),
            );
        });

        let source = CoinGeckoSource::new(TradingPair::ZkcUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let rate = source.get_rate().await.unwrap();

        mock.assert();
        assert_eq!(rate.pair, TradingPair::ZkcUsd);
        assert_eq!(rate.rate, U256::from(12345600u128)); // 0.123456 * 1e8
        assert_eq!(rate.timestamp, 1706547300);
    }

    #[tokio::test]
    async fn test_handles_http_error() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(GET).path("/api/v3/simple/price");
            then.status(500);
        });

        let source = CoinGeckoSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_rate().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handles_missing_coin_data() {
        let server = MockServer::start();

        server.mock(|when, then| {
            when.method(GET).path("/api/v3/simple/price").query_param("ids", "ethereum");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({
                    "bitcoin": {
                        "usd": 50000.0,
                        "last_updated_at": 1706547200
                    }
                }),
            );
        });

        let source = CoinGeckoSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_rate().await;
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::Internal(_))));
    }

    // Integration tests (require network access)
    #[tokio::test]
    #[ignore]
    async fn test_api_eth_price() -> anyhow::Result<()> {
        let source = CoinGeckoSource::new(TradingPair::EthUsd, Duration::from_secs(10))?;

        let rate = source.get_rate().await?;

        println!("{:?}", rate);

        // Sanity check: ETH should be worth something
        assert_eq!(rate.pair, TradingPair::EthUsd);
        assert!(rate.rate > U256::ZERO);
        assert!(rate.timestamp > 0);

        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_zkc_price() -> anyhow::Result<()> {
        let source = CoinGeckoSource::new(TradingPair::ZkcUsd, Duration::from_secs(10))?;

        let rate = source.get_rate().await?;

        println!("{:?}", rate);

        // Sanity check: ZKC should be worth something
        assert_eq!(rate.pair, TradingPair::ZkcUsd);
        assert!(rate.rate > U256::ZERO);
        assert!(rate.timestamp > 0);

        Ok(())
    }
}
