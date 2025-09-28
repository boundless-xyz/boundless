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

//! Integration tests for Delegations API endpoints

use serde::Deserialize;

use super::{PaginatedResponse, TestEnv};

#[derive(Debug, Deserialize)]
struct DelegationEntry {
    rank: u64,
    delegatee_address: String,
    total_power: String,
    total_power_formatted: String,
    delegator_count: u64,
}

#[tokio::test]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_delegations_votes_leaderboard() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test votes delegation leaderboard
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/votes").await?;

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/votes?limit=3").await?;
    assert!(response.entries.len() <= 3);
    assert_eq!(response.pagination.limit, 3);

    // Verify rank ordering if we have data
    if response.entries.len() > 1 {
        for i in 1..response.entries.len() {
            assert!(response.entries[i-1].rank < response.entries[i].rank);
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_delegations_rewards_leaderboard() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test rewards delegation leaderboard
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/rewards").await?;

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Test with limit
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/rewards?limit=3").await?;
    assert!(response.entries.len() <= 3);
    assert_eq!(response.pagination.limit, 3);

    // Verify rank ordering if we have data
    if response.entries.len() > 1 {
        for i in 1..response.entries.len() {
            assert!(response.entries[i-1].rank < response.entries[i].rank);
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_delegations_votes_by_epoch() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test votes delegation for a specific epoch
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/votes?epoch=7").await?;

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Verify we have entries (epoch 7 should have data)
    if response.pagination.count > 0 {
        assert!(!response.entries.is_empty());

        // Verify addresses are valid
        for entry in &response.entries {
            assert!(entry.delegatee_address.starts_with("0x"));
            assert_eq!(entry.delegatee_address.len(), 42);
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_delegations_rewards_by_epoch() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test rewards delegation for a specific epoch
    let response: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/rewards?epoch=7").await?;

    // Basic validation
    assert!(response.pagination.count <= response.pagination.limit as usize);

    // Verify we have entries (epoch 7 should have data)
    if response.pagination.count > 0 {
        assert!(!response.entries.is_empty());

        // Verify addresses are valid
        for entry in &response.entries {
            assert!(entry.delegatee_address.starts_with("0x"));
            assert_eq!(entry.delegatee_address.len(), 42);
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore = "Requires ETH_RPC_URL"]
async fn test_delegations_pagination() -> anyhow::Result<()> {
    let env = TestEnv::new().await?;

    // Test pagination for votes
    let response1: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/votes?limit=2").await?;
    let response2: PaginatedResponse<DelegationEntry> = env.get("/v1/delegations/votes?limit=2&offset=2").await?;

    // Ensure responses are different if we have enough data
    if response1.entries.len() == 2 && response2.entries.len() > 0 {
        assert_ne!(response1.entries[0].delegatee_address, response2.entries[0].delegatee_address);
    }

    // Verify pagination metadata
    assert_eq!(response1.pagination.offset, 0);
    assert_eq!(response2.pagination.offset, 2);

    Ok(())
}