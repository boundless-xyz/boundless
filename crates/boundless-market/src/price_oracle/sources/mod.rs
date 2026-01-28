use crate::price_oracle::{PriceOracle, PriceOracleError};
use std::{future::Future};
use alloy_primitives::{I256, U256};

/// Price source implementation for chainlink
pub mod chainlink;
/// Price source implementation for coingecko
pub mod coingecko;
/// Price source implementation for coinmarketcap
pub mod cmc;

pub use chainlink::ChainlinkSource;
pub use coingecko::CoinGeckoSource;
pub use cmc::CoinMarketCapSource;

const SCALE_DECIMALS: u32 = 8;

/// Trait for price sources
pub trait PriceSource: PriceOracle + Send + Sync {
    /// Returns the name of this price source
    fn name(&self) -> &'static str;
}

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

// TODO: add tests for price scaling functions