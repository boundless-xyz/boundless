use std::time::SystemTime;
use alloy::primitives::U256;
use alloy_primitives::I256;
use chrono::DateTime;

/// Configuration types for price oracle
pub mod config;
/// Error types for price oracle
pub mod error;
/// Price source implementations
pub mod sources;
/// Composite oracle with aggregation
pub mod composite_oracle;
/// Cached oracle with background refresh
pub mod cached_oracle;
mod manager;
/// Integration tests (run with --ignored flag)
#[cfg(test)]
mod integration_tests;

pub use config::PriceOracleConfig;
pub use error::PriceOracleError;
pub use composite_oracle::CompositeOracle;
pub use cached_oracle::CachedPriceOracle;
pub use manager::PriceOracleManager;

/// Trading pair for price queries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradingPair {
    /// ETH/USD trading pair
    EthUsd,
    /// ZKC/USD trading pair
    ZkcUsd,
}

impl TradingPair {
    /// Returns the string representation of the trading pair
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EthUsd => "ETH/USD",
            Self::ZkcUsd => "ZKC/USD",
        }
    }
}

impl std::fmt::Display for TradingPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Price quote from a source (all prices scaled by 1e8)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceQuote {
    /// USD price scaled by 1e8 (e.g., $2000.00 = 200000000000)
    pub price: U256,
    /// Unix timestamp when the price was last updated
    pub timestamp: u64,
}

impl PriceQuote {
    /// Create a new price quote
    pub fn new(price: U256, timestamp: u64) -> Self {
        Self { price, timestamp }
    }

    /// Check if quote is stale (older than max_age_secs)
    pub fn is_stale(&self, max_age_secs: u64) -> bool {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now.saturating_sub(self.timestamp) > max_age_secs
    }

    /// Convert scaled U256 price to f64 USD value
    pub fn price_to_f64(&self) -> f64 {
        self.price.to::<u128>() as f64 / 10u64.pow(SCALE_DECIMALS) as f64
    }

    /// Convert timestamp to human-readable string
    pub fn timestamp_to_human_readable(&self) -> String {
        let datetime = DateTime::from_timestamp(self.timestamp as i64, 0);
        match datetime {
            Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
            None => "Invalid timestamp".to_string(),
        }
    }
}

/// Aggregation mode for composite oracle
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AggregationMode {
    /// First successful source wins
    Priority,
    /// Median of all successful sources
    Median,
    /// Average of all successful sources
    Average,
}

impl Default for AggregationMode {
    fn default() -> Self {
        Self::Median
    }
}

/// Price oracle trait - each instance is dedicated to one trading pair
#[async_trait::async_trait]
pub trait PriceOracle: Send + Sync {
    /// Get the current price (pair is determined at construction)
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError>;
}

/// Trait for price sources - each instance is dedicated to one trading pair
pub trait PriceSource: PriceOracle + Send + Sync {
    /// Returns the name of this price source
    fn name(&self) -> &'static str;
}

const SCALE_DECIMALS: u32 = 8;

/// Scale a floating-point price to U256 with fixed decimals
pub fn scale_price_from_f64(price: f64) -> Result<U256, PriceOracleError> {
    // Validate the price
    if !price.is_finite() || price < 0.0 {
        return Err(PriceOracleError::InvalidPrice(format!("price data is infinite: {}", price)));
    }

    let price_scaled = (price * 10u64.pow(SCALE_DECIMALS) as f64).round() as u128;

    Ok(U256::from(price_scaled))
}

/// Scale an I256 price to U256 with fixed decimals
pub fn scale_price_from_i256(price: I256, decimals: u32) -> Result<U256, PriceOracleError> {
    if price <= I256::ZERO {
        return Err(PriceOracleError::InvalidPrice(format!("non-positive: {}", price)));
    }

    let price_raw: U256 = price
        .try_into()
        .map_err(|_| PriceOracleError::InvalidPrice(format!("conversion failed: {}", price)))?;

    let price = match decimals.cmp(&SCALE_DECIMALS) {
        std::cmp::Ordering::Equal => price_raw,
        std::cmp::Ordering::Less => {
            price_raw * U256::from(10u64.pow(SCALE_DECIMALS - decimals))
        }
        std::cmp::Ordering::Greater => {
            price_raw / U256::from(10u64.pow(decimals - SCALE_DECIMALS))
        }
    };

    Ok(price)
}

/// Validate that a price quote is not stale
pub fn validate_freshness(quote: PriceQuote, max_staleness_secs: Option<u64>) -> Result<PriceQuote, PriceOracleError> {
    if let Some(max_age) = max_staleness_secs {
        if quote.is_stale(max_age) {
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            return Err(PriceOracleError::StalePrice {
                age_secs: now.saturating_sub(quote.timestamp),
                max_secs: max_age,
            });
        }
    }
    Ok(quote)
}

