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

//! Exchange rate types and utilities for price conversions.
//!
//! This module provides types for representing exchange rates between assets,
//! with proper decimal precision and timestamp tracking.

use alloy::primitives::U256;
use alloy_primitives::I256;
use chrono::DateTime;
use std::time::SystemTime;

use super::asset::{convert_asset_value, Amount, Asset, ConversionError};
use super::error::PriceOracleError;

/// Standard decimal places for price quotes (Chainlink standard)
pub const PRICE_QUOTE_DECIMALS: u32 = 8;

/// Trading pair representing base/quote (e.g., ETH/USD means price of ETH in USD)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TradingPair {
    /// ETH/USD trading pair
    EthUsd,
    /// ZKC/USD trading pair
    ZkcUsd,
}

impl TradingPair {
    /// Get the base asset (the asset being priced)
    pub fn base(&self) -> Asset {
        match self {
            Self::EthUsd => Asset::ETH,
            Self::ZkcUsd => Asset::ZKC,
        }
    }

    /// Get the quote asset (the asset the price is expressed in)
    pub fn quote(&self) -> Asset {
        match self {
            Self::EthUsd => Asset::USD,
            Self::ZkcUsd => Asset::USD,
        }
    }
}

impl std::fmt::Display for TradingPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.base(), self.quote())
    }
}

/// An exchange rate between two assets with timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExchangeRate {
    /// The trading pair this rate represents
    pub pair: TradingPair,
    /// Rate scaled by 1e8 (e.g., 2000 USD/ETH = 200_000_000_000)
    pub rate: U256,
    /// Unix timestamp when the rate was observed
    pub timestamp: u64,
}

impl ExchangeRate {
    /// Create a new exchange rate
    pub fn new(pair: TradingPair, rate: U256, timestamp: u64) -> Self {
        Self { pair, rate, timestamp }
    }

    /// Convenience constructor with explicit base/quote
    pub fn new_with_assets(base: Asset, quote: Asset, rate: U256, timestamp: u64) -> Self {
        let pair = match (base, quote) {
            (Asset::ETH, Asset::USD) => TradingPair::EthUsd,
            (Asset::ZKC, Asset::USD) => TradingPair::ZkcUsd,
            _ => panic!("Unsupported trading pair: {}/{}", base, quote),
        };
        Self { pair, rate, timestamp }
    }

    /// Convert an amount from base to quote currency
    pub fn convert(&self, amount: &Amount) -> Result<Amount, ConversionError> {
        if amount.asset != self.pair.base() {
            return Err(ConversionError::AssetMismatch {
                expected: self.pair.base(),
                actual: amount.asset,
            });
        }
        let converted =
            convert_asset_value(amount.value, self.pair.base(), self.pair.quote(), self.rate)?;
        Ok(Amount::new(converted, self.pair.quote()))
    }

    /// Check if rate is stale (older than max_age_secs)
    pub fn is_stale(&self, max_age_secs: u64) -> bool {
        let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        now.saturating_sub(self.timestamp) > max_age_secs
    }

    /// Convert rate to f64 for display
    pub fn rate_to_f64(&self) -> f64 {
        self.rate.to::<u128>() as f64 / 10u64.pow(PRICE_QUOTE_DECIMALS) as f64
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

/// Scale a floating-point price to U256 with fixed decimals
pub fn scale_price_from_f64(price: f64) -> Result<U256, PriceOracleError> {
    // Validate the price
    if !price.is_finite() || price < 0.0 {
        return Err(PriceOracleError::InvalidPrice(format!("price data is infinite: {}", price)));
    }

    let price_scaled = (price * 10u64.pow(PRICE_QUOTE_DECIMALS) as f64).round() as u128;

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

    let price = match decimals.cmp(&PRICE_QUOTE_DECIMALS) {
        std::cmp::Ordering::Equal => price_raw,
        std::cmp::Ordering::Less => {
            price_raw * U256::from(10u64.pow(PRICE_QUOTE_DECIMALS - decimals))
        }
        std::cmp::Ordering::Greater => {
            price_raw / U256::from(10u64.pow(decimals - PRICE_QUOTE_DECIMALS))
        }
    };

    Ok(price)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_exchange_rate_convert() {
        let rate = ExchangeRate::new_with_assets(
            Asset::ETH,
            Asset::USD,
            U256::from(200_000_000_000u128), // $2000/ETH
            1234567890,
        );

        let eth_amount = Amount::new(U256::from(1_000_000_000_000_000_000u128), Asset::ETH); // 1 ETH
        let usd_amount = rate.convert(&eth_amount).unwrap();

        assert_eq!(usd_amount.asset, Asset::USD);
        assert_eq!(usd_amount.value, U256::from(2_000_000_000u128)); // $2000
    }

    #[test]
    fn test_exchange_rate_convert_wrong_asset() {
        let rate = ExchangeRate::new_with_assets(
            Asset::ETH,
            Asset::USD,
            U256::from(200_000_000_000u128),
            1234567890,
        );

        let zkc_amount = Amount::new(U256::from(1_000_000_000_000_000_000u128), Asset::ZKC);
        let result = rate.convert(&zkc_amount);

        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::AssetMismatch { expected, actual } => {
                assert_eq!(expected, Asset::ETH);
                assert_eq!(actual, Asset::ZKC);
            }
            _ => panic!("Expected AssetMismatch error"),
        }
    }

    #[test]
    fn test_trading_pair_display() {
        let pair = TradingPair::EthUsd;
        assert_eq!(pair.to_string(), "ETH/USD");

        let pair = TradingPair::ZkcUsd;
        assert_eq!(pair.to_string(), "ZKC/USD");
    }
}
