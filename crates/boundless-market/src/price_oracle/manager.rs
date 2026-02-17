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

use super::asset::{convert_asset_value, Amount, Asset, ConversionError};
use super::{CachedPriceOracle, ExchangeRate, PriceOracle, PriceOracleError, TradingPair};
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
    /// Maximum time without a successful price update before returning an UpdateTimeout error
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

    /// Get exchange rate for a specific trading pair
    pub async fn get_rate(&self, pair: TradingPair) -> Result<ExchangeRate, PriceOracleError> {
        match pair {
            TradingPair::EthUsd => self.eth_usd.get_rate().await,
            TradingPair::ZkcUsd => self.zkc_usd.get_rate().await,
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
                self.get_rate(TradingPair::EthUsd).await?.rate
            }
            (Asset::USD, Asset::ZKC) | (Asset::ZKC, Asset::USD) => {
                self.get_rate(TradingPair::ZkcUsd).await?.rate
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

    /// Refresh all rates from their oracles.
    pub async fn refresh_all_rates(&self) {
        tokio::join!(self.eth_usd.refresh_rate(), self.zkc_usd.refresh_rate(),);
    }

    /// Spawn background refresh task for all oracles
    ///
    /// Returns a join handle that completes when the task is cancelled.
    pub async fn start_oracle(
        self,
        cancel_token: CancellationToken,
    ) -> Result<(), PriceOracleError> {
        tracing::info!("Price oracle refresh task started (interval: {}s)", self.refresh_interval);

        // with ::interval the first tick completes immediately so we refresh right away.
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

                // Periodic tick to refresh rates and check staleness
                _ = ticker.tick() => {
                    tokio::join!(
                        self.eth_usd.refresh_rate(),
                        self.zkc_usd.refresh_rate(),
                    );

                    // After refresh, check if we've gone too long without a successful update (if enabled)
                    if self.max_time_without_update > 0 {
                        let eth_rate = self.eth_usd.get_cached_rate().await;
                        let zkc_rate = self.zkc_usd.get_cached_rate().await;

                        if eth_rate.is_none_or(|r| r.is_stale(self.max_time_without_update))
                        || zkc_rate.is_none_or(|r| r.is_stale(self.max_time_without_update)) {
                            tracing::error!(
                                "Rates haven't been updated for too long (ETH: {}, ZKC: {}), returning UpdateTimeout error",
                                eth_rate.map_or("stale".to_string(), |r| format!("{} @ {}", r.rate, r.timestamp_to_human_readable())),
                                zkc_rate.map_or("stale".to_string(), |r| format!("{} @ {}", r.rate, r.timestamp_to_human_readable())),
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
        /// Trading pair
        pair: TradingPair,
        /// Rate to return
        rate: Mutex<U256>,
        /// Call count
        call_count: Arc<AtomicU64>,
        /// Delay to simulate slow fetches
        delay: Duration,
        /// Whether to return errors
        should_error: AtomicBool,
    }

    impl MockOracle {
        fn new(pair: TradingPair, rate: U256) -> Self {
            Self {
                pair,
                rate: Mutex::new(rate),
                call_count: Arc::new(AtomicU64::new(0)),
                delay: Duration::from_millis(0),
                should_error: AtomicBool::new(false),
            }
        }

        async fn set_rate(&self, rate: U256) {
            *self.rate.lock().await = rate;
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

            // Return rate
            let rate = *self.rate.lock().await;

            Ok(ExchangeRate::new(self.pair, rate, 1000))
        }

        fn name(&self) -> String {
            format!("MockOracle({:?})", self.pair)
        }
    }

    #[tokio::test(start_paused = true)]
    async fn test_spawn_refresh_task_periodic_refresh() -> anyhow::Result<()> {
        let eth_oracle =
            Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(TradingPair::ZkcUsd, U256::from(100000000u128)));

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

        // Change rates and advance again
        eth_oracle.set_rate(U256::from(300000000000u128)).await;
        zkc_oracle.set_rate(U256::from(150000000u128)).await;

        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::task::yield_now().await;

        assert_eq!(eth_oracle.get_call_count(), 3);
        assert_eq!(zkc_oracle.get_call_count(), 3);

        // Verify updated rates
        let eth_rate = eth_cached.get_cached_rate().await.unwrap();
        assert_eq!(eth_rate.rate, U256::from(300000000000u128));

        let zkc_rate = zkc_cached.get_cached_rate().await.unwrap();
        assert_eq!(zkc_rate.rate, U256::from(150000000u128));

        cancel_token.cancel();
        handle.await??;

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn test_refresh_task_returns_error_on_stale_cache() -> anyhow::Result<()> {
        let eth_oracle =
            Arc::new(MockOracle::new(TradingPair::EthUsd, U256::from(200000000000u128)));
        let zkc_oracle = Arc::new(MockOracle::new(TradingPair::ZkcUsd, U256::from(100000000u128)));

        let eth_cached = Arc::new(CachedPriceOracle::new(eth_oracle.clone()));
        let zkc_cached = Arc::new(CachedPriceOracle::new(zkc_oracle.clone()));

        // First populate the cache
        eth_cached.refresh_rate().await;
        zkc_cached.refresh_rate().await;

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
        assert!(result.is_err(), "Task should have exited due to stale rates");

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_usd_to_eth() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ETH price = $2000 (200000000000 with 8 decimals)
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 2000 USD to ETH (should be 1 ETH)
        let usd_amount = Amount::parse("2000 USD", None)?;
        let eth_amount = manager.convert(&usd_amount, Asset::ETH).await?;

        assert_eq!(eth_amount.asset, Asset::ETH);
        assert_eq!(eth_amount.value, U256::from(1_000_000_000_000_000_000u128)); // 1 ETH

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_eth_to_usd() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ETH price = $2000
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 1 ETH to USD (should be $2000)
        let eth_amount = Amount::parse("1 ETH", None)?;
        let usd_amount = manager.convert(&eth_amount, Asset::USD).await?;

        assert_eq!(usd_amount.asset, Asset::USD);
        assert_eq!(usd_amount.value, U256::from(2_000_000_000u128)); // $2000 with 6 decimals

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_usd_to_zkc() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ZKC price = $1.00
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 100 USD to ZKC (should be 100 ZKC)
        let usd_amount = Amount::parse("100 USD", None)?;
        let zkc_amount = manager.convert(&usd_amount, Asset::ZKC).await?;

        assert_eq!(zkc_amount.asset, Asset::ZKC);
        assert_eq!(zkc_amount.value, U256::from(100_000_000_000_000_000_000u128)); // 100 ZKC

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_zkc_to_usd() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        // ZKC price = $1.00
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 100 ZKC to USD (should be $100)
        let zkc_amount = Amount::parse("100 ZKC", None)?;
        let usd_amount = manager.convert(&zkc_amount, Asset::USD).await?;

        assert_eq!(usd_amount.asset, Asset::USD);
        assert_eq!(usd_amount.value, U256::from(100_000_000u128)); // $100 with 6 decimals

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_same_asset_returns_unchanged() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert ETH to ETH (should return same amount)
        let eth_amount = Amount::parse("1.5 ETH", None)?;
        let result = manager.convert(&eth_amount, Asset::ETH).await?;

        assert_eq!(result.asset, Asset::ETH);
        assert_eq!(result.value, eth_amount.value);

        Ok(())
    }

    #[tokio::test]
    async fn test_convert_eth_to_zkc_returns_error() -> anyhow::Result<()> {
        use crate::price_oracle::sources::StaticPriceSource;

        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert ETH to ZKC (should fail - unsupported)
        let eth_amount = Amount::parse("1 ETH", None)?;
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
        let eth_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::EthUsd,
            2000.0,
        ))));
        let zkc_oracle = Arc::new(CachedPriceOracle::new(Arc::new(StaticPriceSource::new(
            TradingPair::ZkcUsd,
            1.0,
        ))));
        let manager = PriceOracleManager::new(eth_oracle, zkc_oracle, 60, 0);

        // Convert 0.001 USD to ETH
        let usd_amount = Amount::parse("0.001 USD", None)?;
        let eth_amount = manager.convert(&usd_amount, Asset::ETH).await?;

        assert_eq!(eth_amount.asset, Asset::ETH);
        // 0.001 USD / 2000 = 0.0000005 ETH = 500000000000 wei
        assert_eq!(eth_amount.value, U256::from(500_000_000_000u128));

        Ok(())
    }
}