/// Wrapper that adds staleness checking to any PriceSource
pub struct WithStalenessCheck<T: PriceSource> {
    inner: T,
    max_staleness_secs: u64,
}

impl<T: PriceSource> WithStalenessCheck<T> {
    /// Wrap a price source with staleness checking
    pub fn new(inner: T, max_staleness_secs: u64) -> Self {
        Self { inner, max_staleness_secs }
    }
}

#[async_trait::async_trait]
impl<T: PriceSource> PriceOracle for WithStalenessCheck<T> {
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
        let quote = self.inner.get_price().await?;
        validate_freshness(quote, Some(self.max_staleness_secs))
    }
}

impl<T: PriceSource> PriceSource for WithStalenessCheck<T> {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSource {
        quote: PriceQuote,
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockSource {
        async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
            Ok(self.quote)
        }
    }

    impl PriceSource for MockSource {
        fn name(&self) -> &'static str {
            "Mock"
        }
    }

    // Tests for scale_price_from_f64
    #[test]
    fn test_scale_price_from_f64_valid_prices() {
        let price = scale_price_from_f64(2000.50).unwrap();
        assert_eq!(price, U256::from(200050000000u128));

        let price = scale_price_from_f64(0.001).unwrap();
        assert_eq!(price, U256::from(100000u128));

        let price = scale_price_from_f64(100000.0).unwrap();
        assert_eq!(price, U256::from(10000000000000u128));

        let price = scale_price_from_f64(0.0).unwrap();
        assert_eq!(price, U256::ZERO);
    }

    #[test]
    fn test_scale_price_from_f64_negative() {
        // Negative price should error
        let result = scale_price_from_f64(-100.0);
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    #[test]
    fn test_scale_price_from_f64_nan() {
        // NaN should error
        let result = scale_price_from_f64(f64::NAN);
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    #[test]
    fn test_scale_price_from_f64_infinity() {
        // Infinity should error
        let result = scale_price_from_f64(f64::INFINITY);
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    // Tests for scale_price_from_i256
    #[test]
    fn test_scale_price_from_i256_standard_8_decimals() {
        // 8 decimals - pass-through
        let price_raw = I256::try_from(200050000000u128).unwrap();
        let price = scale_price_from_i256(price_raw, 8).unwrap();
        assert_eq!(price, U256::from(200050000000u128));
    }

    #[test]
    fn test_scale_price_from_i256_scale_up() {
        // 6 decimals → 8 decimals (multiply by 100)
        let price_raw = I256::try_from(2000500000u128).unwrap();
        let price = scale_price_from_i256(price_raw, 6).unwrap();
        assert_eq!(price, U256::from(200050000000u128));
    }

    #[test]
    fn test_scale_price_from_i256_scale_down() {
        // 18 decimals → 8 decimals (divide by 10^10)
        let price_raw = I256::try_from(2000500000000000000000u128).unwrap();
        let price = scale_price_from_i256(price_raw, 18).unwrap();
        assert_eq!(price, U256::from(200050000000u128));
    }

    #[test]
    fn test_scale_price_from_i256_zero() {
        // Zero value should error
        let result = scale_price_from_i256(I256::ZERO, 8);
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    #[test]
    fn test_scale_price_from_i256_negative() {
        // Negative value should error
        let price_raw = I256::try_from(-100).unwrap();
        let result = scale_price_from_i256(price_raw, 8);
        assert!(result.is_err());
        assert!(matches!(result, Err(PriceOracleError::InvalidPrice(_))));
    }

    // Tests for WithStalenessCheck wrapper
    #[tokio::test]
    async fn test_wrapper_accepts_fresh_price() {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let quote = PriceQuote::new(U256::from(200000000000u128), now - 10); // 10 seconds old

        let source = MockSource { quote };
        let wrapped = WithStalenessCheck::new(source, 60);

        let result = wrapped.get_price().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_wrapper_rejects_stale_price() {
        let old_timestamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() - 120; // 2 minutes old

        let quote = PriceQuote::new(U256::from(200000000000u128), old_timestamp);

        let source = MockSource { quote };
        let wrapped = WithStalenessCheck::new(source, 60); // Max 60 seconds

        let result = wrapped.get_price().await;
        assert!(result.is_err());
        match result {
            Err(PriceOracleError::StalePrice { age_secs, max_secs }) => {
                assert!(age_secs >= 120);
                assert_eq!(max_secs, 60);
            }
            _ => panic!("Expected StalePrice error"),
        }
    }

    #[tokio::test]
    async fn test_wrapper_preserves_source_name() {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let quote = PriceQuote::new(U256::from(200000000000u128), now);

        let source = MockSource { quote };
        let wrapped = WithStalenessCheck::new(source, 60);

        assert_eq!(wrapped.name(), "Mock");
    }
}