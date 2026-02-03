use super::asset::{convert_asset_value, Amount, Asset, ConversionError};
use super::{CachedPriceOracle, PriceOracle, PriceOracleError, PriceQuote, TradingPair};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

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

    /// Convert an Amount to a target asset using current prices.
    ///
    /// Supported conversions: USD↔ETH, USD↔ZKC
    /// Returns error for unsupported pairs (e.g., ETH<->ZKC)
    pub async fn convert(&self, amount: &Amount, target: Asset) -> Result<Amount, ConversionError> {
        if amount.asset == target {
            return Ok(amount.clone());
        }

        let price = match (amount.asset, target) {
            (Asset::USD, Asset::ETH) | (Asset::ETH, Asset::USD) => {
                self.get_price(TradingPair::EthUsd).await?.price
            }
            (Asset::USD, Asset::ZKC) | (Asset::ZKC, Asset::USD) => {
                self.get_price(TradingPair::ZkcUsd).await?.price
            }
            _ => {
                return Err(ConversionError::UnsupportedConversion {
                    from: amount.asset,
                    to: target,
                })
            }
        };

        let converted_value = convert_asset_value(amount.value, amount.asset, target, price)?;
        Ok(Amount::new(converted_value, target))
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
    use crate::price_oracle::PriceOracle;
    use alloy::primitives::U256;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use tokio::sync::Mutex;

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

    #[tokio::test]
    async fn test_convert_usd_to_eth() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ETH price = $2000 (200000000000 with 8 decimals)
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 2000 USD to ETH (should be 1 ETH)
        let usd_amount = Amount::parse("2000 USD")?;
        let eth_amount = manager.convert(&usd_amount, Asset::ETH).await?;

        assert_eq!(eth_amount.asset, Asset::ETH);
        assert_eq!(eth_amount.value, U256::from(1_000_000_000_000_000_000u128)); // 1 ETH

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_eth_to_usd() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ETH price = $2000
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 1 ETH to USD (should be $2000)
        let eth_amount = Amount::parse("1 ETH")?;
        let usd_amount = manager.convert(&eth_amount, Asset::USD).await?;

        assert_eq!(usd_amount.asset, Asset::USD);
        assert_eq!(usd_amount.value, U256::from(2_000_000_000u128)); // $2000 with 6 decimals

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_usd_to_zkc() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ZKC price = $1.00
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 100 USD to ZKC (should be 100 ZKC)
        let usd_amount = Amount::parse("100 USD")?;
        let zkc_amount = manager.convert(&usd_amount, Asset::ZKC).await?;

        assert_eq!(zkc_amount.asset, Asset::ZKC);
        assert_eq!(zkc_amount.value, U256::from(100_000_000_000_000_000_000u128)); // 100 ZKC

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_zkc_to_usd() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ZKC price = $1.00
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 100 ZKC to USD (should be $100)
        let zkc_amount = Amount::parse("100 ZKC")?;
        let usd_amount = manager.convert(&zkc_amount, Asset::USD).await?;

        assert_eq!(usd_amount.asset, Asset::USD);
        assert_eq!(usd_amount.value, U256::from(100_000_000u128)); // $100 with 6 decimals

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_same_asset_returns_unchanged() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert ETH to ETH (should return same amount)
        let eth_amount = Amount::parse("1.5 ETH")?;
        let result = manager.convert(&eth_amount, Asset::ETH).await?;

        assert_eq!(result.asset, Asset::ETH);
        assert_eq!(result.value, eth_amount.value);

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_eth_to_zkc_returns_error() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert ETH to ZKC (should fail - unsupported)
        let eth_amount = Amount::parse("1 ETH")?;
        let result = manager.convert(&eth_amount, Asset::ZKC).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ConversionError::UnsupportedConversion { from, to } => {
                assert_eq!(from, Asset::ETH);
                assert_eq!(to, Asset::ZKC);
            }
            _ => panic!("Expected UnsupportedConversion error"),
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_small_amounts() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ETH price = $2000
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(2000.0))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(1.0))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 0.001 USD to ETH
        let usd_amount = Amount::parse("0.001 USD")?;
        let eth_amount = manager.convert(&usd_amount, Asset::ETH).await?;

        assert_eq!(eth_amount.asset, Asset::ETH);
        // 0.001 USD / 2000 = 0.0000005 ETH = 500000000000 wei
        assert_eq!(eth_amount.value, U256::from(500_000_000_000u128));

        Ok(())
    }
}
