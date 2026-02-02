use std::sync::Arc;
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;
use crate::price_oracle::{CachedPriceOracle, PriceOracle, PriceOracleError, PriceQuote, TradingPair};

/// Container for per-pair price oracles
#[derive(Clone)]
pub struct PriceOracleManager {
    /// ETH/USD price oracle
    pub eth_usd: Arc<CachedPriceOracle>,
    /// ZKC/USD price oracle
    pub zkc_usd: Arc<CachedPriceOracle>,
    /// Refresh interval for background price updates
    refresh_interval: u64,
    /// Maximum time without a successful price update before the refresh task exits
    max_time_without_update: u64,
}

impl PriceOracleManager {
    /// Create a new PriceOracleManager
    pub fn new(
        eth_usd: Arc<CachedPriceOracle>,
        zkc_usd: Arc<CachedPriceOracle>,
        refresh_interval_secs: u64,
        max_secs_without_price_update: u64,
    ) -> Self {
        Self {
            eth_usd,
            zkc_usd,
            refresh_interval: refresh_interval_secs,
            max_time_without_update: max_secs_without_price_update,
        }
    }

    /// Get price for a specific trading pair
    pub async fn get_price(&self, pair: TradingPair) -> Result<PriceQuote, PriceOracleError> {
        match pair {
            TradingPair::EthUsd => self.eth_usd.get_price().await,
            TradingPair::ZkcUsd => self.zkc_usd.get_price().await,
        }
    }

