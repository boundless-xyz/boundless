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

use crate::price_oracle::{
    AggregationMode, ExchangeRate, PriceOracle, PriceOracleError, TradingPair,
};
use alloy_primitives::U256;
use futures::future::join_all;
use std::sync::Arc;

/// Composite oracle that aggregates multiple price sources for a single trading pair
pub struct CompositeOracle {
    sources: Vec<Arc<dyn PriceOracle>>,
    aggregation_mode: AggregationMode,
    min_sources: u8,
    pair: TradingPair,
}

impl CompositeOracle {
    /// Create a new composite oracle for a specific trading pair
    pub fn new(
        pair: TradingPair,
        sources: Vec<Arc<dyn PriceOracle>>,
        aggregation_mode: AggregationMode,
        min_sources: u8,
    ) -> Self {
        Self { sources, aggregation_mode, min_sources, pair }
    }

    /// Fetch sources sequentially for priority mode, returning the first successful result
    async fn fetch_priority_sequential(&self) -> Result<ExchangeRate, PriceOracleError> {
        let mut errors = Vec::new();

        for source in &self.sources {
            match source.get_rate().await {
                Ok(rate) => {
                    tracing::debug!(
                        "Priority mode: using {} for {} with rate {} => {}. Timestamp: {} => {}",
                        source.name(),
                        self.pair,
                        rate.rate,
                        rate.rate_to_f64(),
                        rate.timestamp,
                        rate.timestamp_to_human_readable()
                    );
                    return Ok(rate);
                }
                Err(e) => {
                    let error_msg = format!("{}: {}", source.name(), e);
                    tracing::warn!(
                        "Priority mode: {} failed for {}: {}",
                        source.name(),
                        self.pair,
                        e
                    );
                    errors.push(error_msg);
                }
            }
        }

        // All sources failed
        Err(PriceOracleError::AllSourcesFailed { pair: self.pair, errors })
    }

    /// Fetch all sources in parallel for median/average mode
    async fn fetch_all_parallel(&self) -> Vec<(usize, Result<ExchangeRate, PriceOracleError>)> {
        let futures = self.sources.iter().enumerate().map(|(i, source)| {
            let source = Arc::clone(source);
            async move {
                let result = source.get_rate().await;
                (i, result)
            }
        });
        join_all(futures).await
    }

    fn aggregate_median_or_average(
        &self,
        results: Vec<(usize, Result<ExchangeRate, PriceOracleError>)>,
    ) -> Result<ExchangeRate, PriceOracleError> {
        // Collect successful results
        let mut successful: Vec<ExchangeRate> = Vec::new();
        for (i, result) in results.into_iter() {
            match result {
                Ok(rate) => {
                    tracing::debug!(
                        "Source {} returned rate for {}: {} => {}. Timestamp: {} => {}",
                        self.sources[i].name(),
                        self.pair,
                        rate.rate,
                        rate.rate_to_f64(),
                        rate.timestamp,
                        rate.timestamp_to_human_readable()
                    );
                    successful.push(rate);
                }
                Err(e) => {
                    tracing::warn!(
                        "Source {} failed for {}: {}",
                        self.sources[i].name(),
                        self.pair,
                        e
                    );
                }
            }
        }

        // Check if we have enough successful sources
        if successful.len() < self.min_sources as usize {
            return Err(PriceOracleError::InsufficientSources {
                pair: self.pair,
                got: successful.len() as u8,
                need: self.min_sources,
            });
        }

        // Aggregate based on mode
        let aggregated_rate = match self.aggregation_mode {
            AggregationMode::Median => {
                // Sort by rate and take median
                successful.sort_by(|a, b| a.rate.cmp(&b.rate));
                let mid = successful.len() / 2;
                successful[mid]
            }
            AggregationMode::Average => {
                // Calculate average
                let sum: u128 = successful.iter().map(|r| r.rate.to::<u128>()).sum();
                let avg = sum / successful.len() as u128;
                let timestamp = successful[0].timestamp; // Use first timestamp
                ExchangeRate::new(self.pair, U256::from(avg), timestamp)
            }
            AggregationMode::Priority => unreachable!("Priority mode handled separately"),
        };

        tracing::debug!(
            "Aggregated rate for {} using {:?}: {} => {}. Timestamp: {} => {}",
            self.pair,
            self.aggregation_mode,
            aggregated_rate.rate,
            aggregated_rate.rate_to_f64(),
            aggregated_rate.timestamp,
            aggregated_rate.timestamp_to_human_readable()
        );
        Ok(aggregated_rate)
    }
}

