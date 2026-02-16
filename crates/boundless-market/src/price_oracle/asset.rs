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

//! Asset and amount types for USD-based pricing.
//!
//! This module provides types for representing monetary amounts in different assets
//! (USD, ETH, ZKC) with proper decimal precision.

use crate::price_oracle::PriceOracleError;
use alloy_primitives::U256;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;

/// Asset types supported in the pricing system
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Asset {
    /// US Dollar (6 decimals, like USDC/USDT)
    USD,
    /// Ethereum (18 decimals)
    ETH,
    /// ZKC token (18 decimals)
    ZKC,
}

impl Asset {
    /// Returns the number of decimal places for this asset
    pub fn decimals(&self) -> u8 {
        match self {
            Asset::USD => 6,
            Asset::ETH => 18,
            Asset::ZKC => 18,
        }
    }

    /// Returns the default number of decimal places for human-readable display
    pub fn display_decimals(&self) -> usize {
        match self {
            Asset::USD => 4,
            Asset::ETH => 8,
            Asset::ZKC => 6,
        }
    }
}

impl FromStr for Asset {
    type Err = ParseAmountError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "USD" => Ok(Asset::USD),
            "ETH" => Ok(Asset::ETH),
            "ZKC" => Ok(Asset::ZKC),
            _ => Err(ParseAmountError::UnknownAsset(s.to_string())),
        }
    }
}

impl fmt::Display for Asset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Asset::USD => write!(f, "USD"),
            Asset::ETH => write!(f, "ETH"),
            Asset::ZKC => write!(f, "ZKC"),
        }
    }
}

// ============ Errors ============

/// Errors that can occur when parsing an amount string
#[derive(Debug, thiserror::Error)]
pub enum ParseAmountError {
    /// Invalid format, expected '\<value\> \<asset\>' or '\<value\>' when default asset is configured
    #[error("invalid format: expected '<value> <ASSET>' (e.g., '1.12 USD' (up to 6 decimals), '1.500012 ETH' (up to 18 decimals)) or '<value>' when default asset is configured")]
    InvalidFormat,
    /// Unknown asset type
    #[error("unknown asset: {0}")]
    UnknownAsset(String),
    /// Invalid numeric value
    #[error("invalid number: {0}")]
    InvalidNumber(String),
    /// Asset not in the allowed set
    #[error("asset {0} not in allowed set: {1:?}")]
    AssetNotAllowed(
        /// The asset that was provided
        Asset,
        /// The list of allowed assets
        Vec<Asset>,
    ),
    /// Too many decimal places for the asset
    #[error("too many decimal places for {asset}: got {got}, max {max}")]
    TooManyDecimals {
        /// The asset type
        asset: Asset,
        /// Number of decimals provided
        got: usize,
        /// Maximum allowed decimals
        max: u8,
    },
}

// ============ Amount ============

/// An amount of a specific asset with proper decimal precision
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amount {
    /// The value in the smallest unit (e.g., wei for ETH, micro-USD for USD)
    pub value: U256,
    /// The asset type
    pub asset: Asset,
}

impl Amount {
    /// Create a new Amount with the given value and asset
    pub fn new(value: U256, asset: Asset) -> Self {
        Self { value, asset }
    }

    /// Parse an amount string into a structured Amount.
    ///
    /// Format: "\<value\> \[\<asset\>\]" (e.g., "1.5 ETH", "100 USD", "0.001" if default asset is provided)
    /// Default asset can be provided for plain numbers without asset specifier
    pub fn parse(s: &str, default: Option<Asset>) -> Result<Self, ParseAmountError> {
        let s = s.trim();

        // Split by whitespace to find asset
        let split = s.split_whitespace().collect::<Vec<_>>();

        // Must have at least value
        if split.is_empty() {
            return Err(ParseAmountError::InvalidFormat);
        }
        let value_str = split[0];

        // Determine asset in order: specified, default, error
        let asset: Asset = if split.len() == 2 {
            let asset_str = split[1];
            asset_str.trim().parse()?
        } else if let Some(def) = default {
            def
        } else {
            return Err(ParseAmountError::InvalidFormat);
        };

        let value = parse_decimal_to_u256(value_str.trim(), asset.decimals(), asset)?;

        Ok(Self { value, asset })
    }