    /// Spawn background refresh task for all oracles
    ///
    /// Returns a join handle that completes when the task is cancelled.
    pub async fn start_oracle(
        self,
        cancel_token: CancellationToken,
    ) -> Result<(), PriceOracleError> {
        tracing::info!("Price oracle refresh task started (interval: {}s)", self.refresh_interval);

        let mut ticker = tokio::time::interval(Duration::from_secs(self.refresh_interval));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                // Make sure to handle cancellation promptly
                biased;

                _ = cancel_token.cancelled() => {
                    tracing::info!("Price oracle refresh task shutting down");
                    break;
                }

                // Periodic tick to refresh prices and check staleness
                _ = ticker.tick() => {
                    tokio::join!(
                        self.eth_usd.refresh_price(),
                        self.zkc_usd.refresh_price(),
                    );

                    // After refresh, check if we've gone too long without a successful update (if enabled)
                    if self.max_time_without_update > 0 {
                        let eth_quote = self.eth_usd.get_cached_price().await;
                        let zkc_quote = self.zkc_usd.get_cached_price().await;

                        // TODO: we need to use this to trigger a shutdown of the entire broker
                        if eth_quote.is_none_or(|q| q.is_stale(self.max_time_without_update))
                        || zkc_quote.is_none_or(|q| q.is_stale(self.max_time_without_update)) {
                            tracing::error!(
                                "Prices haven't been updated for too long (ETH: {}, ZKC: {}), exiting price oracle",
                                eth_quote.map_or("stale".to_string(), |q| format!("{} @ {}", q.price, q.timestamp_to_human_readable())),
                                zkc_quote.map_or("stale".to_string(), |q| format!("{} @ {}", q.price, q.timestamp_to_human_readable())),
                            );

                            return Err(PriceOracleError::UpdateTimeout());
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;
    use crate::price_oracle::PriceOracle;
    use tokio::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use crate::price_oracle::TradingPair::{EthUsd, ZkcUsd};

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
    async fn test_spawn_refresh_task_initial_refresh_and_cancellation() -> anyhow::Result<()> {
        let eth_oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        let manager = PriceOracleManager::new(eth_cached.clone(), zkc_cached.clone(), 60, 600);

        // Spawn the refresh task
        let cancel_token = CancellationToken::new();
        let handle = manager.start_oracle(cancel_token.clone()).await;

        // Verify prices are cached
        let eth_price = eth_cached.get_cached_price().await.expect("ETH price should be in cache after initial refresh");
        assert_eq!(eth_price.price, U256::from(200000000000u128));

        let zkc_price = zkc_cached.get_cached_price().await.expect("ZKC price should be in cache after initial refresh");
        assert_eq!(zkc_price.price, U256::from(100000000u128));

        // Cleanup
        cancel_token.cancel();

        // Verify the task completes successfully
        let result = tokio::time::timeout(Duration::from_secs(2),handle).await?;
        assert!(result.is_ok(), "Task should complete within timeout");

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_refresh_task_periodic_refresh() -> anyhow::Result<()> {
        let eth_oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        // Use 1 second refresh interval, disable staleness check (0) for this timing-sensitive test
        let manager = PriceOracleManager::new(eth_cached.clone(), zkc_cached.clone(), 1, 0);

        let cancel_token = CancellationToken::new();
        let handle = manager.start_oracle(cancel_token.clone()).await;

        // Wait for initial refresh (the task does an immediate refresh, then the first
        // tick() completes immediately too, so we get 2 refreshes right away)
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Verify initial refreshes happened (2 due to immediate tick behavior)
        let eth_initial_count = eth_oracle.get_call_count();
        let zkc_initial_count = zkc_oracle.get_call_count();
        assert_eq!(eth_initial_count, 2, "ETH: Initial refresh + first tick = 2");
        assert_eq!(zkc_initial_count, 2, "ZKC: Initial refresh + first tick = 2");

        // Verify caches were populated with initial prices
        let eth_price = manager.get_price(EthUsd).await?;
        assert_eq!(eth_price.price, U256::from(200000000000u128));
        let zkc_price = manager.get_price(ZkcUsd).await?;
        assert_eq!(zkc_price.price, U256::from(100000000u128));

        // Change the prices
        eth_oracle.set_price(U256::from(300000000000u128)).await;
        zkc_oracle.set_price(U256::from(150000000u128)).await;

        // Wait for the next refresh cycle (1 second interval)
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify a third refresh happened for both
        let eth_third_count = eth_oracle.get_call_count();
        let zkc_third_count = zkc_oracle.get_call_count();
        assert_eq!(eth_third_count, 3, "ETH: Should have exactly 3 refreshes");
        assert_eq!(zkc_third_count, 3, "ZKC: Should have exactly 3 refreshes");

        // Verify caches were updated with new prices
        let eth_price = manager.get_price(EthUsd).await?;
        assert_eq!(eth_price.price, U256::from(300000000000u128));
        let zkc_price = manager.get_price(ZkcUsd).await?;
        assert_eq!(zkc_price.price, U256::from(150000000u128));

        // Wait for another refresh cycle
        tokio::time::sleep(Duration::from_millis(1100)).await;

        // Verify fourth refresh happened for both
        let eth_fourth_count = eth_oracle.get_call_count();
        let zkc_fourth_count = zkc_oracle.get_call_count();
        assert_eq!(eth_fourth_count, 4, "ETH: Should have exactly 4 refreshes");
        assert_eq!(zkc_fourth_count, 4, "ZKC: Should have exactly 4 refreshes");

        // Cleanup
        cancel_token.cancel();
        handle.await?;

        Ok(())
    }

    #[tokio::test]
    async fn test_spawn_refresh_task_exits_on_stale_cache() -> anyhow::Result<()> {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init()
            .ok();

        // Create mock oracles that will fail to update (simulate all sources down)
        let eth_oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        // First populate the cache with an old price by temporarily using non-erroring oracles
        eth_oracle.should_error.store(false, Ordering::SeqCst);
        zkc_oracle.should_error.store(false, Ordering::SeqCst);
        let _ = eth_cached.refresh_price().await;
        let _ = zkc_cached.refresh_price().await;

        // Now make them error
        eth_oracle.should_error.store(true, Ordering::SeqCst);
        zkc_oracle.should_error.store(true, Ordering::SeqCst);

        // Create manager with short cache age limit and refresh interval
        // max_cache_age_secs = 2 seconds, refresh_interval = 1 second
        let manager = PriceOracleManager::new(eth_cached.clone(), zkc_cached.clone(), 1, 2);

        let cancel_token = CancellationToken::new();
        let handle = manager.start_oracle(cancel_token.clone()).await;

        // Wait longer than the cache age limit
        // After initial refresh + 3 more ticks = 4 seconds total, which exceeds the 2 second limit
        tokio::time::sleep(Duration::from_secs(4)).await;

        // Check that the task has completed (not just cancelled)
        let result = tokio::time::timeout(Duration::from_millis(100), handle).await;
        assert!(result.is_ok(), "Task should have exited due prices not being refreshed for too long");

        Ok(())
    }
}
