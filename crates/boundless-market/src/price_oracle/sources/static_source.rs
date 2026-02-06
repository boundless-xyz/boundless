use crate::price_oracle::{
    scale_price_from_f64, ExchangeRate, PriceOracle, PriceOracleError, PriceSource, TradingPair,
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
}

impl PriceSource for StaticPriceSource {
    fn name(&self) -> &'static str {
        "static"
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
