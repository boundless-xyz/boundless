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

/// Asset and amount types for USD-based pricing
pub mod asset;
/// Cached oracle with background refresh
pub mod cached_oracle;
/// Composite oracle with aggregation
pub mod composite_oracle;
/// Configuration types for price oracle
pub mod config;
/// Error types for price oracle
pub mod error;
/// Exchange rate types and utilities
pub mod exchange_rate;
/// Integration tests (run with --ignored flag)
#[cfg(test)]
mod integration_tests;
mod manager;
/// Price source implementations
pub mod sources;

pub use asset::{
    convert_asset_value, scale_decimals, Amount, Asset, ConversionError, ParseAmountError,
};
pub use cached_oracle::CachedPriceOracle;
pub use composite_oracle::CompositeOracle;
pub use config::PriceOracleConfig;
pub use error::PriceOracleError;
pub use exchange_rate::{
    scale_price_from_f64, scale_price_from_i256, ExchangeRate, TradingPair, PRICE_QUOTE_DECIMALS,
};
pub use manager::PriceOracleManager;

/// Aggregation mode for composite oracle
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AggregationMode {
    /// First successful source wins
    Priority,
    /// Median of all successful sources
    #[default]
    Median,
    /// Average of all successful sources
    Average,
}

/// Price oracle trait - each instance is dedicated to one trading pair
#[async_trait::async_trait]
pub trait PriceOracle: Send + Sync {
    /// The trading pair this oracle provides
    fn pair(&self) -> TradingPair;

    /// Get the current exchange rate
    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError>;
}

/// Trait for price sources - each instance is dedicated to one trading pair
pub trait PriceSource: PriceOracle + Send + Sync {
    /// Returns the name of this price source
    fn name(&self) -> &'static str;
}
