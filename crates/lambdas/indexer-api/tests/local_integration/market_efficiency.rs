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

use indexer_api::models::{
    EfficiencyAggregatesResponse, EfficiencyRequestsResponse, EfficiencySummaryResponse,
};

use super::TestEnv;

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_summary() {
    let env = TestEnv::market().await;

    let response: EfficiencySummaryResponse = env.get("/v1/market/efficiency").await.unwrap();

    tracing::info!(target: "efficiency-summary", "response: {:?}", response);

    // Summary should return valid data (even if no efficiency data has been indexed)
    // Verify the counts are consistent
    assert_eq!(
        response.total_requests_analyzed,
        response.most_profitable_locked + response.not_most_profitable_locked,
        "total should equal sum of most + not most profitable"
    );

    // If we have efficiency rates, they should be valid
    if let Some(rate) = response.latest_hourly_efficiency_rate {
        assert!(rate >= 0.0 && rate <= 1.0, "hourly efficiency rate should be 0-1: {}", rate);
    }
    if let Some(rate) = response.latest_daily_efficiency_rate {
        assert!(rate >= 0.0 && rate <= 1.0, "daily efficiency rate should be 0-1: {}", rate);
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_aggregates_hourly() {
    let env = TestEnv::market().await;

    let response: EfficiencyAggregatesResponse =
        env.get("/v1/market/efficiency/aggregates?granularity=hourly&limit=10").await.unwrap();

    tracing::info!(target: "efficiency-aggregates-hourly", "response: {:?}", response);

    // Response should have valid structure
    for entry in &response.data {
        assert!(entry.period_timestamp > 0, "period_timestamp should be positive");
        assert!(!entry.period_timestamp_iso.is_empty(), "period_timestamp_iso should not be empty");
        assert!(
            entry.period_timestamp_iso.contains('T'),
            "period_timestamp_iso should be ISO 8601: {}",
            entry.period_timestamp_iso
        );
        assert!(
            entry.efficiency_rate >= 0.0 && entry.efficiency_rate <= 1.0,
            "efficiency_rate should be 0-1: {}",
            entry.efficiency_rate
        );
    }

    // Cursor should only be present if has_more is true
    if response.has_more {
        assert!(response.next_cursor.is_some(), "next_cursor should be present when has_more");
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_aggregates_daily() {
    let env = TestEnv::market().await;

    let response: EfficiencyAggregatesResponse =
        env.get("/v1/market/efficiency/aggregates?granularity=daily&limit=10").await.unwrap();

    tracing::info!(target: "efficiency-aggregates-daily", "response: {:?}", response);

    // Response should have valid structure
    for entry in &response.data {
        assert!(entry.period_timestamp > 0, "period_timestamp should be positive");
        assert!(
            entry.efficiency_rate >= 0.0 && entry.efficiency_rate <= 1.0,
            "efficiency_rate should be 0-1: {}",
            entry.efficiency_rate
        );
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_requests_list() {
    let env = TestEnv::market().await;

    let response: EfficiencyRequestsResponse =
        env.get("/v1/market/efficiency/requests?limit=10").await.unwrap();

    tracing::info!(target: "efficiency-requests", "response: {:?}", response);

    // Response should have valid structure
    for entry in &response.data {
        assert!(
            entry.request_digest.starts_with("0x"),
            "request_digest should start with 0x: {}",
            entry.request_digest
        );
        assert!(!entry.request_id.is_empty(), "request_id should not be empty");
        assert!(entry.locked_at > 0, "locked_at should be positive");
        assert!(!entry.locked_at_iso.is_empty(), "locked_at_iso should not be empty");
        assert!(
            entry.locked_at_iso.contains('T'),
            "locked_at_iso should be ISO 8601: {}",
            entry.locked_at_iso
        );
        assert!(!entry.lock_price.is_empty(), "lock_price should not be empty");
        assert!(!entry.program_cycles.is_empty(), "program_cycles should not be empty");
        assert!(!entry.lock_price_per_cycle.is_empty(), "lock_price_per_cycle should not be empty");
    }

    // Cursor should only be present if has_more is true
    if response.has_more {
        assert!(response.next_cursor.is_some(), "next_cursor should be present when has_more");
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_summary_gas_adjusted() {
    let env = TestEnv::market().await;

    let response: EfficiencySummaryResponse =
        env.get("/v1/market/efficiency?gas_adjusted=true").await.unwrap();

    assert_eq!(
        response.total_requests_analyzed,
        response.most_profitable_locked + response.not_most_profitable_locked,
        "total should equal sum of most + not most profitable"
    );
    if let Some(rate) = response.latest_hourly_efficiency_rate {
        assert!(rate >= 0.0 && rate <= 1.0, "hourly efficiency rate should be 0-1: {}", rate);
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_summary_gas_adjusted_with_exclusions() {
    let env = TestEnv::market().await;

    let response: EfficiencySummaryResponse =
        env.get("/v1/market/efficiency?gas_adjusted_with_exclusions=true").await.unwrap();

    assert_eq!(
        response.total_requests_analyzed,
        response.most_profitable_locked + response.not_most_profitable_locked,
        "total should equal sum of most + not most profitable"
    );
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_aggregates_gas_adjusted() {
    let env = TestEnv::market().await;

    let response: EfficiencyAggregatesResponse = env
        .get("/v1/market/efficiency/aggregates?granularity=hourly&limit=10&gas_adjusted=true")
        .await
        .unwrap();

    for entry in &response.data {
        assert!(entry.period_timestamp > 0);
        assert!(
            entry.efficiency_rate >= 0.0 && entry.efficiency_rate <= 1.0,
            "efficiency_rate should be 0-1: {}",
            entry.efficiency_rate
        );
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_aggregates_gas_adjusted_with_exclusions() {
    let env = TestEnv::market().await;

    let response: EfficiencyAggregatesResponse = env
        .get("/v1/market/efficiency/aggregates?granularity=hourly&limit=10&gas_adjusted_with_exclusions=true")
        .await
        .unwrap();

    for entry in &response.data {
        assert!(entry.period_timestamp > 0);
        assert!(
            entry.efficiency_rate >= 0.0 && entry.efficiency_rate <= 1.0,
            "efficiency_rate should be 0-1: {}",
            entry.efficiency_rate
        );
    }
}

#[tokio::test]
#[cfg_attr(not(feature = "test-rpc"), ignore = "Requires BASE_MAINNET_RPC_URL")]
async fn test_efficiency_requests_pagination() {
    let env = TestEnv::market().await;

    // First page
    let page1: EfficiencyRequestsResponse =
        env.get("/v1/market/efficiency/requests?limit=5").await.unwrap();

    tracing::info!(target: "efficiency-requests-pagination", "page1: {:?}", page1);

    // If there's a next page, fetch it
    if let Some(cursor) = &page1.next_cursor {
        let page2: EfficiencyRequestsResponse = env
            .get(&format!("/v1/market/efficiency/requests?limit=5&cursor={}", cursor))
            .await
            .unwrap();

        tracing::info!(target: "efficiency-requests-pagination", "page2: {:?}", page2);

        // Pages should have different data (if there are enough records)
        if !page1.data.is_empty() && !page2.data.is_empty() {
            assert_ne!(
                page1.data[0].request_id, page2.data[0].request_id,
                "Pagination should return different records"
            );
        }
    }
}
