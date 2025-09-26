// Copyright 2025 RISC Zero, Inc.
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

//! Integration tests for the Indexer API
//!
//! Run with: INDEXER_API_URL="https://api.example.com" cargo test -p indexer-api --test api_integration -- --ignored --nocapture

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use std::env;

// Helper function to get the API URL from environment
fn get_api_url() -> String {
    env::var("INDEXER_API_URL").expect("INDEXER_API_URL environment variable must be set")
}

// Helper function to make GET requests and parse JSON
async fn get_json<T: for<'de> Deserialize<'de>>(
    path: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    let url = format!("{}{}", get_api_url(), path);
    let client = Client::new();
    let response = client.get(&url).send().await?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await?;
        return Err(format!("Request failed with status {}: {}", status, text).into());
    }

    Ok(response.json().await?)
}

// Response structures for testing
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PaginatedResponse<T> {
    entries: Vec<T>,
    pagination: PaginationMetadata,
    summary: Option<Value>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PaginationMetadata {
    count: usize,
    offset: u64,
    limit: u64,
}

#[derive(Debug, Deserialize)]
struct HealthResponse {
    status: String,
    service: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct StakingEntry {
    rank: u64,
    staker_address: String,
    total_staked: Option<String>,
    total_staked_formatted: Option<String>,
    staked_amount: Option<String>,
    staked_amount_formatted: Option<String>,
    is_withdrawing: bool,
    epochs_participated: Option<u64>,
    epoch: Option<u64>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct PovwEntry {
    rank: u64,
    work_log_id: String,
    total_work_submitted: Option<String>,
    total_work_submitted_formatted: Option<String>,
    total_actual_rewards: Option<String>,
    total_actual_rewards_formatted: Option<String>,
    work_submitted: Option<String>,
    work_submitted_formatted: Option<String>,
    actual_rewards: Option<String>,
    actual_rewards_formatted: Option<String>,
    epochs_participated: Option<u64>,
    epoch: Option<u64>,
    percentage: Option<f64>,
}

// Helper to validate formatted ZKC values
fn assert_formatted_zkc(raw: &str, formatted: &str) {
    assert!(formatted.ends_with(" ZKC"), "Formatted ZKC should end with ' ZKC': {}", formatted);
    assert!(
        formatted.contains(",") || formatted == "0 ZKC" || !raw.starts_with("1000"),
        "Large ZKC values should have commas: {}",
        formatted
    );
}

// Helper to validate formatted cycle values
fn assert_formatted_cycles(raw: &str, formatted: &str) {
    assert!(
        formatted.ends_with(" cycles"),
        "Formatted cycles should end with ' cycles': {}",
        formatted
    );
    // Cycles are raw counts, check commas for values >= 1000
    if let Ok(num) = raw.parse::<u64>() {
        if num >= 1000 {
            assert!(
                formatted.contains(","),
                "Cycle values >= 1000 should have commas: {} -> {}",
                raw,
                formatted
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_health_endpoint() {
    let response: HealthResponse =
        get_json("/health").await.expect("Failed to get health endpoint");

    assert_eq!(response.status, "healthy");
    assert_eq!(response.service, "indexer-api");
}

#[tokio::test]
#[ignore]
async fn test_aggregate_staking_endpoint() {
    let response: PaginatedResponse<StakingEntry> =
        get_json("/v1/staking?limit=10").await.expect("Failed to get aggregate staking");

    // Check pagination
    assert!(response.entries.len() <= 10, "Should respect limit");
    assert_eq!(response.pagination.limit, 10);

    // Check entries have required fields
    for entry in &response.entries {
        assert!(!entry.staker_address.is_empty());

        // Check formatted fields exist and are valid
        if let (Some(raw), Some(formatted)) = (&entry.total_staked, &entry.total_staked_formatted) {
            assert_formatted_zkc(raw, formatted);
        }
    }

    // Check summary if present
    if let Some(summary) = response.summary {
        // Summary should have expected fields
        assert!(summary.get("current_total_staked").is_some());
        assert!(summary.get("current_total_staked_formatted").is_some());
        assert!(summary.get("total_unique_stakers").is_some());

        // Validate formatted field
        if let (Some(raw), Some(formatted)) = (
            summary.get("current_total_staked").and_then(|v| v.as_str()),
            summary.get("current_total_staked_formatted").and_then(|v| v.as_str()),
        ) {
            assert_formatted_zkc(raw, formatted);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_epoch_staking_endpoint() {
    let response: PaginatedResponse<StakingEntry> =
        get_json("/v1/staking/epochs/5?limit=10").await.expect("Failed to get epoch staking");

    // Check entries for epoch-specific fields
    for entry in &response.entries {
        assert!(!entry.staker_address.is_empty());
        assert_eq!(entry.epoch, Some(5));

        // Check formatted fields
        if let (Some(raw), Some(formatted)) = (&entry.staked_amount, &entry.staked_amount_formatted)
        {
            assert_formatted_zkc(raw, formatted);
        }
    }

    // Check epoch summary if present
    if let Some(summary) = response.summary {
        assert_eq!(summary.get("epoch").and_then(|v| v.as_u64()), Some(5));
        assert!(summary.get("total_staked").is_some());
        assert!(summary.get("total_staked_formatted").is_some());

        if let (Some(raw), Some(formatted)) = (
            summary.get("total_staked").and_then(|v| v.as_str()),
            summary.get("total_staked_formatted").and_then(|v| v.as_str()),
        ) {
            assert_formatted_zkc(raw, formatted);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_aggregate_povw_endpoint() {
    let response: PaginatedResponse<PovwEntry> =
        get_json("/v1/povw?limit=10").await.expect("Failed to get aggregate PoVW");

    // Check entries
    for entry in &response.entries {
        assert!(!entry.work_log_id.is_empty());

        // Check formatted fields
        if let (Some(raw), Some(formatted)) =
            (&entry.total_work_submitted, &entry.total_work_submitted_formatted)
        {
            assert_formatted_cycles(raw, formatted);
        }

        if let (Some(raw), Some(formatted)) =
            (&entry.total_actual_rewards, &entry.total_actual_rewards_formatted)
        {
            assert_formatted_zkc(raw, formatted);
        }
    }

    // Check summary if present
    if let Some(summary) = response.summary {
        assert!(summary.get("total_epochs_with_work").is_some());
        assert!(summary.get("total_work_all_time").is_some());
        assert!(summary.get("total_work_all_time_formatted").is_some());

        if let (Some(raw), Some(formatted)) = (
            summary.get("total_work_all_time").and_then(|v| v.as_str()),
            summary.get("total_work_all_time_formatted").and_then(|v| v.as_str()),
        ) {
            assert_formatted_cycles(raw, formatted);
        }

        if let (Some(raw), Some(formatted)) = (
            summary.get("total_capped_rewards_all_time").and_then(|v| v.as_str()),
            summary.get("total_capped_rewards_all_time_formatted").and_then(|v| v.as_str()),
        ) {
            assert_formatted_zkc(raw, formatted);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_epoch_povw_endpoint() {
    let response: PaginatedResponse<PovwEntry> =
        get_json("/v1/povw/epochs/2?limit=10").await.expect("Failed to get epoch PoVW");

    // Check entries for epoch-specific fields
    for entry in &response.entries {
        assert!(!entry.work_log_id.is_empty());
        assert_eq!(entry.epoch, Some(2));

        // Check formatted fields
        if let (Some(raw), Some(formatted)) =
            (&entry.work_submitted, &entry.work_submitted_formatted)
        {
            assert_formatted_cycles(raw, formatted);
        }

        if let (Some(raw), Some(formatted)) =
            (&entry.actual_rewards, &entry.actual_rewards_formatted)
        {
            assert_formatted_zkc(raw, formatted);
        }
    }

    // Check epoch summary if present
    if let Some(summary) = response.summary {
        assert_eq!(summary.get("epoch").and_then(|v| v.as_u64()), Some(2));
        assert!(summary.get("total_work").is_some());
        assert!(summary.get("total_work_formatted").is_some());

        if let (Some(raw), Some(formatted)) = (
            summary.get("total_work").and_then(|v| v.as_str()),
            summary.get("total_work_formatted").and_then(|v| v.as_str()),
        ) {
            assert_formatted_cycles(raw, formatted);
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_staking_address_history() {
    // Test with a known address from the data (you may need to adjust this)
    let test_address = "0x2408e37489c231f883126c87e8aadbad782a040a";
    let path = format!("/v1/staking/addresses/{}", test_address);

    let result: Result<PaginatedResponse<StakingEntry>, _> = get_json(&path).await;

    // It's okay if the address doesn't exist, but the endpoint should respond
    match result {
        Ok(response) => {
            // If we get data, validate it
            for entry in &response.entries {
                assert_eq!(entry.staker_address.to_lowercase(), test_address.to_lowercase());

                if let (Some(raw), Some(formatted)) =
                    (&entry.staked_amount, &entry.staked_amount_formatted)
                {
                    assert_formatted_zkc(raw, formatted);
                }
            }

            // Check address summary if present
            if let Some(summary) = response.summary {
                if let Some(addr) = summary.get("staker_address").and_then(|v| v.as_str()) {
                    assert_eq!(addr.to_lowercase(), test_address.to_lowercase());
                }

                if let (Some(raw), Some(formatted)) = (
                    summary.get("total_staked").and_then(|v| v.as_str()),
                    summary.get("total_staked_formatted").and_then(|v| v.as_str()),
                ) {
                    assert_formatted_zkc(raw, formatted);
                }
            }
        }
        Err(e) => {
            // Check if it's a 404 (address not found) which is acceptable
            let error_str = e.to_string();
            assert!(
                error_str.contains("404") || error_str.contains("not found"),
                "Unexpected error: {}",
                error_str
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_povw_address_history() {
    // Test with a known work log ID from the data (you may need to adjust this)
    let test_address = "0x94072d2282cb2c718d23d5779a5f8484e2530f2a";
    let path = format!("/v1/povw/addresses/{}", test_address);

    let result: Result<PaginatedResponse<PovwEntry>, _> = get_json(&path).await;

    match result {
        Ok(response) => {
            // If we get data, validate it
            for entry in &response.entries {
                assert_eq!(entry.work_log_id.to_lowercase(), test_address.to_lowercase());

                if let (Some(raw), Some(formatted)) =
                    (&entry.work_submitted, &entry.work_submitted_formatted)
                {
                    assert_formatted_cycles(raw, formatted);
                }

                if let (Some(raw), Some(formatted)) =
                    (&entry.actual_rewards, &entry.actual_rewards_formatted)
                {
                    assert_formatted_zkc(raw, formatted);
                }
            }

            // Check address summary if present
            if let Some(summary) = response.summary {
                if let Some(addr) = summary.get("work_log_id").and_then(|v| v.as_str()) {
                    assert_eq!(addr.to_lowercase(), test_address.to_lowercase());
                }

                if let (Some(raw), Some(formatted)) = (
                    summary.get("total_work_submitted").and_then(|v| v.as_str()),
                    summary.get("total_work_submitted_formatted").and_then(|v| v.as_str()),
                ) {
                    assert_formatted_cycles(raw, formatted);
                }
            }
        }
        Err(e) => {
            // Check if it's a 404 (address not found) which is acceptable
            let error_str = e.to_string();
            assert!(
                error_str.contains("404") || error_str.contains("not found"),
                "Unexpected error: {}",
                error_str
            );
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_pagination_parameters() {
    // Test with different pagination parameters
    let response1: PaginatedResponse<StakingEntry> =
        get_json("/v1/staking?limit=5&offset=0").await.expect("Failed to get page 1");

    let response2: PaginatedResponse<StakingEntry> =
        get_json("/v1/staking?limit=5&offset=5").await.expect("Failed to get page 2");

    // Check that pagination is working
    assert!(response1.entries.len() <= 5);
    assert!(response2.entries.len() <= 5);
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 5);

    // Check that ranks are sequential
    if !response1.entries.is_empty() && !response2.entries.is_empty() {
        let last_rank_page1 = response1.entries.last().unwrap().rank;
        let first_rank_page2 = response2.entries.first().unwrap().rank;
        assert_eq!(
            first_rank_page2,
            last_rank_page1 + 1,
            "Ranks should be sequential across pages"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_invalid_endpoint_returns_error() {
    let result: Result<Value, _> = get_json("/v1/nonexistent").await;
    assert!(result.is_err(), "Invalid endpoint should return an error");
}

#[tokio::test]
#[ignore]
async fn test_invalid_address_format() {
    // Test with invalid address format
    let result: Result<PaginatedResponse<StakingEntry>, _> =
        get_json("/v1/staking/addresses/invalid_address").await;

    assert!(result.is_err(), "Invalid address should return an error");
    if let Err(e) = result {
        let error_str = e.to_string();
        assert!(
            error_str.contains("400") || error_str.contains("Invalid"),
            "Should return appropriate error for invalid address"
        );
    }
}
