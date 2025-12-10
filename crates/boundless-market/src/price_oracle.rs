// Copyright 2025 Boundless Foundation, Inc.
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

use alloy::primitives::U256;
use alloy::providers::ProviderBuilder;
use alloy::sol;
use alloy_primitives::Signed;
use reqwest::Client;
use serde::Deserialize;
use thiserror::Error;

const SCALE_DECIMALS: u32 = 8;
const SCALE: u128 = 100_000_000; // 10^8
const WEI_SCALE_U128: u128 = 1_000_000_000_000_000_000; // 1e18

// Define the ABI interface using alloy-sol-macro if available
sol! {
    #[sol(rpc)]
    interface AggregatorV3Interface {
        function latestRoundData()
            external
            view
            returns (
                uint80 roundId,
                int256 answer,
                uint256 startedAt,
                uint256 updatedAt,
                uint80 answeredInRound
            );
    }
}

impl EthPriceClient {
    async fn fetch_onchain_oracle(&self, cfg: &OnchainOracleConfig) -> Result<U256, EthPriceError> {
        // Build provider
        let provider = ProviderBuilder::new().connect_http(
            cfg.rpc_url.parse().map_err(|_| EthPriceError::InvalidData("bad rpc url"))?,
        );

        // Bind contract
        let oracle = AggregatorV3Interface::new(cfg.oracle_address, &provider);

        let result = oracle
            .latestRoundData()
            .call()
            .await
            .map_err(|_| EthPriceError::InvalidData("oracle call failed"))?;
        let answer = result.answer;

        if answer <= Signed::ZERO {
            return Err(EthPriceError::InvalidData("non-positive oracle answer"));
        }

        let ans_u256 = U256::from(answer);

        // Oracle answer is typically scaled by 10^decimals (e.g. 1e8).
        // We want to normalize to our internal SCALE_DECIMALS (also 1e8 in our earlier code).
        //
        // If oracle_decimals == SCALE_DECIMALS, we can just return `ans_u256`.
        // Otherwise, rescale:
        let oracle_decimals = cfg.decimals;
        if oracle_decimals == SCALE_DECIMALS as u8 {
            Ok(ans_u256)
        } else if oracle_decimals < SCALE_DECIMALS as u8 {
            let diff = (SCALE_DECIMALS as u8 - oracle_decimals) as u32;
            Ok(ans_u256 * U256::from(10u64.pow(diff)))
        } else {
            let diff = (oracle_decimals - SCALE_DECIMALS as u8) as u32;
            Ok(ans_u256 / U256::from(10u64.pow(diff)))
        }
    }
}

#[derive(Debug, Error)]
pub enum EthPriceError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Provider returned invalid or missing data: {0}")]
    InvalidData(&'static str),

    #[error("All providers failed")]
    AllProvidersFailed,

    #[error("Division by zero (zero price?)")]
    DivisionByZero,
}

#[derive(Debug, Clone)]
pub enum PriceSource {
    CoinGecko,
    Coinbase,
    Binance,
    Onchain(OnchainOracleConfig),
}

#[derive(Debug, Clone)]
pub struct OnchainOracleConfig {
    pub rpc_url: String,
    pub oracle_address: alloy::primitives::Address,
    /// Number of decimals in the oracle answer (e.g. 8 for many Chainlink feeds).
    pub decimals: u8,
}

pub struct EthPriceClient {
    http: Client,
    sources: Vec<PriceSource>,
}

impl EthPriceClient {
    pub fn new_default() -> Self {
        Self {
            http: Client::new(),
            sources: vec![PriceSource::CoinGecko, PriceSource::Coinbase, PriceSource::Binance],
        }
    }

    pub fn new_with_onchain(oracle: OnchainOracleConfig) -> Self {
        Self {
            http: Client::new(),
            sources: vec![
                PriceSource::Onchain(oracle),
                PriceSource::CoinGecko,
                PriceSource::Coinbase,
                PriceSource::Binance,
            ],
        }
    }

    // ----------------- Public API -----------------