    /// Parse with validation of allowed assets
    /// Default asset can be provided for plain numbers without asset specifier
    pub fn parse_with_allowed(
        s: &str,
        allowed: &[Asset],
        default: Option<Asset>,
    ) -> Result<Self, ParseAmountError> {
        let amount = Self::parse(s, default)?;
        if !allowed.contains(&amount.asset) {
            return Err(ParseAmountError::AssetNotAllowed(amount.asset, allowed.to_vec()));
        }
        Ok(amount)
    }

    /// Format for display with a specific number of decimal places, with rounding.
    pub fn format_display(&self, decimals: usize) -> String {
        format_amount_rounded(self.value, self.asset, decimals)
    }

    /// Format for display using the asset's default decimal places.
    pub fn format(&self) -> String {
        self.format_display(self.asset.display_decimals())
    }
}

impl fmt::Display for Amount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let decimals = f.precision().unwrap_or(self.asset.display_decimals());
        write!(f, "{}", self.format_display(decimals))
    }
}

impl FromStr for Amount {
    type Err = ParseAmountError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s, None)
    }
}

// ============ Parsing helpers ============

fn parse_decimal_to_u256(s: &str, decimals: u8, asset: Asset) -> Result<U256, ParseAmountError> {
    let (integer, fraction) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };

    if fraction.len() > decimals as usize {
        return Err(ParseAmountError::TooManyDecimals {
            asset,
            got: fraction.len(),
            max: decimals,
        });
    }

    let padded = format!("{integer}{fraction:0<width$}", width = decimals as usize);

    U256::from_str(&padded).map_err(|_| ParseAmountError::InvalidNumber(s.to_string()))
}

/// Format a value and asset as a human-readable string
///
/// Properly handles decimal places and trailing zeros
pub fn format_amount(value: U256, asset: Asset) -> String {
    let decimals = asset.decimals() as usize;
    let s = value.to_string();

    if decimals == 0 {
        return format!("{s} {asset}");
    }

    let formatted = if s.len() <= decimals {
        format!("0.{:0>width$}", s, width = decimals)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        let (integer, fraction) = s.split_at(s.len() - decimals);
        let fraction = fraction.trim_end_matches('0');
        if fraction.is_empty() {
            integer.to_string()
        } else {
            format!("{integer}.{fraction}")
        }
    };

    format!("{formatted} {asset}")
}

/// Format a value with rounding to a specific number of decimal places.
/// Uses round-half-up rounding and trims trailing zeros.
fn format_amount_rounded(value: U256, asset: Asset, display_decimals: usize) -> String {
    let total_decimals = asset.decimals() as usize;

    // If display decimals >= total decimals, no rounding needed
    if display_decimals >= total_decimals {
        return format_amount(value, asset);
    }

    // Round: divisor = 10^(total_decimals - display_decimals)
    let shift = total_decimals - display_decimals;
    let divisor = U256::from(10u64).pow(U256::from(shift));
    let half = divisor / U256::from(2);
    let rounded = (value + half) / divisor;

    // If rounding would show zero but value is non-zero, fall back to lossless format
    if rounded == U256::ZERO && value > U256::ZERO {
        return format_amount(value, asset);
    }

    // Now `rounded` has `display_decimals` implicit decimal places
    let s = rounded.to_string();

    if display_decimals == 0 {
        return format!("{s} {asset}");
    }

    let formatted = if s.len() <= display_decimals {
        format!("0.{:0>width$}", s, width = display_decimals)
    } else {
        let (integer, fraction) = s.split_at(s.len() - display_decimals);
        format!("{integer}.{fraction}")
    };

    // Trim trailing zeros for cleaner display
    let formatted = formatted.trim_end_matches('0').trim_end_matches('.');

    format!("{formatted} {asset}")
}

// ============ Serde ============

