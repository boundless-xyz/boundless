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

    #[tokio::test(start_paused = true)]
    async fn test_spawn_refresh_task_periodic_refresh() -> anyhow::Result<()> {
        let eth_oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        // 1 second refresh interval, disable staleness check
        let manager = PriceOracleManager::new(eth_cached.clone(), zkc_cached.clone(), 1, 0);

        let cancel_token = CancellationToken::new();
        let handle = tokio::spawn(manager.start_oracle(cancel_token.clone()));

        // Yield to let first tick complete
        tokio::task::yield_now().await;

        // Verify initial refresh happened
        assert_eq!(eth_oracle.get_call_count(), 1);
        assert_eq!(zkc_oracle.get_call_count(), 1);

        // Advance time by 1 second to trigger next refresh
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        assert_eq!(eth_oracle.get_call_count(), 2);
        assert_eq!(zkc_oracle.get_call_count(), 2);

        // Change prices and advance again
        eth_oracle.set_price(U256::from(300000000000u128)).await;
        zkc_oracle.set_price(U256::from(150000000u128)).await;

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        assert_eq!(eth_oracle.get_call_count(), 3);
        assert_eq!(zkc_oracle.get_call_count(), 3);

        // Verify updated prices
        let eth_price = eth_cached.get_cached_price().await.unwrap();
        assert_eq!(eth_price.price, U256::from(300000000000u128));

        let zkc_price = zkc_cached.get_cached_price().await.unwrap();
        assert_eq!(zkc_price.price, U256::from(150000000u128));

        cancel_token.cancel();
        handle.await??;

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn test_spawn_refresh_task_exits_on_stale_cache() -> anyhow::Result<()> {
        let eth_oracle = Arc::new(MockOracle::new(U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        // First populate the cache
        eth_cached.refresh_price().await;
        zkc_cached.refresh_price().await;

        // Now make oracles error
        eth_oracle.should_error.store(true, Ordering::SeqCst);
        zkc_oracle.should_error.store(true, Ordering::SeqCst);

        // refresh_interval=1s, max_time_without_update=2s
        let manager = PriceOracleManager::new(eth_cached.clone(), zkc_cached.clone(), 1, 2);

        let cancel_token = CancellationToken::new();
        let handle = tokio::spawn(manager.start_oracle(cancel_token.clone()));

        // Advance time past the max_time_without_update threshold
        // Need to advance enough for staleness check to trigger
        tokio::time::advance(Duration::from_secs(3)).await;
        tokio::task::yield_now().await;

        // The task should have exited with an error
        let result = handle.await?;
        assert!(result.is_err(), "Task should have exited due to stale prices");

        Ok(())
    }
}
