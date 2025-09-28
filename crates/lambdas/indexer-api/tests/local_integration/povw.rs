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

//! Integration tests for PoVW API endpoints

use serde::Deserialize;
use serde_json::Value;
use test_log::test;

use super::{PaginatedResponse, TestEnv};

#[derive(Debug, Deserialize)]
struct PovwEntry {
    rank: Option<u64>,
    work_log_id: String,
    epoch: Option<u64>,
    work_submitted: Option<String>,
    work_submitted_formatted: Option<String>,
    actual_rewards: Option<String>,
    actual_rewards_formatted: Option<String>,
    total_work_submitted: Option<String>,
    total_work_submitted_formatted: Option<String>,
    total_actual_rewards: Option<String>,
    total_actual_rewards_formatted: Option<String>,
    epochs_participated: Option<u64>,
    percentage: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct PovwAddressResponse {
    entries: Vec<PovwEntry>,
    pagination: super::PaginationMetadata,
    summary: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct EpochsSummaryResponse {
    entries: Vec<EpochSummary>,
    pagination: super::PaginationMetadata,
}

#[derive(Debug, Deserialize)]
struct EpochSummary {
    epoch: u64,
    total_work: String,
    total_emissions: String,
    total_capped_rewards: String,
    total_uncapped_rewards: String,
    epoch_start_time: u64,
    epoch_end_time: u64,
    num_participants: u64,
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_povw_leaderboard() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test default leaderboard
    let response: PaginatedResponse<PovwEntry> = env.get("/v1/povw/addresses").await?;
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: PaginatedResponse<PovwEntry> = env.get("/v1/povw/addresses?limit=5").await?;
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
async fn test_povw_epochs_summary() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test epochs summary
    let response: EpochsSummaryResponse = env.get("/v1/povw/epochs").await?;

    // Verify we have some epochs
    assert!(!response.entries.is_empty(), "Should have at least one epoch");

    // Verify epoch structure
    let epoch = &response.entries[0];
    assert!(epoch.epoch > 0);
    assert!(epoch.epoch_start_time > 0);
    assert!(epoch.epoch_end_time > epoch.epoch_start_time);

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_povw_epoch_details() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test specific epoch (epoch 4 usually has data)
    let response: PaginatedResponse<PovwEntry> = env.get("/v1/povw/epochs/4").await?;

    // Verify all entries are for the requested epoch
    for entry in &response.entries {
        assert_eq!(entry.epoch, Some(4));
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_povw_address() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Use a known address with PoVW data
    let address = "0x4a48ad93e826a0b64602b8ba7f86b056f079e609";
    let path = format!("/v1/povw/addresses/{}", address);

    let response: PovwAddressResponse = env.get(&path).await?;

    // Verify address-specific response
    for entry in &response.entries {
        // Verify rank is not present for individual address queries
        assert!(entry.rank.is_none());
        // Verify work_log_id matches the address pattern
        assert!(entry.work_log_id.to_lowercase().contains(&address[2..]));
    }

    // Check summary if present
    if let Some(summary) = response.summary {
        assert!(summary.get("work_log_id").is_some());
        assert!(summary.get("epochs_participated").is_some());
    }

    Ok(())
}

#[test(tokio::test)]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_povw_pagination() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test pagination with offset
    let response1: PaginatedResponse<PovwEntry> = env.get("/v1/povw?limit=2").await?;
    let response2: PaginatedResponse<PovwEntry> = env.get("/v1/povw?limit=2&offset=2").await?;

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && response2.entries.len() > 0 {
        assert_ne!(response1.entries[0].work_log_id, response2.entries[0].work_log_id);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);

    Ok(())
}