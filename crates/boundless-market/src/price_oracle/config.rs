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

use crate::price_oracle::cached_oracle::CachedPriceOracle;
use crate::price_oracle::manager::PriceOracleManager;
use crate::price_oracle::sources::{
    ChainlinkSource, CoinGeckoSource, CoinMarketCapSource, StaticPriceSource,
};
use crate::price_oracle::{
    AggregationMode, CompositeOracle, PriceOracle, PriceOracleError, PriceSource, TradingPair,
};
use alloy::providers::Provider;
use alloy_chains::NamedChain;
use core::time::Duration;
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::Arc;

/// Price value configuration: either "auto" for dynamic fetching or a static numeric value
#[derive(Debug, Clone, PartialEq)]
pub enum PriceValue {
    /// Fetch price dynamically from configured sources
    Auto,
    /// Use a static price value
    Static(f64),
}

impl Default for PriceValue {
    fn default() -> Self {
        Self::Auto
    }
}

impl<'de> Deserialize<'de> for PriceValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        if s.to_lowercase() == "auto" {
            Ok(PriceValue::Auto)
        } else {
            let value = s.parse::<f64>().map_err(|e| {
                serde::de::Error::custom(format!(
                    "Invalid price value '{}': must be 'auto' or a valid number: {}",
                    s, e
                ))
            })?;
            if !value.is_finite() || value <= 0.0 {
                return Err(serde::de::Error::custom(format!(
                    "Invalid price value '{}': must be >0 and finite",
                    s
                )));
            }
            Ok(PriceValue::Static(value))
        }
    }
}

impl Serialize for PriceValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PriceValue::Auto => serializer.serialize_str("auto"),
            PriceValue::Static(value) => serializer.serialize_str(&value.to_string()),
        }
    }
}

/// Price oracle configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PriceOracleConfig {
    /// ETH/USD price: "auto" for dynamic or a static value like "2500.00"
    pub eth_usd: PriceValue,
    /// ZKC/USD price: "auto" for dynamic or a static value like "1.00"
    pub zkc_usd: PriceValue,
    /// Maximum seconds without a successful price update before prover exits.
    /// This prevents the system from operating with stale prices when all sources are failing.
    /// Set to 0 to disable this check.
    pub max_secs_without_price_update: u64,
    /// Refresh interval in seconds
    pub refresh_interval_secs: u64,
    /// HTTP/RPC timeout in seconds
    pub timeout_secs: u64,
    /// Aggregation mode for combining multiple sources
    pub aggregation_mode: AggregationMode,
    /// Minimum number of successful sources required
    pub min_sources: u8,
    /// On-chain source configuration
    pub onchain: Option<OnChainConfig>,
    /// Off-chain source configuration
    pub offchain: Option<OffChainConfig>,
}

impl Default for PriceOracleConfig {
    fn default() -> Self {
        Self {
            eth_usd: PriceValue::Auto,
            zkc_usd: PriceValue::Auto,
            max_secs_without_price_update: 7200,
            refresh_interval_secs: 60,
            timeout_secs: 10,
            aggregation_mode: AggregationMode::Priority,
            min_sources: 1,
            // Enable both Chainlink and CoinGecko by default
            onchain: Some(OnChainConfig { chainlink: Some(ChainlinkConfig { enabled: true }) }),
            offchain: Some(OffChainConfig {
                coingecko: Some(CoinGeckoConfig { enabled: true, api_key: None }),
                cmc: None,
            }),
        }
    }
}

/// On-chain price source configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OnChainConfig {
    /// Chainlink price feed configuration
    pub chainlink: Option<ChainlinkConfig>,
}

/// Chainlink price feed configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainlinkConfig {
    /// Whether Chainlink is enabled
    pub enabled: bool,
}

/// Off-chain price source configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OffChainConfig {
    /// CoinGecko configuration
    pub coingecko: Option<CoinGeckoConfig>,
    /// CoinMarketCap configuration
    pub cmc: Option<CoinMarketCapConfig>,
}

/// CoinGecko API configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoinGeckoConfig {
    /// Whether CoinGecko is enabled
    pub enabled: bool,
    /// API key (optional, for pro tier)
    pub api_key: Option<String>,
}

/// CoinMarketCap API configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoinMarketCapConfig {
    /// Whether CoinMarketCap is enabled
    pub enabled: bool,
    /// API key (required)
    pub api_key: String,
}

impl PriceOracleConfig {
    /// Build price oracle manager from this configuration
    pub fn build<P>(
        &self,
        named_chain: NamedChain,
        provider: P,
    ) -> Result<PriceOracleManager, PriceOracleError>
    where
        P: Provider + Clone + 'static,
    {
        // Build ETH/USD oracle
        let eth_usd = self.build_oracle_for_pair(
            TradingPair::EthUsd,
            &self.eth_usd,
            named_chain,
            provider.clone(),
        )?;

        // Build ZKC/USD oracle
        let zkc_usd = self.build_oracle_for_pair(
            TradingPair::ZkcUsd,
            &self.zkc_usd,
            named_chain,
            provider.clone(),
        )?;

        Ok(PriceOracleManager::new(
            eth_usd,
            zkc_usd,
            self.refresh_interval_secs,
            self.max_secs_without_price_update,
        ))
    }

