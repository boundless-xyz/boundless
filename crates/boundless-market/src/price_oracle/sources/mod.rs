use crate::price_oracle::{PriceOracle, PriceOracleError};
use std::{future::Future};
use alloy_primitives::U256;

/// Price source implementation for chainlink
pub mod chainlink;
/// Price source implementation for coingecko
pub mod coingecko;
/// Price source implementation for coinmarketcap
pub mod cmc;

pub use chainlink::ChainlinkSource;
pub use coingecko::CoinGeckoSource;
pub use cmc::CoinMarketCapSource;

/// Trait for price sources
pub trait PriceSource: PriceOracle + Send + Sync {
    /// Returns the name of this price source
    fn name(&self) -> &'static str;
}

/// Scale a floating-point price to U256 with 8 decimal places
pub fn scale_price(price: f64) -> Result<U256, PriceOracleError> {
    // Validate the price
    if !price.is_finite() || price < 0.0 {
        return Err(PriceOracleError::Internal(format!("invalid price data: {}", price)));
    }

    let price_scaled = (price * 1e8).round() as u128;

    Ok(U256::from(price_scaled))
}