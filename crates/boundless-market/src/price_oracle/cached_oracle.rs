use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote};
use std::{sync::Arc, time::Duration};
use tokio::{sync::RwLock, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// Cached price oracle with background refresh (single trading pair)
#[derive(Clone)]
pub struct CachedPriceOracle {
    oracle: Arc<dyn PriceOracle>,
    cache: Arc<RwLock<Option<PriceQuote>>>,
    refresh_interval: Duration,
}

impl CachedPriceOracle {
    /// Create a new cached price oracle
    pub fn new(oracle: Arc<dyn PriceOracle>, refresh_interval_secs: u64) -> Self {
        Self {
            oracle,
            cache: Arc::new(RwLock::new(None)),
            refresh_interval: Duration::from_secs(refresh_interval_secs),
        }
    }

    /// Get cached price (non-blocking read)
    pub async fn get_cached_price(&self) -> Option<PriceQuote> {
        let cache = self.cache.read().await;
        *cache
    }

    /// Spawn background refresh task
    pub fn spawn_refresh_task(self: Arc<Self>, cancel_token: CancellationToken) -> tokio::task::JoinHandle<()> {
        let this = self;

        tokio::spawn(async move {
            tracing::info!("Price oracle refresh task started (interval: {}s)", this.refresh_interval.as_secs());

            let mut ticker = tokio::time::interval(this.refresh_interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

            // Do an initial refresh immediately
            this.refresh_price().await;

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        this.refresh_price().await;
                    }
                    _ = cancel_token.cancelled() => {
                        tracing::info!("Price oracle refresh task shutting down");
                        break;
                    }
                }
            }
        })
    }

    async fn refresh_price(&self) {
        match self.oracle.get_price().await {
            Ok(quote) => {
                tracing::debug!("Refreshed price: {} (timestamp: {})", quote.price, quote.timestamp);
                let mut cache = self.cache.write().await;
                *cache = Some(quote);
            }
            Err(e) => {
                tracing::error!("Failed to refresh price: {}", e);
            }
        }
    }
}

#[async_trait::async_trait]
impl PriceOracle for CachedPriceOracle {
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
        // Try to get from cache first
        if let Some(quote) = self.get_cached_price().await {
            return Ok(quote);
        }

        // If not in cache, fetch from oracle and cache it.
        // In practice this should never happen, as we fetch when we initialize the spawn_refresh_task.
        tracing::warn!("Cache miss, fetching from oracle");

        let quote = self.oracle.get_price().await?;
        let mut cache = self.cache.write().await;
        *cache = Some(quote);
        Ok(quote)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use crate::price_oracle::PriceOracle;
    use tokio::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    /// Mock oracle with configurable behavior for testing
    struct MockOracle {
        /// Price to return
        price: Mutex<U256>,
        /// Call count
        call_count: Arc<AtomicU64>,
        /// Delay to simulate slow fetches
        delay: Duration,
        /// Whether to return errors
        should_error: AtomicBool,
    }

    impl MockOracle {
        fn new(price: U256) -> Self {
            Self {
                price: Mutex::new(price),
                call_count: Arc::new(AtomicU64::new(0)),
                delay: Duration::from_millis(0),
                should_error: AtomicBool::new(false),
            }
        }

        async fn set_price(&self, price: U256) {
            *self.price.lock().await = price;
        }

        fn set_should_error(&self, should_error: bool) {
            self.should_error.store(should_error, Ordering::SeqCst);
        }

        fn get_call_count(&self) -> u64 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockOracle {
        async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
            // Track call counts
            self.call_count.fetch_add(1, Ordering::SeqCst);

            // Simulate delay
            if self.delay > Duration::from_millis(0) {
                tokio::time::sleep(self.delay).await;
            }

            // Check if we should error
            if self.should_error.load(Ordering::SeqCst) {
                return Err(PriceOracleError::Internal("Mock error".to_string()));
            }

            // Return price
            let price = *self.price.lock().await;

            Ok(PriceQuote::new(price, 1000))
        }
    }

    #[tokio::test]
    async fn test_caching() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone(), 60);

        // First call should fetch from oracle
        let result1 = cached_oracle.get_price().await?;
        assert_eq!(result1.price, U256::from(200000000000u128));

        // Change underlying price
        oracle.set_price(U256::from(300000000000u128)).await;

        // Second call should return cached value
        let result2 = cached_oracle.get_price().await?;
        assert_eq!(result2.price, U256::from(200000000000u128));

        // Refresh price
        cached_oracle.refresh_price().await;

        // After refresh, should get updated price
        let result3 = cached_oracle.get_price().await?;
        assert_eq!(result3.price, U256::from(300000000000u128));

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_refresh_task_initial_refresh_and_cancellation() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = Arc::new(CachedPriceOracle::new(oracle.clone(), 60));

        // Clone to keep a reference for checking cache
        let cached_oracle_ref = cached_oracle.clone();

        // Spawn the refresh task
        let cancel_token = CancellationToken::new();
        let handle = cached_oracle.spawn_refresh_task(cancel_token.clone());

        // Wait a bit for the initial refresh to complete
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify price is cached
        let price = cached_oracle_ref.get_cached_price().await.expect("Price should be in cache after initial refresh");

        assert_eq!(price.price, U256::from(200000000000u128));

        // Cleanup
        cancel_token.cancel();

        // Verify the task completes successfully
        let result = tokio::time::timeout(Duration::from_secs(1), handle).await?;
        assert!(result.is_ok(), "Task should complete within timeout");

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_refresh_task_periodic_refresh() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        // Use 1 second refresh interval
        let cached_oracle = Arc::new(CachedPriceOracle::new(oracle.clone(), 1));
        let cached_oracle_ref = cached_oracle.clone();

        let cancel_token = CancellationToken::new();
        let handle = cached_oracle.clone().spawn_refresh_task(cancel_token.clone());

        // Wait for initial refresh (the task does an immediate refresh, then the first
        // tick() completes immediately too, so we get 2 refreshes right away)
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify initial refreshes happened (2 due to immediate tick behavior)
        let initial_count = oracle.get_call_count();
        assert_eq!(initial_count, 2, "Initial refresh + first tick = 2");

        // Change the price
        oracle.set_price(U256::from(300000000000u128)).await;

        // Wait for the next refresh cycle (1 second interval)
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify a third refresh happened
        let third_count = oracle.get_call_count();
        assert_eq!(third_count, 3, "Should have exactly 3 refreshes");

        // Verify cache was updated with new price
        let cached_price = cached_oracle_ref.get_cached_price().await.unwrap();
        assert_eq!(cached_price.price, U256::from(300000000000u128));

        // Wait for another refresh cycle
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify fourth refresh happened
        let fourth_count = oracle.get_call_count();
        assert_eq!(fourth_count, 4, "Should have exactly 4 refreshes");

        // Cleanup
        cancel_token.cancel();
        handle.await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_refresh_error_preserves_cache() -> anyhow::Result<()> {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone(), 60);

        // Pre-populate cache with valid price
        cached_oracle.refresh_price().await;

        let cached = cached_oracle.get_cached_price().await.unwrap();
        assert_eq!(cached.price, U256::from(200000000000u128));

        // Configure oracle to return errors
        oracle.set_should_error(true);

        // Attempt refresh (should fail but not clear cache)
        cached_oracle.refresh_price().await;

        // Verify cache still has the old value
        let still_cached = cached_oracle.get_cached_price().await.unwrap();
        assert_eq!(still_cached.price, U256::from(200000000000u128));

        Ok(())
    }
}
