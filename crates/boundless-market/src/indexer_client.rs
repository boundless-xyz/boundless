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

//! Client for interacting with the Boundless Indexer API to fetch market price aggregates.

use alloy_primitives::U256;
use anyhow::{bail, Context, Result};
use reqwest::Client;
use std::str::FromStr;
use url::Url;

use crate::{
    deployments::{BASE_MAINNET_INDEXER_URL, BASE_SEPOLIA_INDEXER_URL, SEPOLIA_INDEXER_URL},
    price_provider::PricePercentiles,
};

pub use crate::indexer_types::{
    AggregationGranularity, MarketAggregateEntry, MarketAggregatesParams, MarketAggregatesResponse,
};

#[derive(Clone, Debug)]
/// Client for the Boundless Indexer API
pub struct IndexerClient {
    client: Client,
    base_url: Url,
}

impl IndexerClient {
    /// Create a new IndexerClient with hardcoded URL based on chain ID
    /// Can be overridden with INDEXER_API_URL environment variable
    pub fn new_from_chain_id(chain_id: u64) -> Result<Self> {
        // Check for environment variable override first
        let base_url = if let Ok(url_str) = std::env::var("INDEXER_API_URL") {
            tracing::debug!("Using INDEXER_API_URL from environment: {}", url_str);
            Url::parse(&url_str).context("Invalid INDEXER_API_URL")?
        } else {
            // Use hardcoded URLs based on chain ID
            let url_str = match chain_id {
                11155111 => SEPOLIA_INDEXER_URL,   // Sepolia testnet
                8453 => BASE_MAINNET_INDEXER_URL,  // Base mainnet
                84532 => BASE_SEPOLIA_INDEXER_URL, // Base sepolia
                _ => {
                    tracing::warn!(
                        "Unknown chain ID {}, defaulting to base mainnet indexer",
                        chain_id
                    );
                    BASE_MAINNET_INDEXER_URL
                }
            };
            tracing::debug!("Using indexer API for chain {}: {}", chain_id, url_str);
            Url::parse(url_str).expect("Hardcoded URL should be valid")
        };

        Self::new(base_url)
    }

