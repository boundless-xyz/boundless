use std::sync::Arc;
use alloy::providers::Provider;
use alloy_chains::NamedChain;
use serde::{Deserialize, Serialize};
use core::time::Duration;
use crate::price_oracle::{PriceQuote, TradingPair, PriceOracleError, AggregationMode, scale_price_from_f64, PriceSource, CompositeOracle, WithStalenessCheck};
use crate::price_oracle::cached_oracle::CachedPriceOracle;
use crate::price_oracle::sources::{ChainlinkSource, CoinGeckoSource, CoinMarketCapSource};

/// Static fallback configuration (NOT a PriceSource!)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StaticPriceConfig {
    /// ETH/USD price as string (e.g., "2000.00")
    pub eth_usd: f64,
    /// ZKC/USD price as string (e.g., "1.00")
    pub zkc_usd: f64,
}

impl StaticPriceConfig {
    /// Get the static price for a trading pair
    pub fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
        let price_f64 = match pair {
            TradingPair::EthUsd => self.eth_usd,
            TradingPair::ZkcUsd => self.zkc_usd,
        };

        let price = scale_price_from_f64(price_f64)?;

        // Static prices are considered always fresh, so we set the timestamp to now.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(PriceQuote::new(price, now))
    }
}

/// Price oracle configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct PriceOracleConfig {
    /// Whether the price oracle is enabled
    pub enabled: bool,
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
    /// Static fallback prices
    pub static_fallback: Option<StaticPriceConfig>,
    /// Maximum age in seconds before a price is considered stale
    pub max_staleness_secs: u64,
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
    /// Build a price oracle from this configuration
    pub async fn build<P>(
        &self,
        provider: P,
    ) -> Result<Option<Arc<CachedPriceOracle>>, PriceOracleError>
    where
        P: Provider + Clone + 'static,
    {
        if !self.enabled {
            return Ok(None);
        }

        let mut sources: Vec<Arc<dyn PriceSource>> = Vec::new();

        // Build on-chain sources
        if let Some(ref onchain) = self.onchain {
            if let Some(ref chainlink_config) = onchain.chainlink {
                if chainlink_config.enabled {
                    let chainlink = ChainlinkSource::for_chain(
                        provider.clone(),
                        NamedChain::Mainnet,
                    )?;
                    if self.max_staleness_secs > 0 {
                        sources.push(Arc::new(WithStalenessCheck::new(chainlink, self.max_staleness_secs)));
                    } else {
                        sources.push(Arc::new(chainlink));
                    }
                }
            }
        }

        // Build off-chain sources
        if let Some(ref offchain) = self.offchain {
            if let Some(ref coingecko_config) = offchain.coingecko {
                if coingecko_config.enabled {
                    let coingecko = CoinGeckoSource::new(
                        Duration::from_secs(self.timeout_secs),
                    )?;
                    if self.max_staleness_secs > 0 {
                        sources.push(Arc::new(WithStalenessCheck::new(coingecko, self.max_staleness_secs)));
                    } else {
                        sources.push(Arc::new(coingecko));
                    }
                }
            }

            // CoinMarketCap
            if let Some(ref cmc_config) = offchain.cmc {
                if cmc_config.enabled {
                    let cmc = CoinMarketCapSource::new(
                        cmc_config.api_key.clone(),
                        Duration::from_secs(self.timeout_secs),
                    )?;
                    if self.max_staleness_secs > 0 {
                        sources.push(Arc::new(WithStalenessCheck::new(cmc, self.max_staleness_secs)));
                    } else {
                        sources.push(Arc::new(cmc));
                    }
                }
            }
        }

        if sources.is_empty() && self.static_fallback.is_none() {
            return Err(PriceOracleError::ConfigError(
                "No price sources configured and no static fallback".to_string()
            ));
        }

        let composite = CompositeOracle::new(
            sources,
            self.static_fallback.clone(),
            self.aggregation_mode,
            self.min_sources,
        );

        let cached = Arc::new(CachedPriceOracle::new(
            Arc::new(composite),
            self.refresh_interval_secs,
        ));

        Ok(Some(cached))
    }
}
