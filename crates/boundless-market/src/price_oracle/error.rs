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

use super::TradingPair;
use thiserror::Error;

/// Price oracle error types
#[derive(Error, Debug)]
pub enum PriceOracleError {
    /// All price sources failed for a trading pair
    #[error("{code} All price sources failed for {pair:?}", code = self.code())]
    AllSourcesFailed {
        /// The trading pair that failed
        pair: TradingPair,
        /// Error messages from each source
        errors: Vec<String>,
    },

    /// Insufficient sources succeeded to meet minimum threshold
    #[error("{code} Insufficient sources for {pair:?}: got {got}, need {need}", code = self.code())]
    InsufficientSources {
        /// The trading pair
        pair: TradingPair,
        /// Number of successful sources
        got: u8,
        /// Number of sources required
        need: u8,
    },

    /// RPC error from on-chain source
    #[error("{code} RPC error: {0}", code = self.code())]
    RpcError(#[from] alloy::contract::Error),

    /// HTTP error from off-chain source
    #[error("{code} HTTP error: {0}", code = self.code())]
    HttpError(#[from] reqwest::Error),

    /// Invalid price data received
    #[error("{code} Invalid price data: {0}", code = self.code())]
    InvalidPrice(String),

    /// Stale price data
    #[error("{code} Stale price data: age {age_secs}s exceeds max {max_secs}s", code = self.code())]
    StalePrice {
        /// Age of the price in seconds
        age_secs: u64,
        /// Maximum allowed age in seconds
        max_secs: u64,
    },

    /// Configuration error
    #[error("{code} Configuration error: {0}", code = self.code())]
    ConfigError(String),

    /// Internal error
    #[error("{code} Internal error: {0}", code = self.code())]
    Internal(String),

    /// Price oracle could not be updated for too long
    #[error("{code} Price oracle could not be updated for too long, shutting down. Please make sure it is correctly configured or set static prices in the config.", code = self.code())]
    UpdateTimeout(),
}

impl PriceOracleError {
    /// Returns the error code for this error variant
    pub fn code(&self) -> &str {
        match self {
            PriceOracleError::AllSourcesFailed { .. } => "[B-PO-001]",
            PriceOracleError::InsufficientSources { .. } => "[B-PO-002]",
            PriceOracleError::RpcError(_) => "[B-PO-003]",
            PriceOracleError::HttpError(_) => "[B-PO-004]",
            PriceOracleError::InvalidPrice(_) => "[B-PO-005]",
            PriceOracleError::StalePrice { .. } => "[B-PO-006]",
            PriceOracleError::ConfigError(_) => "[B-PO-007]",
            PriceOracleError::Internal(_) => "[B-PO-008]",
            PriceOracleError::UpdateTimeout() => "[B-PO-009]",
        }
    }
}
