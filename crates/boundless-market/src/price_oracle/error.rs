use super::TradingPair;
use thiserror::Error;

/// Price oracle error types
#[derive(Error, Debug)]
pub enum PriceOracleError {
    /// All price sources failed for a trading pair
    #[error("All price sources failed for {pair:?}")]
    AllSourcesFailed {
        /// The trading pair that failed
        pair: TradingPair,
        /// Error messages from each source
        errors: Vec<String>,
    },

    /// Insufficient sources succeeded to meet minimum threshold
    #[error("Insufficient sources for {pair:?}: got {got}, need {need}")]
    InsufficientSources {
        /// The trading pair
        pair: TradingPair,
        /// Number of successful sources
        got: u8,
        /// Number of sources required
        need: u8,
    },

    /// RPC error from on-chain source
    #[error("RPC error: {0}")]
    RpcError(#[from] alloy::contract::Error),

    /// HTTP error from off-chain source
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),

    /// Invalid price data received
    #[error("Invalid price data: {0}")]
    InvalidPrice(String),

    /// Stale price data
    #[error("Stale price data: age {age_secs}s exceeds max {max_secs}s")]
    StalePrice {
        /// Age of the price in seconds
        age_secs: u64,
        /// Maximum allowed age in seconds
        max_secs: u64,
    },

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Price oracle could not be updated for too long
    #[error("Price oracle could not be updated for too long, shutting down. Please make sure it is correctly configured or set static prices in the config.")]
    UpdateTimeout(),
}