    fn build_oracle_for_pair<P>(
        &self,
        pair: TradingPair,
        price_value: &PriceValue,
        named_chain: NamedChain,
        provider: P,
    ) -> Result<Arc<CachedPriceOracle>, PriceOracleError>
    where
        P: Provider + Clone + 'static,
    {
        // Build the inner oracle based on whether we have static or dynamic pricing
        let inner: Arc<dyn PriceOracle> = match price_value {
            PriceValue::Static(price) => {
                // Static price: use source directly, no composite needed
                Arc::new(StaticPriceSource::new(pair, *price))
            }
            PriceValue::Auto => {
                // Dynamic pricing: build sources and wrap in CompositeOracle
                let mut sources: Vec<Arc<dyn PriceSource>> = Vec::new();

                // On-chain sources
                if let Some(ref onchain) = self.onchain {
                    if let Some(ref chainlink_config) = onchain.chainlink {
                        if chainlink_config.enabled && pair == TradingPair::EthUsd {
                            // Chainlink only supports ETH/USD
                            let chainlink =
                                ChainlinkSource::for_eth_usd(provider.clone(), named_chain)?;
                            sources.push(Arc::new(chainlink));
                        }
                    }
                }

                // Off-chain sources
                if let Some(ref offchain) = self.offchain {
                    // CoinGecko
                    if let Some(ref coingecko_config) = offchain.coingecko {
                        if coingecko_config.enabled {
                            let coingecko =
                                CoinGeckoSource::new(pair, Duration::from_secs(self.timeout_secs))?;
                            sources.push(Arc::new(coingecko));
                        }
                    }

                    // CoinMarketCap
                    if let Some(ref cmc_config) = offchain.cmc {
                        if cmc_config.enabled {
                            let cmc = CoinMarketCapSource::new(
                                pair,
                                cmc_config.api_key.clone(),
                                Duration::from_secs(self.timeout_secs),
                            )?;
                            sources.push(Arc::new(cmc));
                        }
                    }
                }

                // Ensure we have at least one source. This should only happen if misconfigured by user.
                if sources.is_empty() {
                    return Err(PriceOracleError::ConfigError(
                        format!("No price sources configured for {}: ensure at least one source is enabled when using 'auto' pricing or specify a static price", pair)
                    ));
                }

                Arc::new(CompositeOracle::new(
                    pair,
                    sources,
                    self.aggregation_mode,
                    self.min_sources,
                ))
            }
        };

        // Wrap in CachedPriceOracle for consistent API
        Ok(Arc::new(CachedPriceOracle::new(inner)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_price_value_deserialize_auto() {
        let json = r#""auto""#;
        let value: PriceValue = serde_json::from_str(json).unwrap();
        assert_eq!(value, PriceValue::Auto);

        let json = r#""AUTO""#;
        let value: PriceValue = serde_json::from_str(json).unwrap();
        assert_eq!(value, PriceValue::Auto);

        let json = r#""Auto""#;
        let value: PriceValue = serde_json::from_str(json).unwrap();
        assert_eq!(value, PriceValue::Auto);
    }

    #[test]
    fn test_price_value_deserialize_static_various_formats() {
        let test_cases = vec![
            ("1.00", 1.00),
            ("0.50", 0.50),
            ("2500.00", 2500.0),
            ("2500.5", 2500.5),
            ("0.00001", 0.00001),
        ];

        for (input, expected) in test_cases {
            let json = format!(r#""{}""#, input);
            let value: PriceValue = serde_json::from_str(&json).unwrap();
            assert_eq!(value, PriceValue::Static(expected));
        }
    }

    #[test]
    fn test_price_value_deserialize_invalid() {
        let json = r#""not_a_number""#;
        let result: Result<PriceValue, _> = serde_json::from_str(json);
        assert!(result.is_err());

        let json = r#""-100.00""#;
        let result: Result<PriceValue, _> = serde_json::from_str(json);
        assert!(result.is_err());

        let json = r#""0.0""#;
        let result: Result<PriceValue, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_price_value_serialize() {
        let auto = PriceValue::Auto;
        let serialized = serde_json::to_string(&auto).unwrap();
        assert!(serialized.contains("auto"));

        let static_val = PriceValue::Static(2500.00);
        let serialized = serde_json::to_string(&static_val).unwrap();
        assert!(serialized.contains("2500"));
    }

    #[test]
    fn test_price_value_roundtrip() {
        let values = vec![PriceValue::Auto, PriceValue::Static(1.00), PriceValue::Static(2500.50)];

        for original in values {
            let serialized = serde_json::to_string(&original).unwrap();
            let deserialized: PriceValue = serde_json::from_str(&serialized).unwrap();
            assert_eq!(original, deserialized);
        }
    }
}
