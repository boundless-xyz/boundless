use crate::price_oracle::{
    AggregationMode, PriceOracle, PriceOracleError, PriceQuote, PriceSource, TradingPair,
};
use alloy_primitives::U256;
use futures::future::join_all;
use std::sync::Arc;

/// Composite oracle that aggregates multiple price sources for a single trading pair
pub struct CompositeOracle {
    sources: Vec<Arc<dyn PriceSource>>,
    aggregation_mode: AggregationMode,
    min_sources: u8,
    pair: TradingPair,
}

impl CompositeOracle {
    /// Create a new composite oracle for a specific trading pair
    pub fn new(
        pair: TradingPair,
        sources: Vec<Arc<dyn PriceSource>>,
        aggregation_mode: AggregationMode,
        min_sources: u8,
    ) -> Self {
        Self { sources, aggregation_mode, min_sources, pair }
    }

    /// Fetch sources sequentially for priority mode, returning the first successful result
    async fn fetch_priority_sequential(&self) -> Result<PriceQuote, PriceOracleError> {
        let mut errors = Vec::new();

        for source in &self.sources {
            match source.get_price().await {
                Ok(quote) => {
                    tracing::debug!(
                        "Priority mode: using {} for {} with price {} => {}. Timestamp: {} => {}",
                        source.name(),
                        self.pair.as_str(),
                        quote.price,
                        quote.price_to_f64(),
                        quote.timestamp,
                        quote.timestamp_to_human_readable()
                    );
                    return Ok(quote);
                }
                Err(e) => {
                    let error_msg = format!("{}: {}", source.name(), e);
                    tracing::warn!(
                        "Priority mode: {} failed for {}: {}",
                        source.name(),
                        self.pair.as_str(),
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
    async fn fetch_all_parallel(&self) -> Vec<(usize, Result<PriceQuote, PriceOracleError>)> {
        let futures = self.sources.iter().enumerate().map(|(i, source)| {
            let source = Arc::clone(source);
            async move {
                let result = source.get_price().await;
                (i, result)
            }
        });
        join_all(futures).await
    }

    fn aggregate_median_or_average(
        &self,
        results: Vec<(usize, Result<PriceQuote, PriceOracleError>)>,
    ) -> Result<PriceQuote, PriceOracleError> {
        // Collect successful results
        let mut successful: Vec<PriceQuote> = Vec::new();
        for (i, result) in results.into_iter() {
            match result {
                Ok(quote) => {
                    tracing::debug!(
                        "Source {} returned price for {}: {} => {}. Timestamp: {} => {}",
                        self.sources[i].name(),
                        self.pair.as_str(),
                        quote.price,
                        quote.price_to_f64(),
                        quote.timestamp,
                        quote.timestamp_to_human_readable()
                    );
                    successful.push(quote);
                }
                Err(e) => {
                    tracing::warn!(
                        "Source {} failed for {}: {}",
                        self.sources[i].name(),
                        self.pair.as_str(),
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
        let aggregated_price = match self.aggregation_mode {
            AggregationMode::Median => {
                // Sort by price and take median
                successful.sort_by(|a, b| a.price.cmp(&b.price));
                let mid = successful.len() / 2;
                successful[mid]
            }
            AggregationMode::Average => {
                // Calculate average
                let sum: u128 = successful.iter().map(|q| q.price.to::<u128>()).sum();
                let avg = sum / successful.len() as u128;
                let timestamp = successful[0].timestamp; // Use first timestamp
                PriceQuote::new(U256::from(avg), timestamp)
            }
            AggregationMode::Priority => unreachable!("Priority mode handled separately"),
        };

        tracing::debug!(
            "Aggregated price for {} using {:?}: {} => {}. Timestamp: {} => {}",
            self.pair.as_str(),
            self.aggregation_mode,
            aggregated_price.price,
            aggregated_price.price_to_f64(),
            aggregated_price.timestamp,
            aggregated_price.timestamp_to_human_readable()
        );
        Ok(aggregated_price)
    }
}

#[async_trait::async_trait]
impl PriceOracle for CompositeOracle {
    async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
        // Use different fetching strategies based on aggregation mode
        match self.aggregation_mode {
            AggregationMode::Priority => self.fetch_priority_sequential().await,
            AggregationMode::Median | AggregationMode::Average => {
                let results = self.fetch_all_parallel().await;
                self.aggregate_median_or_average(results)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // TODO: increase test coverage
    use super::*;
    use alloy::primitives::U256;

    struct MockSource {
        name: &'static str,
        result: Result<PriceQuote, &'static str>,
    }

    #[async_trait::async_trait]
    impl PriceOracle for MockSource {
        async fn get_price(&self) -> Result<PriceQuote, PriceOracleError> {
            self.result.clone().map_err(|s| PriceOracleError::InvalidPrice(s.to_string()))
        }
    }

    impl PriceSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }
    }

    #[tokio::test]
    async fn test_priority_mode() -> anyhow::Result<()> {
        let expected = PriceQuote::new(U256::from(200000000000u128), 1000);

        let sources: Vec<Arc<dyn PriceSource>> = vec![
            Arc::new(MockSource { name: "Failed", result: Err("test error") }),
            Arc::new(MockSource { name: "Success", result: Ok(expected.clone()) }),
        ];

        let oracle =
            CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Priority, 1);

        let result = oracle.get_price().await?;
        assert_eq!(result, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_median_mode() -> anyhow::Result<()> {
        let sources: Vec<Arc<dyn PriceSource>> = vec![
            Arc::new(MockSource {
                name: "Source1",
                result: Ok(PriceQuote::new(U256::from(100000000000u128), 999)),
            }),
            Arc::new(MockSource {
                name: "Source2",
                result: Ok(PriceQuote::new(U256::from(210000000000u128), 1000)),
            }),
            Arc::new(MockSource {
                name: "Source3",
                result: Ok(PriceQuote::new(U256::from(300000000000u128), 1001)),
            }),
        ];

        let oracle = CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Median, 1);

        let result = oracle.get_price().await?;

        let expected = PriceQuote::new(U256::from(210000000000u128), 1000);
        assert_eq!(result, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_average_mode() -> anyhow::Result<()> {
        let sources: Vec<Arc<dyn PriceSource>> = vec![
            Arc::new(MockSource {
                name: "Source1",
                result: Ok(PriceQuote::new(U256::from(100000000000u128), 999)),
            }),
            Arc::new(MockSource {
                name: "Source2",
                result: Ok(PriceQuote::new(U256::from(200000000000u128), 1000)),
            }),
            Arc::new(MockSource {
                name: "Source3",
                result: Ok(PriceQuote::new(U256::from(300000000000u128), 1001)),
            }),
        ];

        let oracle =
            CompositeOracle::new(TradingPair::EthUsd, sources, AggregationMode::Average, 1);

        let result = oracle.get_price().await?;

        let expected = PriceQuote::new(U256::from(200000000000u128), 999);
        assert_eq!(result, expected);

        Ok(())
    }
}
