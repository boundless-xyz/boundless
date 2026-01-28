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
