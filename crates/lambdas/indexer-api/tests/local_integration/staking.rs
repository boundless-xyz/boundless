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

//! Integration tests for Staking API endpoints

use serde::Deserialize;
use serde_json::Value;
use test_log::test;

use super::{PaginatedResponse, TestEnv};

#[derive(Debug, Deserialize)]
struct StakingEntry {
    rank: Option<u64>,
    staker_address: String,
    epoch: Option<u64>,
    staked_amount: Option<String>,
    staked_amount_formatted: Option<String>,
    is_withdrawing: bool,
    rewards_delegated_to: Option<String>,
    votes_delegated_to: Option<String>,
    rewards_generated: Option<String>,
    rewards_generated_formatted: Option<String>,
    total_staked: Option<String>,
    total_staked_formatted: Option<String>,
    epochs_participated: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct StakingAddressResponse {
    entries: Vec<StakingEntry>,
    pagination: super::PaginationMetadata,
    summary: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct StakingEpochsSummaryResponse {
    entries: Vec<EpochStakingSummary>,
    pagination: super::PaginationMetadata,
}

#[derive(Debug, Deserialize)]
struct EpochStakingSummary {
    epoch: u64,
    total_staked: String,
    num_stakers: u64,
    num_withdrawing: u64,
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_leaderboard() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test default leaderboard
    let response: PaginatedResponse<StakingEntry> = env.get("/v1/staking").await?;
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: PaginatedResponse<StakingEntry> = env.get("/v1/staking?limit=5").await?;
    assert!(response.entries.len() <= 5);
    assert_eq!(response.pagination.limit, 5);

    // Verify rank field is present for leaderboard
    if !response.entries.is_empty() {
        assert!(response.entries[0].rank.is_some());
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_epochs_summary() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test epochs summary
    let response: StakingEpochsSummaryResponse = env.get("/v1/staking/epochs").await?;

    // Verify we have some epochs
    assert!(!response.entries.is_empty(), "Should have at least one epoch");

    // Verify epoch structure
    let epoch = &response.entries[0];
    assert!(epoch.epoch > 0);
    assert!(epoch.num_stakers > 0);

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_epoch_details() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test specific epoch (epoch 7 usually has data)
    let response: PaginatedResponse<StakingEntry> = env.get("/v1/staking/epochs/7").await?;

    // Verify all entries are for the requested epoch
    for entry in &response.entries {
        assert_eq!(entry.epoch, Some(7));
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_address() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Use a known address with staking data
    let address = "0x00000000f2708738d4886bc4aedefd8dd04818b0";
    let path = format!("/v1/staking/addresses/{}", address);

    let response: StakingAddressResponse = env.get(&path).await?;

    // Verify address-specific response
    for entry in &response.entries {
        assert_eq!(entry.staker_address.to_lowercase(), address);
    }

    // Check summary if present
    if let Some(summary) = response.summary {
        assert!(summary.get("staker_address").is_some());
        assert!(summary.get("epochs_participated").is_some());
        assert!(summary.get("total_staked").is_some());
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_filters() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test filtering by withdrawing status
    let response: PaginatedResponse<StakingEntry> = env.get("/v1/staking?is_withdrawing=false").await?;

    // Verify all entries match the filter
    for entry in &response.entries {
        assert_eq!(entry.is_withdrawing, false);
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_staking_pagination() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test pagination with offset
    let response1: PaginatedResponse<StakingEntry> = env.get("/v1/staking?limit=2").await?;
    let response2: PaginatedResponse<StakingEntry> = env.get("/v1/staking?limit=2&offset=2").await?;

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && response2.entries.len() > 0 {
        assert_ne!(response1.entries[0].staker_address, response2.entries[0].staker_address);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);

    Ok(())
}