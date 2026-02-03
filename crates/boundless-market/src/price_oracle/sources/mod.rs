/// Price source implementation for chainlink
pub mod chainlink;
/// Price source implementation for coinmarketcap
pub mod cmc;
/// Price source implementation for coingecko
pub mod coingecko;
/// Price source implementation for static prices
pub mod static_source;

pub use chainlink::ChainlinkSource;
pub use cmc::CoinMarketCapSource;
pub use coingecko::CoinGeckoSource;
pub use static_source::StaticPriceSource;
