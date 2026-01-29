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

//! Integration tests for PoVW API endpoints

use indexer_api::models::{
    AddressLeaderboardResponse, AggregateLeaderboardEntry, EpochLeaderboardEntry, EpochPoVWSummary,
    LeaderboardResponse, PoVWAddressSummary, PoVWSummaryStats,
};

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_leaderboard() {
    let env = TestEnv::shared().await;

    // Test default leaderboard
    let response: LeaderboardResponse<AggregateLeaderboardEntry> =
        env.get("/v1/povw/addresses").await.unwrap();
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit of 3 to check top entries
    let response: LeaderboardResponse<AggregateLeaderboardEntry> =
        env.get("/v1/povw/addresses?limit=3").await.unwrap();
    assert!(response.entries.len() <= 3);
    assert_eq!(response.pagination.limit, 3);

    // Verify rank field is present for leaderboard
    if !response.entries.is_empty() {
        assert!(response.entries[0].rank.is_some());

        // Check specific values from real data for top 3
        if response.entries.len() >= 3 {
            let first = &response.entries[0];
            assert_eq!(first.work_log_id, "0x94072d2282cb2c718d23d5779a5f8484e2530f2a");
            assert_eq!(first.total_work_submitted, "18245963022336");
            assert_eq!(first.total_actual_rewards, "28666666666666666666666");
            assert_eq!(first.total_uncapped_rewards, "454178915961434029731928");
            assert_eq!(first.epochs_participated, 3);

            let second = &response.entries[1];
            assert_eq!(second.work_log_id, "0x0164ec96442196a02931f57e7e20fa59cff43845");
            assert_eq!(second.total_work_submitted, "2349000278016");
            assert_eq!(second.total_actual_rewards, "8825197537996492524728"); // Fixed: was 13540303064735614608777
            assert_eq!(second.total_uncapped_rewards, "8825197537996492524728"); // Fixed: was 13540303064735614608777
            assert_eq!(second.epochs_participated, 2);
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_summary() {
    let env = TestEnv::shared().await;

    // Test the summary endpoint
    let summary: PoVWSummaryStats = env.get("/v1/povw").await.unwrap();

    // Check specific values from real data
    assert_eq!(summary.total_epochs_with_work, 3);
    assert_eq!(summary.total_unique_work_log_ids, 26);
    assert_eq!(summary.total_work_all_time, "24999835418624");
    assert_eq!(summary.total_emissions_all_time, "1395361974850288500000000");
    assert_eq!(summary.total_capped_rewards_all_time, "54999464530233482198753");
    assert_eq!(summary.total_uncapped_rewards_all_time, "837217107775305749999989"); // Fixed: was 624997088546559733077848

    // Verify formatted strings are present
    assert_eq!(summary.total_work_all_time_formatted, "24,999,835,418,624 cycles");
    assert_eq!(summary.total_uncapped_rewards_all_time_formatted, "837217.107775305749999989 ZKC");
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_epochs_summary() {
    let env = TestEnv::shared().await;

    // Test epochs summary
    let response: LeaderboardResponse<EpochPoVWSummary> = env.get("/v1/povw/epochs").await.unwrap();

    // Verify we have exactly 4 epochs (matching our end-epoch parameter)
    assert_eq!(response.entries.len(), 5, "Should have epochs 0-4");

    // Verify epoch structure
    let epoch = &response.entries[0];
    assert!(epoch.epoch > 0);
    assert!(epoch.epoch_start_time > 0);
    assert!(epoch.epoch_end_time > epoch.epoch_start_time);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_epoch_details() {
    let env = TestEnv::shared().await;

    // Test specific epoch (epoch 4 usually has data)
    let response: LeaderboardResponse<EpochLeaderboardEntry> =
        env.get("/v1/povw/epochs/4/addresses").await.unwrap();

    // Verify all entries are for the requested epoch if we have data
    for entry in &response.entries {
        assert_eq!(entry.epoch, 4);
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_address() {
    let env = TestEnv::shared().await;

    // Use a known address with PoVW data
    let address = "0x4a48ad93e826a0b64602b8ba7f86b056f079e609";
    let path = format!("/v1/povw/addresses/{}", address);

    let response: AddressLeaderboardResponse<EpochLeaderboardEntry, PoVWAddressSummary> =
        env.get(&path).await.unwrap();

    // Verify address-specific response
    for entry in &response.entries {
        // Verify work_log_id matches the address pattern
        assert!(entry.work_log_id.to_lowercase().contains(&address[2..]));
    }

    // Check summary (always present in AddressLeaderboardResponse)
    let summary = &response.summary;
    assert!(summary.work_log_id.to_lowercase().contains(&address[2..]));
    // If there's data, verify it
    if !response.entries.is_empty() {
        assert!(summary.epochs_participated > 0);
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_pagination() {
    let env = TestEnv::shared().await;

    // Test pagination with offset
    let response1: LeaderboardResponse<AggregateLeaderboardEntry> =
        env.get("/v1/povw/addresses?limit=2").await.unwrap();
    let response2: LeaderboardResponse<AggregateLeaderboardEntry> =
        env.get("/v1/povw/addresses?limit=2&offset=2").await.unwrap();

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && !response2.entries.is_empty() {
        assert_ne!(response1.entries[0].work_log_id, response2.entries[0].work_log_id);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);
}
