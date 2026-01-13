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

use super::common;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_summary_stats() {
    let db = common::setup_test_db().await;

    let stats = db
        .get_staking_summary_stats()
        .await
        .expect("Failed to get staking summary stats")
        .expect("No staking summary stats found");

    // Check specific values matching the actual indexed data
    // Total reflects ALL stakers in the system (not just top 2)
    assert_eq!(stats.current_total_staked.to_string(), "4330465936598121426217840");
    assert_eq!(stats.total_unique_stakers, 343);
    assert_eq!(stats.current_active_stakers, 343);
    assert_eq!(stats.current_withdrawing, 11);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_epoch_staking_summary() {
    let db = common::setup_test_db().await;

    // Test epoch 3 summary
    let epoch3 = db
        .get_epoch_staking_summary(3)
        .await
        .expect("Failed to get epoch 3 staking summary")
        .expect("No epoch 3 staking summary found");

    assert_eq!(epoch3.epoch, 3);
    assert_eq!(epoch3.num_stakers, 311);
    assert_eq!(epoch3.total_staked.to_string(), "3685477115558191540906493");
    assert_eq!(epoch3.num_withdrawing, 7);

    // Test epoch 4 summary
    let epoch4 = db
        .get_epoch_staking_summary(4)
        .await
        .expect("Failed to get epoch 4 staking summary")
        .expect("No epoch 4 staking summary found");

    assert_eq!(epoch4.epoch, 4);
    assert!(epoch4.num_stakers >= 340); // Should have 343 stakers by epoch 4
    assert!(epoch4.total_staked.to_string().starts_with("43")); // ~4.3M ZKC total
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_positions_by_epoch() {
    let db = common::setup_test_db().await;

    // Test staking positions for epoch 3
    let positions = db
        .get_staking_positions_by_epoch(3, 0, 5)
        .await
        .expect("Failed to get staking positions for epoch 3");

    assert!(positions.len() >= 2); // Should have at least 2 stakers

    // Check top staker
    let top = &positions[0];
    assert_eq!(format!("{:#x}", top.staker_address), "0x2408e37489c231f883126c87e8aadbad782a040a");
    assert_eq!(top.staked_amount.to_string(), "726927981342423248000000");
    assert!(!top.is_withdrawing);

    // Check second staker
    let second = &positions[1];
    assert_eq!(
        format!("{:#x}", second.staker_address),
        "0x7cc3376b8d38b2c923cd9d5164f9d74e303482b2"
    );
    assert_eq!(second.staked_amount.to_string(), "603060340000000000000000");
    assert!(!second.is_withdrawing);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_staking_positions_aggregate() {
    let db = common::setup_test_db().await;

    let aggregates = db
        .get_staking_positions_aggregate(0, 5)
        .await
        .expect("Failed to get staking positions aggregate");

    assert!(aggregates.len() >= 2); // Returns top 5, but we have many stakers

    // Check top aggregate staker
    let top = &aggregates[0];
    assert_eq!(format!("{:#x}", top.staker_address), "0x2408e37489c231f883126c87e8aadbad782a040a");
    assert_eq!(top.total_staked.to_string(), "726927981342423248000000");
    assert_eq!(top.epochs_participated, 3);
    assert!(!top.is_withdrawing);

    // Check rewards delegation
    assert_eq!(
        top.rewards_delegated_to.map(|addr| format!("{:#x}", addr)),
        Some("0x0164ec96442196a02931f57e7e20fa59cff43845".to_string())
    );
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_all_epoch_staking_summaries() {
    let db = common::setup_test_db().await;

    let summaries = db
        .get_all_epoch_staking_summaries(0, 10)
        .await
        .expect("Failed to get all epoch staking summaries");

    // Should have epochs 0-4 (5 total)
    assert_eq!(summaries.len(), 5);

    // Verify we have all expected epochs (may be returned in any order)
    let all_epochs: Vec<u64> = summaries.iter().map(|s| s.epoch).collect();
    assert!(all_epochs.contains(&0));
    assert!(all_epochs.contains(&1));
    assert!(all_epochs.contains(&2));
    assert!(all_epochs.contains(&3));
    assert!(all_epochs.contains(&4));

    // Check that epochs 2, 3, 4 have stakers (and possibly more)
    let epochs_with_stakers: Vec<u64> =
        summaries.iter().filter(|s| s.num_stakers > 0).map(|s| s.epoch).collect();

    // At least epochs 2, 3, 4 should have stakers
    assert!(epochs_with_stakers.contains(&2));
    assert!(epochs_with_stakers.contains(&3));
    assert!(epochs_with_stakers.contains(&4));
}
