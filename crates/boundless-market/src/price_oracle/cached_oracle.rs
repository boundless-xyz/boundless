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

use crate::price_oracle::{ExchangeRate, PriceOracle, PriceOracleError, TradingPair};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Cached price oracle that wraps another oracle and caches its latest rate
#[derive(Clone)]
pub struct CachedPriceOracle {
    oracle: Arc<dyn PriceOracle>,
    cache: Arc<RwLock<Option<ExchangeRate>>>,
}

impl CachedPriceOracle {
    /// Create a new cached price oracle
    pub fn new(oracle: Arc<dyn PriceOracle>) -> Self {
        Self { oracle, cache: Arc::new(RwLock::new(None)) }
    }

    /// Get cached rate (non-blocking read)
    pub async fn get_cached_rate(&self) -> Option<ExchangeRate> {
        let cache = self.cache.read().await;
        *cache
    }

    /// Refresh the cached rate from the underlying oracle
    pub async fn refresh_rate(&self) {
        match self.oracle.get_rate().await {
            Ok(rate) => {
                tracing::debug!(
                    "Refreshed rate for {}: {} (timestamp: {})",
                    rate.pair,
                    rate.rate,
                    rate.timestamp
                );
                let mut cache = self.cache.write().await;
                *cache = Some(rate);
            }
            Err(e) => {
                tracing::error!("Failed to refresh rate: {}", e);
            }
        }
    }
}

#[async_trait::async_trait]
impl PriceOracle for CachedPriceOracle {
    fn pair(&self) -> TradingPair {
        self.oracle.pair()
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        // Try to get from cache first
        if let Some(rate) = self.get_cached_rate().await {
            return Ok(rate);
        }

        // If not in cache, fetch from oracle and cache it.
        // In practice this should never happen, as we fetch when we initialize the oracle.
        tracing::warn!("Cache miss, fetching from oracle");

        let rate = self.oracle.get_rate().await?;
        let mut cache = self.cache.write().await;
        *cache = Some(rate);
        Ok(rate)
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
        pair: TradingPair,
        rate: Mutex<U256>,
        call_count: Arc<AtomicU64>,
        should_error: AtomicBool,
    }

    impl MockOracle {
        fn new(pair: TradingPair, rate: U256) -> Self {
            Self {
                pair,
                rate: Mutex::new(rate),
                call_count: Arc::new(AtomicU64::new(0)),
                should_error: AtomicBool::new(false),
            }
        }

        async fn set_rate(&self, rate: U256) {
            *self.rate.lock().await = rate;
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
        fn pair(&self) -> TradingPair {
            self.pair
        }

        async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            if self.should_error.load(Ordering::SeqCst) {
                return Err(PriceOracleError::Internal("Mock error".to_string()));
            }

            let rate = *self.rate.lock().await;
            Ok(ExchangeRate::new(self.pair, rate, 1000))
        }
    }

    #[tokio::test]
    async fn test_get_cached_rate_returns_none_initially() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle);

        // Cache should be empty initially
        let cached = cached_oracle.get_cached_rate().await;
        assert!(cached.is_none());
    }

    #[tokio::test]
    async fn test_get_rate_fetches_and_caches_on_miss() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // First call should fetch from oracle
        let result = cached_oracle.get_rate().await.unwrap();
        assert_eq!(result.rate, U256::from(200000000000u128));
        assert_eq!(result.pair, TradingPair::EthUsd);
        assert_eq!(oracle.get_call_count(), 1);

        // Cache should now have the value
        let cached = cached_oracle.get_cached_rate().await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().rate, U256::from(200000000000u128));
    }

    #[tokio::test]
    async fn test_get_rate_returns_cached_value() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // First call fetches
        let result1 = cached_oracle.get_rate().await.unwrap();
        assert_eq!(result1.rate, U256::from(200000000000u128));

        // Change underlying rate
        oracle.set_rate(U256::from(300000000000u128)).await;

        // Second call should return cached value, not fetch again
        let result2 = cached_oracle.get_rate().await.unwrap();
        assert_eq!(result2.rate, U256::from(200000000000u128));
        assert_eq!(oracle.get_call_count(), 1, "Should only fetch once");
    }

    #[tokio::test]
    async fn test_refresh_rate_updates_cache() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // Initial fetch
        let _ = cached_oracle.get_rate().await.unwrap();
        assert_eq!(oracle.get_call_count(), 1);

        // Change underlying rate
        oracle.set_rate(U256::from(300000000000u128)).await;

        // Refresh should update cache
        cached_oracle.refresh_rate().await;
        assert_eq!(oracle.get_call_count(), 2);

        // Get rate should now return updated value
        let result = cached_oracle.get_rate().await.unwrap();
        assert_eq!(result.rate, U256::from(300000000000u128));
        assert_eq!(oracle.get_call_count(), 2, "Should not fetch again");
    }

    #[tokio::test]
    async fn test_refresh_error_preserves_cache() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let cached_oracle = CachedPriceOracle::new(oracle.clone());

        // Pre-populate cache with valid rate
        cached_oracle.refresh_rate().await;
        let cached = cached_oracle.get_cached_rate().await.unwrap();
        assert_eq!(cached.rate, U256::from(200000000000u128));

        // Configure oracle to return errors
        oracle.set_should_error(true);

        // Attempt refresh (should fail but not clear cache)
        cached_oracle.refresh_rate().await;

        // Verify cache still has the old value
        let still_cached = cached_oracle.get_cached_rate().await.unwrap();
        assert_eq!(still_cached.rate, U256::from(200000000000u128));
    }

    #[tokio::test]
    async fn test_get_rate_propagates_error_on_cache_miss() {
        let oracle = Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        oracle.set_should_error(true);
        let cached_oracle = CachedPriceOracle::new(oracle);

        // With empty cache and oracle error, get_rate should fail
        let result = cached_oracle.get_rate().await;
        assert!(result.is_err());
    }
}
