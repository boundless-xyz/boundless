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
