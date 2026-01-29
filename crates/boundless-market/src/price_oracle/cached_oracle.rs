use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote, TradingPair};
use futures::future::join_all;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{sync::RwLock, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// Cached price oracle with background refresh
#[derive(Clone)]
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
    pub fn spawn_refresh_task(self: Arc<Self>, cancel_token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let this = self;

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
    use tokio::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Mock oracle with configurable behavior for testing
    struct MockOracle {
        /// Prices per trading pair
        prices: Mutex<HashMap<TradingPair, U256>>,
        /// Call counts per trading pair
        call_counts: Arc<Mutex<HashMap<TradingPair, u64>>>,
        /// Delay to simulate slow fetches
        delay: Duration,
        /// Whether to return errors
        should_error: AtomicBool,
    }

    impl MockOracle {
        fn new() -> Self {
            let mut prices = HashMap::new();
            prices.insert(TradingPair::EthUsd, U256::from(200000000000u128));
            prices.insert(TradingPair::ZkcUsd, U256::from(100000000u128));

            Self {
                prices: Mutex::new(prices),
                call_counts: Arc::new(Mutex::new(HashMap::new())),
                delay: Duration::from_millis(0),
                should_error: AtomicBool::new(false),
            }
        }

        fn with_delay(mut self, delay: Duration) -> Self {
            self.delay = delay;
            self
        }

        async fn set_price(&self, pair: TradingPair, price: U256) {
            self.prices.lock().await.insert(pair, price);
        }

        fn set_should_error(&self, should_error: bool) {
            self.should_error.store(should_error, Ordering::SeqCst);
        }

        async fn get_call_count(&self, pair: TradingPair) -> u64 {
            *self.call_counts.lock().await.get(&pair).unwrap_or(&0)
        }
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockOracle {
        async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
            // Track call counts
            {
                let mut counts = self.call_counts.lock().await;
                *counts.entry(pair).or_insert(0) += 1;
            }

            // Simulate delay
            if self.delay > Duration::from_millis(0) {
                tokio::time::sleep(self.delay).await;
            }

            // Check if we should error
            if self.should_error.load(Ordering::SeqCst) {
                return Err(PriceOracleError::Internal("Mock error".to_string()));
            }

            // Return price
            let prices = self.prices.lock().await;
            let price = *prices.get(&pair).ok_or_else(|| {
                PriceOracleError::Internal(format!("No price for {:?}", pair))
            })?;

            Ok(PriceQuote::new(price, 1000))
        }
    }

    #[tokio::test]
    async fn test_caching() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new());
        let cached_oracle = CachedPriceOracle::new(oracle.clone(), 60);

        // First call should fetch from oracle
        let result1 = cached_oracle.get_price(TradingPair::EthUsd).await?;
        assert_eq!(result1.price, U256::from(200000000000u128));

        // Change underlying price
        oracle.set_price(TradingPair::EthUsd, U256::from(300000000000u128)).await;

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

    #[tokio::test]
    async fn test_spawn_refresh_task_initial_refresh_and_cancellation() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new());
        let cached_oracle = Arc::new(CachedPriceOracle::new(oracle.clone(), 60));

        // Clone to keep a reference for checking cache
        let cached_oracle_ref = cached_oracle.clone();

        // Spawn the refresh task
        let cancel_token = CancellationToken::new();
        let handle = cached_oracle.spawn_refresh_task(cancel_token.clone());

        // Wait a bit for the initial refresh to complete
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify both trading pairs are cached
        let eth_price = cached_oracle_ref.get_cached_price(TradingPair::EthUsd).await.expect("EthUsd should be in cache after initial refresh");
        let zkc_price = cached_oracle_ref.get_cached_price(TradingPair::ZkcUsd).await.expect("ZkcUsd should be in cache after initial refresh");

        assert_eq!(eth_price.price, U256::from(200000000000u128));
        assert_eq!(zkc_price.price, U256::from(100000000u128));

        // Cleanup
        cancel_token.cancel();

        // Verify the task completes successfully
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await?;
        assert!(result.is_ok(), "Task should complete within timeout");

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_refresh_task_periodic_refresh() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new());
        // Use 1 second refresh interval
        let cached_oracle = Arc::new(CachedPriceOracle::new(oracle.clone(), 1));
        let cached_oracle_ref = cached_oracle.clone();

        let cancel_token = CancellationToken::new();
        let handle = cached_oracle.clone().spawn_refresh_task(cancel_token.clone());

        // Wait for initial refresh (the task does an immediate refresh, then the first
        // tick() completes immediately too, so we get 2 refreshes right away)
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify initial refreshes happened (2 due to immediate tick behavior)
        let initial_eth_count = oracle.get_call_count(TradingPair::EthUsd).await;
        let initial_zkc_count = oracle.get_call_count(TradingPair::ZkcUsd).await;
        assert_eq!(initial_eth_count, 2, "Initial refresh + first tick = 2");
        assert_eq!(initial_zkc_count, 2, "Initial refresh + first tick = 2");

        // Change the price
        oracle.set_price(TradingPair::EthUsd, U256::from(300000000000u128)).await;

        // Wait for the next refresh cycle (1 second interval)
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify a third refresh happened
        let third_eth_count = oracle.get_call_count(TradingPair::EthUsd).await;
        let third_zkc_count = oracle.get_call_count(TradingPair::ZkcUsd).await;
        assert_eq!(third_eth_count, 3, "Should have exactly 3 refreshes");
        assert_eq!(third_zkc_count, 3, "Should have exactly 3 refreshes");

        // Verify cache was updated with new price
        let cached_price = cached_oracle_ref.get_cached_price(TradingPair::EthUsd).await.unwrap();
        assert_eq!(cached_price.price, U256::from(300000000000u128));

        // Wait for another refresh cycle
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify fourth refresh happened
        let fourth_eth_count = oracle.get_call_count(TradingPair::EthUsd).await;
        assert_eq!(fourth_eth_count, 4, "Should have exactly 4 refreshes");

        // Cleanup
        cancel_token.cancel();
        handle.await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_refresh_error_preserves_cache() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new());
        let cached_oracle = CachedPriceOracle::new(oracle.clone(), 60);

        // Pre-populate cache with valid prices
        cached_oracle.refresh_prices().await;

        let cached_eth = cached_oracle.get_cached_price(TradingPair::EthUsd).await.unwrap();
        let cached_zkc = cached_oracle.get_cached_price(TradingPair::ZkcUsd).await.unwrap();
        assert_eq!(cached_eth.price, U256::from(200000000000u128));
        assert_eq!(cached_zkc.price, U256::from(100000000u128));

        // Configure oracle to return errors
        oracle.set_should_error(true);

        // Attempt refresh (should fail but not clear cache)
        cached_oracle.refresh_prices().await;

        // Verify cache still has the old values
        let still_cached_eth = cached_oracle.get_cached_price(TradingPair::EthUsd).await.unwrap();
        let still_cached_zkc = cached_oracle.get_cached_price(TradingPair::ZkcUsd).await.unwrap();
        assert_eq!(still_cached_eth.price, U256::from(200000000000u128));
        assert_eq!(still_cached_zkc.price, U256::from(100000000u128));

        Ok(())
    }
}
