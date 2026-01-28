use std::sync::Arc;
use std::time::SystemTime;
use alloy::primitives::U256;
use alloy_primitives::I256;

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

pub use config::{PriceOracleConfig, StaticPriceConfig};
pub use error::PriceOracleError;
pub use composite_oracle::CompositeOracle;

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

/// Price oracle trait
#[async_trait::async_trait]
pub trait PriceOracle: Send + Sync {
    /// Get the current price for a trading pair
    async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError>;
}

/// Trait for price sources
pub trait PriceSource: PriceOracle + Send + Sync {
    /// Returns the name of this price source
    fn name(&self) -> &'static str;
}

const SCALE_DECIMALS: u32 = 8;

/// Scale a floating-point price to U256 with fixed decimals
pub fn scale_price_from_f64(price: f64) -> Result<U256, PriceOracleError> {
    // Validate the price
    if !price.is_finite() || price < 0.0 {
        return Err(PriceOracleError::Internal(format!("invalid price data: {}", price)));
    }

    let price_scaled = (price * 10u64.pow(SCALE_DECIMALS) as f64).round() as u128;

    Ok(U256::from(price_scaled))
}

/// Scale an I256 price to U256 with fixed decimals
pub fn scale_price_from_i256(price: I256, decimals: u32) -> Result<U256, PriceOracleError> {
    if price <= I256::ZERO {
        return Err(PriceOracleError::Internal("invalid price: non-positive".to_string()));
    }

    let price_raw: U256 = price
        .try_into()
        .map_err(|_| PriceOracleError::Internal("price conversion failed".to_string()))?;

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

// TODO: add tests for price scaling function