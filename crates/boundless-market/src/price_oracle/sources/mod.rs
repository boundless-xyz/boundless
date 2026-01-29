
/// Price source implementation for chainlink
pub mod chainlink;
/// Price source implementation for coingecko
pub mod coingecko;
/// Price source implementation for coinmarketcap
pub mod cmc;

pub use chainlink::ChainlinkSource;
pub use coingecko::CoinGeckoSource;
pub use cmc::CoinMarketCapSource;