impl Serialize for Amount {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format_amount(self.value, self.asset))
    }
}

impl<'de> Deserialize<'de> for Amount {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Amount::parse(&s, None).map_err(de::Error::custom)
    }
}

/// Conversion error types for Amount conversions
#[derive(Debug, thiserror::Error)]
pub enum ConversionError {
    /// Asset mismatch when converting
    #[error("asset mismatch: expected {expected}, got {actual}")]
    AssetMismatch {
        /// Expected asset
        expected: Asset,
        /// Actual asset
        actual: Asset,
    },

    /// Unsupported conversion between asset types
    #[error("unsupported conversion: {from} -> {to}")]
    UnsupportedConversion {
        /// Source asset
        from: Asset,
        /// Target asset
        to: Asset,
    },

    /// Price oracle error while fetching prices
    #[error("price oracle error: {0}")]
    PriceOracle(#[from] PriceOracleError),

    /// Arithmetic overflow during conversion
    #[error("arithmetic overflow during conversion")]
    Overflow,

    /// Division by zero during conversion
    #[error("division by zero during conversion")]
    DivisionByZero,
}

// ============ Price Conversion ============

use super::PRICE_QUOTE_DECIMALS;

/// Convert a value between two assets given a price quote.
///
/// The price represents how many USD one unit of the "priced asset" is worth.
/// - For USD→ETH: price is ETH/USD, we divide by price
/// - For ETH→USD: price is ETH/USD, we multiply by price
///
/// Formula derivation:
/// - When converting from lower to higher decimals (USD→ETH):
///   scale = 10^(to_decimals - from_decimals + price_decimals)
///   result = value * scale / price
/// - When converting from higher to lower decimals (ETH→USD):
///   scale = 10^(from_decimals - to_decimals + price_decimals)
///   result = value * price / scale
pub fn convert_asset_value(
    value: U256,
    from: Asset,
    to: Asset,
    price: U256,
) -> Result<U256, ConversionError> {
    let from_decimals = from.decimals() as u32;
    let to_decimals = to.decimals() as u32;
    let price_decimals = PRICE_QUOTE_DECIMALS;

    if to_decimals > from_decimals {
        // Converting from lower to higher decimals (USD → ETH/ZKC)
        // scale = 10^(to_decimals - from_decimals + price_decimals)
        let exponent = to_decimals - from_decimals + price_decimals;
        let scale = U256::from(10u128).pow(U256::from(exponent));
        value
            .checked_mul(scale)
            .ok_or(ConversionError::Overflow)?
            .checked_div(price)
            .ok_or(ConversionError::DivisionByZero)
    } else if from_decimals > to_decimals {
        // Converting from higher to lower decimals (ETH/ZKC → USD)
        // scale = 10^(from_decimals - to_decimals + price_decimals)
        let exponent = from_decimals - to_decimals + price_decimals;
        let scale = U256::from(10u128).pow(U256::from(exponent));
        value
            .checked_mul(price)
            .ok_or(ConversionError::Overflow)?
            .checked_div(scale)
            .ok_or(ConversionError::DivisionByZero)
    } else {
        // Same decimals - shouldn't happen with our current asset set
        // But handle it generically: just apply price scaling
        value
            .checked_mul(price)
            .ok_or(ConversionError::Overflow)?
            .checked_div(U256::from(10u128).pow(U256::from(price_decimals)))
            .ok_or(ConversionError::DivisionByZero)
    }
}

/// Scale a value between different decimal precisions.
///
/// This is useful when you need to convert between token representations with
/// different decimal places (e.g., converting between ZKC with 18 decimals and
/// USDC with 6 decimals) without applying any price conversion.
///
/// Example: scale_decimals(1000000000000000000, 18, 6) = 1000000
/// (1.0 with 18 decimals → 1.0 with 6 decimals)
///
/// # Arguments
/// * `value` - The value to scale
/// * `from_decimals` - The current decimal precision
/// * `to_decimals` - The target decimal precision
///
/// # Returns
/// The scaled value with the target decimal precision
pub fn scale_decimals(value: U256, from_decimals: u8, to_decimals: u8) -> U256 {
    if from_decimals == to_decimals {
        value
    } else if from_decimals > to_decimals {
        let scale = U256::from(10u64).pow(U256::from(from_decimals - to_decimals));
        value / scale
    } else {
        let scale = U256::from(10u64).pow(U256::from(to_decimals - from_decimals));
        value * scale
    }
}

// ============ Tests ============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_eth() {
        let amount = Amount::parse("1.5 ETH", None).unwrap();
        assert_eq!(amount.asset, Asset::ETH);
        assert_eq!(amount.value, U256::from(1_500_000_000_000_000_000u128)); // 1.5 ETH in wei
    }

    #[test]
    fn test_parse_usd() {
        let amount = Amount::parse("100 USD", None).unwrap();
        assert_eq!(amount.asset, Asset::USD);
        assert_eq!(amount.value, U256::from(100_000_000u128)); // 100 USD with 6 decimals
    }

    #[test]
    fn test_parse_with_allowed_success() {
        let amount =
            Amount::parse_with_allowed("1.5 ETH", &[Asset::ETH, Asset::ZKC], None).unwrap();
        assert_eq!(amount.asset, Asset::ETH);
    }

    #[test]
    fn test_parse_with_allowed_failure() {
        let result = Amount::parse_with_allowed("1.5 ETH", &[Asset::USD, Asset::ZKC], None);
        assert!(result.is_err());
        match result.unwrap_err() {
            ParseAmountError::AssetNotAllowed(asset, allowed) => {
                assert_eq!(asset, Asset::ETH);
                assert_eq!(allowed, vec![Asset::USD, Asset::ZKC]);
            }
            _ => panic!("Expected AssetNotAllowed error"),
        }
    }

    #[test]
    fn test_format_roundtrip() {
        let amount = Amount::parse("1.5 ETH", None).unwrap();
        assert_eq!(amount.format(), "1.5 ETH");
        assert_eq!(amount.to_string(), "1.5 ETH");
    }

    #[test]
    fn test_wei_precision() {
        let amount = Amount::parse("0.000000000000000001 ETH", None).unwrap();
        assert_eq!(amount.value, U256::from(1)); // 1 wei
    }

    #[test]
    fn test_usd_precision() {
        let amount = Amount::parse("0.000001 USD", None).unwrap();
        assert_eq!(amount.value, U256::from(1)); // 1 micro-USD
    }

    #[test]
    fn test_display_asset() {
        assert_eq!(Asset::ETH.to_string(), "ETH");
        assert_eq!(Asset::USD.to_string(), "USD");
        assert_eq!(Asset::ZKC.to_string(), "ZKC");
    }

    #[test]
    fn test_default() {
        let result = Amount::parse("0.0000001", Some(Asset::ETH)).unwrap();
        assert_eq!(result.asset, Asset::ETH);
        assert_eq!(result.value, U256::from(100_000_000_000_u128)); // 0.0000001 ETH in wei

        let result =
            Amount::parse_with_allowed("1.5 ETH", &[Asset::ETH, Asset::ZKC], Some(Asset::ETH))
                .unwrap();
        assert_eq!(result.asset, Asset::ETH);
        assert_eq!(result.value, U256::from(1_500_000_000_000_000_000u128)); // 1.5 ETH in wei

        let result = Amount::parse_with_allowed("1.5", &[Asset::ETH, Asset::ZKC], Some(Asset::USD));
        assert!(result.is_err());
    }

    #[test]
    fn test_format_display_usd() {
        let amount = Amount::parse("1.234567 USD", None).unwrap();
        // Default 4 decimals, should round 1.234567 to 1.2346
        assert_eq!(amount.format(), "1.2346 USD");
        // Explicit 2 decimals
        assert_eq!(amount.format_display(2), "1.23 USD");
        // Explicit 6 decimals (no rounding)
        assert_eq!(amount.format_display(6), "1.234567 USD");
        // Formatter precision
        assert_eq!(format!("{:.2}", amount), "1.23 USD");
    }

    #[test]
    fn test_format_display_eth() {
        let amount = Amount::parse("1.123456789012345678 ETH", None).unwrap();
        // Default 8 decimals, should round
        assert_eq!(amount.format(), "1.12345679 ETH");
        // Explicit 2 decimals
        assert_eq!(amount.format_display(2), "1.12 ETH");
        // Explicit 18 decimals (no rounding)
        assert_eq!(amount.format_display(18), "1.123456789012345678 ETH");
        // Formatter precision
        assert_eq!(format!("{:.4}", amount), "1.1235 ETH");
    }

    #[test]
    fn test_rounding_half_up() {
        // 1.1255 USD with 4 decimals should stay 1.1255
        let amount = Amount::parse("1.1255 USD", None).unwrap();
        assert_eq!(amount.format_display(4), "1.1255 USD");

        // With 3 decimals, 1.1255 rounds to 1.126 (round half up)
        assert_eq!(amount.format_display(3), "1.126 USD");

        // With 2 decimals, 1.1255 rounds to 1.13
        assert_eq!(amount.format_display(2), "1.13 USD");
    }

    #[test]
    fn test_format_display_zero() {
        let amount = Amount::parse("0 ETH", None).unwrap();
        assert_eq!(amount.format(), "0 ETH");
        assert_eq!(amount.format_display(2), "0 ETH");
    }

    #[test]
    fn test_format_display_non_zero_rounds_to_zero() {
        // Very small amount that would round to zero with default decimals
        let amount = Amount::parse("0.000000001 ETH", None).unwrap();
        // With 8 default decimals, this would round to 0, so we fall back to lossless
        assert_eq!(amount.format(), "0.000000001 ETH");
        // Explicitly asking for 0 decimals should also fall back
        assert_eq!(amount.format_display(0), "0.000000001 ETH");
    }

    #[test]
    fn test_serialize_preserves_precision() {
        use serde_json;

        let amount = Amount::parse("1.123456789012345678 ETH", None).unwrap();
        let serialized = serde_json::to_string(&amount).unwrap();
        // Serialization preserves all significant digits
        assert!(serialized.contains("1.123456789012345678 ETH"));

        // Roundtrip
        let deserialized: Amount = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, amount);
    }

    #[test]
    fn test_trailing_zeros_trimmed() {
        // 1.5000 USD should display as 1.5 USD
        let amount = Amount::parse("1.5 USD", None).unwrap();
        assert_eq!(amount.format_display(4), "1.5 USD");

        // 2.0 ETH should display as 2 ETH
        let amount = Amount::parse("2.0 ETH", None).unwrap();
        assert_eq!(amount.format_display(8), "2 ETH");
    }
}

