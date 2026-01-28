use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote, TradingPair};
use futures::future::join_all;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::{sync::RwLock, sync::Mutex, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// Cached price oracle with background refresh
pub struct CachedPriceOracle {
    oracle: Arc<dyn PriceOracle>,
    cache: Arc<RwLock<HashMap<TradingPair, PriceQuote>>>,
    refresh_interval: Duration,
}

impl CachedPriceOracle {
    /// Create a new cached price oracle
    pub fn new(oracle: Arc<dyn PriceOracle>, refresh_interval_secs: u64) -> Self {
        Self {
            oracle,
            cache: Arc::new(RwLock::new(HashMap::new())),
            refresh_interval: Duration::from_secs(refresh_interval_secs),
        }
    }

    /// Get cached price (non-blocking read)
    pub async fn get_cached_price(&self, pair: TradingPair) -> Option<PriceQuote> {
        let cache = self.cache.read().await;
        cache.get(&pair).copied()
    }

    /// Spawn background refresh task
    pub fn spawn_refresh_task(self, cancel_token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let this = Arc::new(self);

        tokio::spawn(async move {
            tracing::info!("Price oracle refresh task started (interval: {}s)", this.refresh_interval.as_secs());

            let mut ticker = tokio::time::interval(this.refresh_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            // Do an initial refresh immediately
            this.refresh_prices().await;

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        this.refresh_prices().await;
                    }
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Price oracle refresh task shutting down");
                        break;
                    }
                }
            }
        })
    }

    async fn refresh_prices(&self) {
        let pairs = [TradingPair::EthUsd, TradingPair::ZkcUsd];

        // Fetch all prices concurrently
        let results = join_all(pairs.map(|pair| async move {
            (pair, self.oracle.get_price(pair).await)
        })).await;

        // Process results
        let mut cache = self.cache.write().await;
        for (pair, result) in results {
            match result {
                Ok(quote) => {
                    tracing::debug!("Refreshed {} price: {} (timestamp: {})", pair, quote.price, quote.timestamp);
                    cache.insert(pair, quote);
                }
                Err(e) => {
                    tracing::error!("Failed to refresh {} price: {}", pair, e);
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl PriceOracle for CachedPriceOracle {
    async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
        // Try to get from cache first
        if let Some(quote) = self.get_cached_price(pair).await {
            return Ok(quote);
        }

        // If not in cache, fetch from oracle and cache it.
        // In practice this should never happen, as we fetch when we initialize the spawn_refresh_task.
        tracing::warn!("Cache miss for {:?}, fetching from oracle", pair);

        let quote = self.oracle.get_price(pair).await?;
        let mut cache = self.cache.write().await;
        cache.insert(pair, quote);
        Ok(quote)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use crate::price_oracle::PriceOracle;

    struct MockOracle {
        price: Mutex<U256>,
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockOracle {
        async fn get_price(&self, _pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
            let price = *self.price.lock().await;
            Ok(PriceQuote::new(price, 1000))
        }
    }

    #[tokio::test]
    async fn test_caching() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle {
            price: Mutex::new(U256::from(200000000000u128)),
        });

        let cached_oracle = CachedPriceOracle::new(oracle.clone(), 60);

        // First call should fetch from oracle
        let result1 = cached_oracle.get_price(TradingPair::EthUsd).await?;
        assert_eq!(result1.price, U256::from(200000000000u128));

        *oracle.price.lock().await = U256::from(300000000000u128); // Change underlying price

        // Second call should return cached value
        let result2 = cached_oracle.get_price(TradingPair::EthUsd).await?;
        assert_eq!(result2.price, U256::from(200000000000u128));

        // Refresh prices
        cached_oracle.refresh_prices().await;

        // After refresh, should get updated price
        let result3 = cached_oracle.get_price(TradingPair::EthUsd).await?;
        assert_eq!(result3.price, U256::from(300000000000u128));

        Ok(())
    }

    // TODO: add more tests for the refresh task and error handling
}

