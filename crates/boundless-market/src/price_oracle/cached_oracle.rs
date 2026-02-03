use crate::price_oracle::{PriceOracle, PriceOracleError, PriceQuote};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cached price oracle that wraps another oracle and caches its latest price
#[derive(Clone)]
pub struct CachedPriceOracle {
    oracle: Arc<dyn PriceOracle>,
    cache: Arc<RwLock<Option<PriceQuote>>>,
}

impl CachedPriceOracle {
    /// Create a new cached price oracle
    pub fn new(oracle: Arc<dyn PriceOracle>) -> Self {
        Self { oracle, cache: Arc::new(RwLock::new(None)) }
    }

    /// Get cached price (non-blocking read)
    pub async fn get_cached_price(&self) -> Option<PriceQuote> {
        let cache = self.cache.read().await;
        *cache
    }

    /// Refresh the cached price from the underlying oracle
    pub async fn refresh_price(&self) {
        match self.oracle.get_price().await {
            Ok(quote) => {
                tracing::debug!(
                    "Refreshed price: {} (timestamp: {})",
                    quote.price,
                    quote.timestamp
                );
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
        // In practice this should never happen, as we fetch when we initialize the oracle.
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use tokio::sync::Mutex;

    /// Mock oracle with configurable behavior for testing
    struct MockOracle {
        price: Mutex<U256>,
        call_count: Arc<AtomicU64>,
        should_error: AtomicBool,
    }

    impl MockOracle {
        fn new(price: U256) -> Self {
            Self {
                price: Mutex::new(price),
                call_count: Arc::new(AtomicU64::new(0)),
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
            self.call_count.fetch_add(1, Ordering::SeqCst);

            if self.should_error.load(Ordering::SeqCst) {
                return Err(PriceOracleError::Internal("Mock error".to_string()));
            }

            let price = *self.price.lock().await;
            Ok(PriceQuote::new(price, 1000))
        }
    }

    #[tokio::test]
    async fn test_get_cached_price_returns_none_initially() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle);

        // Cache should be empty initially
        let cached = cached_oracle.get_cached_price().await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_get_price_fetches_and_caches_on_miss() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // First call should fetch from oracle
        let result = cached_oracle.get_price().await.unwrap();
        assert_eq!(result.price, U256::from(200000000000u128));
        assert_eq!(oracle.get_call_count(), 1);

        // Cache should now have the value
        let cached = cached_oracle.get_cached_price().await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().price, U256::from(200000000000u128));
    }

    #[tokio::test]
    async fn test_get_price_returns_cached_value() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // First call fetches
        let result1 = cached_oracle.get_price().await.unwrap();
        assert_eq!(result1.price, U256::from(200000000000u128));

        // Change underlying price
        oracle.set_price(U256::from(300000000000u128)).await;

        // Second call should return cached value, not fetch again
        let result2 = cached_oracle.get_price().await.unwrap();
        assert_eq!(result2.price, U256::from(200000000000u128));
        assert_eq!(oracle.get_call_count(), 1, "Should only fetch once");
    }

    #[tokio::test]
    async fn test_refresh_price_updates_cache() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // Initial fetch
        let _ = cached_oracle.get_price().await.unwrap();
        assert_eq!(oracle.get_call_count(), 1);

        // Change underlying price
        oracle.set_price(U256::from(300000000000u128)).await;

        // Refresh should update cache
        cached_oracle.refresh_price().await;
        assert_eq!(oracle.get_call_count(), 2);

        // Get price should now return updated value
        let result = cached_oracle.get_price().await.unwrap();
        assert_eq!(result.price, U256::from(300000000000u128));
        assert_eq!(oracle.get_call_count(), 2, "Should not fetch again");
    }

    #[tokio::test]
    async fn test_refresh_error_preserves_cache() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

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
    }

    #[tokio::test]
    async fn test_get_price_propagates_error_on_cache_miss() {
        let oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        oracle.set_should_error(true);
        let cached_oracle = CachedPriceOracle::new(oracle);

        // With empty cache and oracle error, get_price should fail
        let result = cached_oracle.get_price().await;
        assert!(result.is_err());
    }
}