    /// Create a new IndexerClient with explicit URL
    pub fn new(base_url: Url) -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("boundless-cli/1.0")
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client, base_url })
    }

    /// Get market aggregates
    /// GET /v1/market/aggregates
    pub async fn get_market_aggregates(
        &self,
        params: MarketAggregatesParams,
    ) -> Result<MarketAggregatesResponse> {
        let mut url = self.base_url.join("v1/market/aggregates").context("Failed to build URL")?;

        // Build query parameters manually to avoid non-Send closures from query_pairs_mut()
        // Collect all query parameters as owned strings first
        let mut query_parts = Vec::new();
        query_parts.push(format!("aggregation={}", params.aggregation));
        if let Some(cursor) = params.cursor {
            // URL encode cursor manually
            let encoded_cursor: String =
                url::form_urlencoded::byte_serialize(cursor.as_bytes()).collect();
            query_parts.push(format!("cursor={}", encoded_cursor));
        }
        if let Some(limit) = params.limit {
            query_parts.push(format!("limit={}", limit));
        }
        if let Some(sort) = params.sort {
            // URL encode sort manually
            let encoded_sort: String =
                url::form_urlencoded::byte_serialize(sort.as_bytes()).collect();
            query_parts.push(format!("sort={}", encoded_sort));
        }
        if let Some(before) = params.before {
            query_parts.push(format!("before={}", before));
        }
        if let Some(after) = params.after {
            query_parts.push(format!("after={}", after));
        }
        if !query_parts.is_empty() {
            url.set_query(Some(&query_parts.join("&")));
        }

        // Convert URL to string before capturing in closures to avoid Send issues
        let url_str = url.to_string();
        let response = self.client.get(url).send().await.with_context(|| {
            format!("Failed to fetch market aggregates from {}", url_str.clone())
        })?;

        if !response.status().is_success() {
            bail!("API error from {}: {}", url_str.clone(), response.status());
        }

        response
            .json()
            .await
            .with_context(|| format!("Failed to parse market aggregates response from {}", url_str))
    }

    /// Get p10 and p99 lock prices per cycle
    /// Returns the most recent weekly aggregate data from the last 7 days
    pub async fn get_prices_percentiles(
        &self,
        aggregation: AggregationGranularity,
    ) -> Result<PricePercentiles> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("Failed to get current time")?
            .as_secs() as i64;
        let one_week_ago = now - 604800; // 7 days in seconds

        let params = MarketAggregatesParams {
            aggregation,
            cursor: None,
            limit: Some(1),
            sort: Some("desc".to_string()), // Most recent first
            before: Some(now),
            after: Some(one_week_ago),
        };

        let response = self.get_market_aggregates(params).await?;

        // Find the most recent entry with valid prices
        for entry in response.data {
            if !entry.p10_lock_price_per_cycle.is_empty()
                && !entry.p99_lock_price_per_cycle.is_empty()
                && !entry.p25_lock_price_per_cycle.is_empty()
                && !entry.p50_lock_price_per_cycle.is_empty()
                && !entry.p75_lock_price_per_cycle.is_empty()
                && !entry.p90_lock_price_per_cycle.is_empty()
                && !entry.p95_lock_price_per_cycle.is_empty()
                && !entry.p99_lock_price_per_cycle.is_empty()
            {
                return Ok(PricePercentiles {
                    p10: U256::from_str(&entry.p10_lock_price_per_cycle)
                        .context("Failed to parse p10 lock price per cycle")?,
                    p25: U256::from_str(&entry.p25_lock_price_per_cycle)
                        .context("Failed to parse p25 lock price per cycle")?,
                    p50: U256::from_str(&entry.p50_lock_price_per_cycle)
                        .context("Failed to parse p50 lock price per cycle")?,
                    p75: U256::from_str(&entry.p75_lock_price_per_cycle)
                        .context("Failed to parse p75 lock price per cycle")?,
                    p90: U256::from_str(&entry.p90_lock_price_per_cycle)
                        .context("Failed to parse p90 lock price per cycle")?,
                    p95: U256::from_str(&entry.p95_lock_price_per_cycle)
                        .context("Failed to parse p95 lock price per cycle")?,
                    p99: U256::from_str(&entry.p99_lock_price_per_cycle)
                        .context("Failed to parse p99 lock price per cycle")?,
                });
            }
        }

        anyhow::bail!("No price data found")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        price_provider::PricePercentiles,
        test_helpers::{create_mock_indexer_client, create_test_indexer_client},
    };
    use alloy_primitives::U256;
    use httpmock::prelude::*;
    use serde_json::json;

    #[tokio::test]
    async fn test_get_market_aggregates() {
        let (server, client): (httpmock::MockServer, IndexerClient) = create_test_indexer_client();
        let response_json = r#"{
            "chain_id": 1,
            "aggregation": "daily",
            "data": [{
                "chain_id": 1,
                "timestamp": 1234567890,
                "timestamp_iso": "2009-02-13T23:31:30Z",
                "total_fulfilled": 100,
                "unique_provers_locking_requests": 5,
                "unique_requesters_submitting_requests": 10,
                "total_fees_locked": "1000000000000000000",
                "total_fees_locked_formatted": "1.0 ETH",
                "total_collateral_locked": "2000000000000000000",
                "total_collateral_locked_formatted": "2.0 ZKC",
                "total_locked_and_expired_collateral": "0",
                "total_locked_and_expired_collateral_formatted": "0.0 ZKC",
                "p10_lock_price_per_cycle": "1000000000000000",
                "p10_lock_price_per_cycle_formatted": "0.001 ETH",
                "p25_lock_price_per_cycle": "2000000000000000",
                "p25_lock_price_per_cycle_formatted": "0.002 ETH",
                "p50_lock_price_per_cycle": "3000000000000000",
                "p50_lock_price_per_cycle_formatted": "0.003 ETH",
                "p75_lock_price_per_cycle": "4000000000000000",
                "p75_lock_price_per_cycle_formatted": "0.004 ETH",
                "p90_lock_price_per_cycle": "5000000000000000",
                "p90_lock_price_per_cycle_formatted": "0.005 ETH",
                "p95_lock_price_per_cycle": "6000000000000000",
                "p95_lock_price_per_cycle_formatted": "0.006 ETH",
                "p99_lock_price_per_cycle": "7000000000000000",
                "p99_lock_price_per_cycle_formatted": "0.007 ETH",
                "total_requests_submitted": 150,
                "total_requests_submitted_onchain": 100,
                "total_requests_submitted_offchain": 50,
                "total_requests_locked": 80,
                "total_requests_slashed": 2,
                "total_expired": 10,
                "total_locked_and_expired": 5,
                "total_locked_and_fulfilled": 75,
                "total_secondary_fulfillments": 3,
                "locked_orders_fulfillment_rate": 93.75,
                "total_program_cycles": "1000000",
                "total_cycles": "1200000"
            }],
            "next_cursor": null,
            "has_more": false
        }"#;
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/market/aggregates")
                .query_param("aggregation", "daily")
                .query_param("limit", "10");
            then.status(200).body(response_json);
        });
        let params = MarketAggregatesParams {
            aggregation: AggregationGranularity::Daily,
            cursor: None,
            limit: Some(10),
            sort: None,
            before: None,
            after: None,
        };
        let response = client.get_market_aggregates(params).await.unwrap();

        assert_eq!(response.chain_id, 1);
        assert_eq!(response.aggregation, AggregationGranularity::Daily);
        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].total_fulfilled, 100);
        assert!(!response.has_more);
        mock.assert();
    }

    #[tokio::test]
    async fn test_get_prices_percentiles() {
        let price_percentiles = PricePercentiles {
            p10: U256::from(2000000000000000u64), // p10: 0.002 ETH
            p25: U256::from(3000000000000000u64), // p25: 0.003 ETH
            p50: U256::from(4000000000000000u64), // p50: 0.004 ETH
            p75: U256::from(5000000000000000u64), // p75: 0.005 ETH
            p90: U256::from(6000000000000000u64), // p90: 0.006 ETH
            p95: U256::from(7000000000000000u64), // p95: 0.007 ETH
            p99: U256::from(8000000000000000u64), // p99: 0.008 ETH
        };
        let (_server, client) = create_mock_indexer_client(&price_percentiles);
        let result = client.get_prices_percentiles(AggregationGranularity::Weekly).await.unwrap();

        assert_eq!(result.p10, price_percentiles.p10);
        assert_eq!(result.p25, price_percentiles.p25);
        assert_eq!(result.p50, price_percentiles.p50);
        assert_eq!(result.p75, price_percentiles.p75);
        assert_eq!(result.p90, price_percentiles.p90);
        assert_eq!(result.p95, price_percentiles.p95);
        assert_eq!(result.p99, price_percentiles.p99);
    }

    #[tokio::test]
    async fn test_get_prices_percentiles_no_data() {
        let (server, client) = create_test_indexer_client();
        let mock = server.mock(|when, then| {
            when.method(GET)
                .path("/v1/market/aggregates")
                .query_param("aggregation", "weekly")
                .query_param("limit", "1")
                .query_param("sort", "desc");
            then.status(200).json_body(json!({
                "chain_id": 1,
                "aggregation": "weekly",
                "data": [],
                "next_cursor": null,
                "has_more": false
            }));
        });

        let result = client.get_prices_percentiles(AggregationGranularity::Weekly).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No price data found"));
        mock.assert();
    }
}