#[async_trait::async_trait]
impl PriceOracle for CompositeOracle {
    fn pair(&self) -> TradingPair {
        self.pair
    }

    async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
        // Use different fetching strategies based on aggregation mode
        match self.aggregation_mode {
            AggregationMode::Priority => self.fetch_priority_sequential().await,
            AggregationMode::Median | AggregationMode::Average => {
                let results = self.fetch_all_parallel().await;
                self.aggregate_median_or_average(results)
            }
        }
    }
    fn name(&self) -> String {
        format!(
            "CompositeOracle: {}",
            self.sources.iter().map(|s| s.name()).collect::<Vec<_>>().join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    // TODO: increase test coverage
    use super::*;
    use alloy::primitives::U256;

    struct MockSource {
        name: &'static str,
        pair: TradingPair,
        result: Result<ExchangeRate, &'static str>,
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockSource {
        fn pair(&self) -> TradingPair {
            self.pair
        }

        async fn get_rate(&self) -> Result<ExchangeRate, PriceOracleError> {
            self.result.map_err(|s| PriceOracleError::InvalidPrice(s.to_string()))
        }

        fn name(&self) -> String {
            self.name.to_string()
        }
    }

    #[tokio::test]
    async fn test_priority_mode() -> anyhow::Result<()> {
        let expected = ExchangeRate::new(TradingPair::EthUsd, U256::from(200000000000u128), 1000);

        let sources: Vec<Arc<dyn PriceOracle>> = vec![
            Arc::new(MockSource {
                name: "Failed",
                pair: TradingPair::EthUsd,
                result: Err("test error"),
            }),
            Arc::new(MockSource {
                name: "Success",
                pair: TradingPair::EthUsd,
                result: Ok(expected),
            }),
        ];

        let oracle =
            CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Priority, 1);

        let result = oracle.get_rate().await?;
        assert_eq!(result, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_median_mode() -> anyhow::Result<()> {
        let sources: Vec<Arc<dyn PriceOracle>> = vec![
            Arc::new(MockSource {
                name: "Source1",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(100000000000u128),
                    999,
                )),
            }),
            Arc::new(MockSource {
                name: "Source2",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(210000000000u128),
                    1000,
                )),
            }),
            Arc::new(MockSource {
                name: "Source3",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(300000000000u128),
                    1001,
                )),
            }),
        ];

        let oracle = CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Median, 1);

        let result = oracle.get_rate().await?;

        let expected = ExchangeRate::new(TradingPair::EthUsd, U256::from(210000000000u128), 1000);
        assert_eq!(result, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_average_mode() -> anyhow::Result<()> {
        let sources: Vec<Arc<dyn PriceOracle>> = vec![
            Arc::new(MockSource {
                name: "Source1",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(100000000000u128),
                    999,
                )),
            }),
            Arc::new(MockSource {
                name: "Source2",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(200000000000u128),
                    1000,
                )),
            }),
            Arc::new(MockSource {
                name: "Source3",
                pair: TradingPair::EthUsd,
                result: Ok(ExchangeRate::new(
                    TradingPair::EthUsd,
                    U256::from(300000000000u128),
                    1001,
                )),
            }),
        ];

        let oracle =
            CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Average, 1);

        let result = oracle.get_rate().await?;

        let expected = ExchangeRate::new(TradingPair::EthUsd, U256::from(200000000000u128), 999);
        assert_eq!(result, expected);

        Ok(())
    }
}
