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
use std::time::SystemTime;

/// A price source that returns a configured static price
pub struct StaticPriceSource {
    pair: TradingPair,
    price: f64,
}

impl StaticPriceSource {
    /// Create a new static price source with a fixed price value for a trading pair
    pub fn new(pair: TradingPair, price: f64) -> Self {
        Self { pair, price }
    }
}

#[async_trait::async_trait]
impl PriceOracle for StaticPriceSource {
    fn pair(&self) -> TradingPair {
        self.pair
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        let rate = scale_price_from_f64(self.price)?;

        // Static prices are always considered fresh
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();

        Ok(ExchangeRate::new(self.pair, rate, now))
    }

    fn name(&self) -> String {
        format!("StaticPriceSource({})", self.pair).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    #[tokio::test]
    async fn test_static_source() {
        let source = StaticPriceSource::new(TradingPair::EthUsd, 2500.00);

        let result = source.get_rate().await.unwrap();
        assert_eq!(result.pair, TradingPair::EthUsd);
        assert_eq!(result.rate, U256::from(250000000000u128)); // 2500.00 * 1e8
    }

    #[tokio::test]
    async fn test_static_source_small_value() {
        let source = StaticPriceSource::new(TradingPair::ZkcUsd, 1.50);

        let result = source.get_rate().await.unwrap();
        assert_eq!(result.pair, TradingPair::ZkcUsd);
        assert_eq!(result.rate, U256::from(150000000u128)); // 1.50 * 1e8
    }
}