#[cfg(test)]
mod conversion_tests {
    use super::*;

    #[test]
    fn test_convert_usd_to_eth() {
        // $2000/ETH, convert 2000 USD to ETH
        let price = U256::from(200_000_000_000u128); // 8 decimals
        let usd = U256::from(2_000_000_000u128); // 6 decimals = $2000

        let eth = convert_asset_value(usd, Asset::USD, Asset::ETH, price).unwrap();
        assert_eq!(eth, U256::from(1_000_000_000_000_000_000u128)); // 1 ETH
    }

    #[test]
    fn test_convert_eth_to_usd() {
        let price = U256::from(200_000_000_000u128);
        let eth = U256::from(1_000_000_000_000_000_000u128); // 1 ETH

        let usd = convert_asset_value(eth, Asset::ETH, Asset::USD, price).unwrap();
        assert_eq!(usd, U256::from(2_000_000_000u128)); // $2000
    }

    #[test]
    fn test_convert_zkc_to_usd() {
        // $1/ZKC
        let price = U256::from(100_000_000u128);
        let zkc = U256::from(100_000_000_000_000_000_000u128); // 100 ZKC

        let usd = convert_asset_value(zkc, Asset::ZKC, Asset::USD, price).unwrap();
        assert_eq!(usd, U256::from(100_000_000u128)); // $100
    }

