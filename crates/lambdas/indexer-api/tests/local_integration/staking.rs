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

//! Integration tests for Staking API endpoints

use indexer_api::models::{
    AddressLeaderboardResponse, AggregateStakingEntry, EpochStakingEntry, EpochStakingSummary,
    LeaderboardResponse, StakingAddressSummary,
};

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_leaderboard() {
    let env = TestEnv::shared().await;

    // Test default leaderboard
    let response: LeaderboardResponse<AggregateStakingEntry> =
        env.get("/v1/staking/addresses").await.unwrap();
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit of 2 to check top entries
    let response: LeaderboardResponse<AggregateStakingEntry> =
        env.get("/v1/staking/addresses?limit=2").await.unwrap();
    assert!(response.entries.len() <= 2);
    assert_eq!(response.pagination.limit, 2);

    // Verify rank field is present for leaderboard
    if !response.entries.is_empty() {
        assert!(response.entries[0].rank.is_some());

        // Check specific values from real data for top 2
        if response.entries.len() >= 2 {
            let first = &response.entries[0];
            assert_eq!(first.staker_address, "0x2408e37489c231f883126c87e8aadbad782a040a");
            assert_eq!(first.total_staked, "726927981342423248000000");
            assert_eq!(first.total_rewards_generated, "43793837998280676959348");
            assert!(!first.is_withdrawing);
            assert_eq!(
                first.rewards_delegated_to,
                Some("0x0164ec96442196a02931f57e7e20fa59cff43845".to_string())
            );
            assert_eq!(first.epochs_participated, 3);

            let second = &response.entries[1];
            assert_eq!(second.staker_address, "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2");
            assert_eq!(second.total_staked, "603060340000000000000000");
            assert_eq!(second.total_rewards_generated, "28191507291258394253114");
            assert!(!second.is_withdrawing);
            assert_eq!(second.rewards_delegated_to, None);
            assert_eq!(second.epochs_participated, 2);
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_epochs_summary() {
    let env = TestEnv::shared().await;

    // Test epochs summary
    let response: LeaderboardResponse<EpochStakingSummary> =
        env.get("/v1/staking/epochs").await.unwrap();

    // Verify we have some epochs
    assert!(!response.entries.is_empty(), "Should have at least one epoch");

    // Verify epoch structure
    let epoch = &response.entries[0];
    assert!(epoch.epoch > 0);
    assert!(epoch.num_stakers > 0);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_epoch_details() {
    let env = TestEnv::shared().await;

    // Test specific epoch (epoch 3 should have data, we index up to epoch 4)
    let response: LeaderboardResponse<EpochStakingEntry> =
        env.get("/v1/staking/epochs/3/addresses").await.unwrap();

    // Verify all entries are for the requested epoch if we have data
    for entry in &response.entries {
        assert_eq!(entry.epoch, 3);
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_address() {
    let env = TestEnv::shared().await;

    // Use a known address with staking data
    let address = "0x00000000f2708738d4886bc4aedefd8dd04818b0";
    let path = format!("/v1/staking/addresses/{}", address);

    let response: AddressLeaderboardResponse<EpochStakingEntry, StakingAddressSummary> =
        env.get(&path).await.unwrap();

    // Verify address-specific response
    for entry in &response.entries {
        assert_eq!(entry.staker_address.to_lowercase(), address);
    }

    // Check summary (always present in AddressLeaderboardResponse)
    let summary = &response.summary;
    assert_eq!(summary.staker_address.to_lowercase(), address);
    // If there's data, verify it
    if !response.entries.is_empty() {
        assert!(summary.epochs_participated > 0);
        assert!(summary.total_staked != "0");
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_pagination() {
    let env = TestEnv::shared().await;

    // Test pagination with offset
    let response1: LeaderboardResponse<AggregateStakingEntry> =
        env.get("/v1/staking/addresses?limit=2").await.unwrap();
    let response2: LeaderboardResponse<AggregateStakingEntry> =
        env.get("/v1/staking/addresses?limit=2&offset=2").await.unwrap();

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && !response2.entries.is_empty() {
        assert_ne!(response1.entries[0].staker_address, response2.entries[0].staker_address);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);
}
