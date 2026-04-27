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
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Deserialize)]
struct CoinbasePriceData {
    amount: String,
}

#[derive(Deserialize)]
struct CoinbasePriceResponse {
    data: CoinbasePriceData,
}

/// Coinbase public spot price source.
///
/// Uses the keyless `/v2/prices/{product}/spot` endpoint.
pub struct CoinbaseSource {
    client: Client,
    api_url: Url,
    pair: TradingPair,
}

impl CoinbaseSource {
    /// Create a new Coinbase source for a specific trading pair
    pub fn new(pair: TradingPair, timeout: Duration) -> Result<Self, PriceOracleError> {
        let api_url = Url::parse("https://api.coinbase.com").unwrap();
        let client = Client::builder().timeout(timeout).build()?;
        Ok(Self { client, api_url, pair })
    }

    /// Configure the API URL for testing
    pub fn with_api_url(mut self, url: Url) -> Self {
        self.api_url = url;
        self
    }

    fn product(&self) -> &'static str {
        match self.pair {
            TradingPair::EthUsd => "ETH-USD",
            TradingPair::ZkcUsd => "ZKC-USD",
        }
    }
}

#[async_trait::async_trait]
impl PriceOracle for CoinbaseSource {
    fn pair(&self) -> TradingPair {
        self.pair
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        let mut url = self.api_url.clone();
        url.set_path(&format!("/v2/prices/{}/spot", self.product()));

        let response = self.client.get(url).send().await?.error_for_status()?;
        let data: CoinbasePriceResponse = response.json().await?;

        let price: f64 = data.data.amount.parse().map_err(|e| {
            PriceOracleError::InvalidPrice(format!(
                "failed to parse Coinbase price '{}': {e}",
                data.data.amount
            ))
        })?;
        let rate = scale_price_from_f64(price)?;

        // Coinbase /spot has no timestamp; use wall-clock as observation time.
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
        Ok(ExchangeRate::new(self.pair, rate, timestamp))
    }

    fn name(&self) -> String {
        format!("CoinbaseSource({})", self.pair)
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
            when.method(GET).path("/v2/prices/ETH-USD/spot");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({"data": {"amount": "2500.50", "base": "ETH", "currency": "USD"}}),
            );
        });

        let source = CoinbaseSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let rate = source.get_rate().await.unwrap();

        mock.assert();
        assert_eq!(rate.pair, TradingPair::EthUsd);
        assert_eq!(rate.rate, U256::from(250050000000u128));
    }

    #[tokio::test]
    async fn test_zkc_usd_price_success() {
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/v2/prices/ZKC-USD/spot");
            then.status(200).header("content-type", "application/json").json_body(
                serde_json::json!({"data": {"amount": "0.0733", "base": "ZKC", "currency": "USD"}}),
            );
        });

        let source = CoinbaseSource::new(TradingPair::ZkcUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let rate = source.get_rate().await.unwrap();

        mock.assert();
        assert_eq!(rate.pair, TradingPair::ZkcUsd);
        assert_eq!(rate.rate, U256::from(7330000u128));
    }

    #[tokio::test]
    async fn test_handles_http_error() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/prices/ETH-USD/spot");
            then.status(503);
        });

        let source = CoinbaseSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_rate().await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_handles_unparseable_price() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/v2/prices/ETH-USD/spot");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({"data": {"amount": "nan-ish"}}));
        });

        let source = CoinbaseSource::new(TradingPair::EthUsd, Duration::from_secs(10))
            .unwrap()
            .with_api_url(server.base_url().parse().unwrap());

        let result = source.get_rate().await;
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_eth_price() -> anyhow::Result<()> {
        let source = CoinbaseSource::new(TradingPair::EthUsd, Duration::from_secs(10))?;
        let rate = source.get_rate().await?;
        println!("{:?}", rate);
        assert_eq!(rate.pair, TradingPair::EthUsd);
        assert!(rate.rate > U256::ZERO);
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn test_api_zkc_price() -> anyhow::Result<()> {
        let source = CoinbaseSource::new(TradingPair::ZkcUsd, Duration::from_secs(10))?;
        let rate = source.get_rate().await?;
        println!("{:?}", rate);
        assert_eq!(rate.pair, TradingPair::ZkcUsd);
        assert!(rate.rate > U256::ZERO);
        Ok(())
    }
}