    #[test]
    fn test_roundtrip_preserves_value() {
        let price = U256::from(250_000_000_000u128); // $2500
        let eth = U256::from(1_000_000_000_000_000_000u128);

        let usd = convert_asset_value(eth, Asset::ETH, Asset::USD, price).unwrap();
        let eth_back = convert_asset_value(usd, Asset::USD, Asset::ETH, price).unwrap();

        assert_eq!(eth_back, eth);
    }

    #[test]
    fn test_small_amount_precision() {
        let price = U256::from(200_000_000_000u128); // $2000
        let usd = U256::from(1000u128); // $0.001

        let eth = convert_asset_value(usd, Asset::USD, Asset::ETH, price).unwrap();
        // 0.001 / 2000 = 0.0000005 ETH = 500_000_000_000 wei
        assert_eq!(eth, U256::from(500_000_000_000u128));
    }

    #[test]
    fn test_zero_price_returns_error() {
        let result =
            convert_asset_value(U256::from(1_000_000u128), Asset::USD, Asset::ETH, U256::ZERO);
        assert!(matches!(result, Err(ConversionError::DivisionByZero)));
    }

    #[test]
    fn test_usd_to_zkc() {
        // $1/ZKC
        let price = U256::from(100_000_000u128);
        let usd = U256::from(100_000_000u128); // $100

        let zkc = convert_asset_value(usd, Asset::USD, Asset::ZKC, price).unwrap();
        assert_eq!(zkc, U256::from(100_000_000_000_000_000_000u128)); // 100 ZKC
    }
}