    /// Fetch ETH/USD price as a fixed-point U256 scaled by 10^SCALE_DECIMALS.
    pub async fn fetch_eth_usd(&self) -> Result<U256, EthPriceError> {
        let mut last_err: Option<EthPriceError> = None;

        for source in &self.sources {
            let res = match source {
                PriceSource::CoinGecko => self.fetch_coingecko().await,
                PriceSource::Coinbase => self.fetch_coinbase().await,
                PriceSource::Binance => self.fetch_binance().await,
                PriceSource::Onchain(cfg) => self.fetch_onchain_oracle(cfg).await,
            };

            match res {
                Ok(price) => return Ok(price),
                Err(e) => {
                    // log source + error if you want
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or(EthPriceError::AllProvidersFailed))
    }

    /// High-level helper:
    /// read a USD amount as a decimal string (e.g. "1.23") and convert it to wei
    /// using the current ETH/USD price.
    pub async fn usd_str_to_wei(&self, usd_amount: &str) -> Result<U256, EthPriceError> {
        let price_scaled = self.fetch_eth_usd().await?; // ETH/USD, scaled 1e8
        if price_scaled.is_zero() {
            return Err(EthPriceError::DivisionByZero);
        }

        // Parse USD amount string with the same 1e8 scale
        let usd_scaled = Self::price_str_to_scaled_u256(usd_amount)?;

        // wei = usd_scaled / price_usd * 1e18
        //      = usd_scaled * 1e18 / price_scaled
        let num = usd_scaled
            .checked_mul(U256::from(WEI_SCALE_U128))
            .ok_or(EthPriceError::InvalidData("overflow in usd_to_wei mul"))?;

        Ok(num / price_scaled)
    }

    // ----------------- Shared parser -----------------

    /// Parse a decimal string (e.g. "2345.12") into a U256 scaled by 10^SCALE_DECIMALS.
    fn price_str_to_scaled_u256(s: &str) -> Result<U256, EthPriceError> {
        let s = s.trim();

        if s.is_empty() {
            return Err(EthPriceError::InvalidData("empty price string"));
        }

        // No sign handling; extend if you ever need it.
        let mut parts = s.split('.');

        let int_part = parts.next().ok_or(EthPriceError::InvalidData("missing integer part"))?;
        let frac_part_raw = parts.next().unwrap_or("");

        if parts.next().is_some() {
            return Err(EthPriceError::InvalidData("too many decimal points"));
        }

        let int_val = int_part
            .parse::<u128>()
            .map_err(|_| EthPriceError::InvalidData("invalid integer part"))?;

        let mut frac_str: String = frac_part_raw.chars().take(SCALE_DECIMALS as usize).collect();

        while frac_str.len() < SCALE_DECIMALS as usize {
            frac_str.push('0');
        }

        let frac_val = if frac_str.is_empty() {
            0u128
        } else {
            frac_str
                .parse::<u128>()
                .map_err(|_| EthPriceError::InvalidData("invalid fractional part"))?
        };

        let scaled_int = int_val
            .checked_mul(SCALE)
            .ok_or(EthPriceError::InvalidData("overflow in scaled int"))?;

        let scaled = scaled_int
            .checked_add(frac_val)
            .ok_or(EthPriceError::InvalidData("overflow in scaled add"))?;

        Ok(U256::from(scaled))
    }

    // ----------------- Provider implementations -----------------

    async fn fetch_coingecko(&self) -> Result<U256, EthPriceError> {
        #[derive(Debug, Deserialize)]
        struct CoinGeckoEth {
            usd: f64,
        }

        #[derive(Debug, Deserialize)]
        struct CoinGeckoResp {
            ethereum: CoinGeckoEth,
        }

        let url = "https://api.coingecko.com/api/v3/simple/price?ids=ethereum&vs_currencies=usd";
        let resp: CoinGeckoResp =
            self.http.get(url).send().await?.error_for_status()?.json().await?;

        let s = resp.ethereum.usd.to_string();
        Self::price_str_to_scaled_u256(&s)
    }

    async fn fetch_coinbase(&self) -> Result<U256, EthPriceError> {
        #[derive(Debug, Deserialize)]
        struct CoinbaseInner {
            amount: String,
        }

        #[derive(Debug, Deserialize)]
        struct CoinbaseResp {
            data: CoinbaseInner,
        }

        let url = "https://api.coinbase.com/v2/prices/ETH-USD/spot";
        let resp: CoinbaseResp =
            self.http.get(url).send().await?.error_for_status()?.json().await?;

        Self::price_str_to_scaled_u256(&resp.data.amount)
    }

    async fn fetch_binance(&self) -> Result<U256, EthPriceError> {
        #[derive(Debug, Deserialize)]
        struct BinanceResp {
            price: String,
        }

        let url = "https://api.binance.com/api/v3/ticker/price?symbol=ETHUSDT";
        let resp: BinanceResp = self.http.get(url).send().await?.error_for_status()?.json().await?;

        Self::price_str_to_scaled_u256(&resp.price)
    }
}
