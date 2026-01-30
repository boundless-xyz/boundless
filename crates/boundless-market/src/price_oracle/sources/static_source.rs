use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote, PriceSource, scale_price_from_f64};
use std::time::SystemTime;

/// A price source that returns a configured static price
pub struct StaticPriceSource {
    price: f64,
}

impl StaticPriceSource {
    /// Create a new static price source with a fixed price value
    pub fn new(price: f64) -> Self {
        Self { price }
    }
}

#[async_trait::async_trait]
impl PriceOracle for StaticPriceSource {
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
        let price = scale_price_from_f64(self.price)?;

        // Static prices are always considered fresh
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(PriceQuote::new(price, now))
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
        let source = StaticPriceSource::new(2500.00);

        let result = source.get_price().await.unwrap();
        assert_eq!(result.price, U256::from(250000000000u128)); // 2500.00 * 1e8
    }

    #[tokio::test]
    async fn test_static_source_small_value() {
        let source = StaticPriceSource::new(1.50);

        let result = source.get_price().await.unwrap();
        assert_eq!(result.price, U256::from(150000000u128)); // 1.50 * 1e8
    }
}