#[cfg(test)]
mod scale_decimals_tests {
    use super::*;

    #[test]
    fn test_scale_decimals_same_decimals() {
        let value = U256::from(1_000_000_000_000_000_000u128); // 1.0 with 18 decimals
        let result = scale_decimals(value, 18, 18);
        assert_eq!(result, value);
    }

    #[test]
    fn test_scale_decimals_down_18_to_6() {
        // 1.0 with 18 decimals → 1.0 with 6 decimals
        let value = U256::from(1_000_000_000_000_000_000u128);
        let result = scale_decimals(value, 18, 6);
        assert_eq!(result, U256::from(1_000_000u128));
    }

    #[test]
    fn test_scale_decimals_up_6_to_18() {
        // 1.0 with 6 decimals → 1.0 with 18 decimals
        let value = U256::from(1_000_000u128);
        let result = scale_decimals(value, 6, 18);
        assert_eq!(result, U256::from(1_000_000_000_000_000_000u128));
    }

    #[test]
    fn test_scale_decimals_roundtrip() {
        let original = U256::from(1_500_000_000_000_000_000u128); // 1.5 with 18 decimals
        let scaled_down = scale_decimals(original, 18, 6);
        let scaled_back = scale_decimals(scaled_down, 6, 18);
        assert_eq!(scaled_back, original);
    }

    #[test]
    fn test_scale_decimals_precision_loss() {
        // Value with precision beyond 6 decimals will lose precision when scaling down
        let value = U256::from(1_234_567_890_123_456_789u128); // 1.234567890123456789 with 18 decimals
        let scaled = scale_decimals(value, 18, 6);
        // Should truncate to 1.234567 (1234567 with 6 decimals)
        assert_eq!(scaled, U256::from(1_234_567u128));
    }

    #[test]
    fn test_scale_decimals_zero_value() {
        let result = scale_decimals(U256::ZERO, 18, 6);
        assert_eq!(result, U256::ZERO);

        let result = scale_decimals(U256::ZERO, 6, 18);
        assert_eq!(result, U256::ZERO);
    }

    #[test]
    fn test_scale_decimals_large_value() {
        // Test with large values
        let value = U256::from(1_000_000_000_000_000_000_000_000u128); // 1 million with 18 decimals
        let scaled = scale_decimals(value, 18, 6);
        assert_eq!(scaled, U256::from(1_000_000_000_000u128)); // 1 million with 6 decimals
    }
}
