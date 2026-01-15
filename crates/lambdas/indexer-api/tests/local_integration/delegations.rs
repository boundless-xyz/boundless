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

//! Integration tests for Delegations API endpoints

use indexer_api::models::{DelegationPowerEntry, LeaderboardResponse};

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_delegations_votes_leaderboard() {
    let env = TestEnv::shared().await;

    // Test votes delegation leaderboard
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/votes/addresses").await.unwrap();

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/votes/addresses?limit=3").await.unwrap();
    assert!(response.entries.len() <= 3);
    assert_eq!(response.pagination.limit, 3);

    // Check specific values from real data for top entries
    if response.entries.len() >= 2 {
        let first = &response.entries[0];
        assert_eq!(first.delegate_address, "0x2408e37489c231f883126c87e8aadbad782a040a");
        assert_eq!(first.power, "726927981342423248000000");
        assert_eq!(first.delegator_count, 0);
        assert_eq!(first.delegators.len(), 0);

        let second = &response.entries[1];
        assert_eq!(second.delegate_address, "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2");
        assert_eq!(second.power, "603060340000000000000000");
        assert_eq!(second.delegator_count, 0);
        assert_eq!(second.delegators.len(), 0);
    }

    // Verify rank ordering if we have data
    if response.entries.len() > 1 {
        for i in 1..response.entries.len() {
            if let (Some(rank1), Some(rank2)) =
                (response.entries[i - 1].rank, response.entries[i].rank)
            {
                assert!(rank1 < rank2);
            }
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_delegations_rewards_leaderboard() {
    let env = TestEnv::shared().await;

    // Test rewards delegation leaderboard
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/rewards/addresses").await.unwrap();

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/rewards/addresses?limit=3").await.unwrap();
    assert!(response.entries.len() <= 3);
    assert_eq!(response.pagination.limit, 3);

    // Check specific values from real data for top entries
    if response.entries.len() >= 2 {
        let first = &response.entries[0];
        assert_eq!(first.delegate_address, "0x0164ec96442196a02931f57e7e20fa59cff43845");
        assert_eq!(first.power, "726927981342423248000000");
        assert_eq!(first.delegator_count, 1);
        assert_eq!(first.delegators.len(), 1);

        let second = &response.entries[1];
        assert_eq!(second.delegate_address, "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2");
        assert_eq!(second.power, "603060340000000000000000");
        assert_eq!(second.delegator_count, 0);
        assert_eq!(second.delegators.len(), 0);
    }

    // Verify rank ordering if we have data
    if response.entries.len() > 1 {
        for i in 1..response.entries.len() {
            if let (Some(rank1), Some(rank2)) =
                (response.entries[i - 1].rank, response.entries[i].rank)
            {
                assert!(rank1 < rank2);
            }
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_delegations_votes_by_epoch() {
    let env = TestEnv::shared().await;

    // Test votes delegation for a specific epoch (we index up to epoch 4)
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/votes/epochs/3/addresses").await.unwrap();

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Verify we have entries (epoch 3 should have data)
    if response.pagination.count > 0 {
        assert!(!response.entries.is_empty());

        // Verify addresses are valid
        for entry in &response.entries {
            assert!(entry.delegate_address.starts_with("0x"));
            assert_eq!(entry.delegate_address.len(), 42);
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_delegations_rewards_by_epoch() {
    let env = TestEnv::shared().await;

    // Test rewards delegation for a specific epoch (we index up to epoch 4)
    let response: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/rewards/epochs/3/addresses").await.unwrap();

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Verify we have entries (epoch 3 should have data)
    if response.pagination.count > 0 {
        assert!(!response.entries.is_empty());

        // Verify addresses are valid
        for entry in &response.entries {
            assert!(entry.delegate_address.starts_with("0x"));
            assert_eq!(entry.delegate_address.len(), 42);
        }
    }
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_delegations_pagination() {
    let env = TestEnv::shared().await;

    // Test pagination for votes
    let response1: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/votes/addresses?limit=2").await.unwrap();
    let response2: LeaderboardResponse<DelegationPowerEntry> =
        env.get("/v1/delegations/votes/addresses?limit=2&offset=2").await.unwrap();

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && !response2.entries.is_empty() {
        assert_ne!(response1.entries[0].delegate_address, response2.entries[0].delegate_address);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);
}
