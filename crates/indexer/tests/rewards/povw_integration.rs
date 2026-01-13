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
async fn test_povw_summary_stats() {
    let db = common::setup_test_db().await;

    let stats = db
        .get_povw_summary_stats()
        .await
        .expect("Failed to get PoVW summary stats")
        .expect("No PoVW summary stats found");

    // Check specific values matching the actual indexed data
    assert_eq!(stats.total_epochs_with_work, 3);
    assert_eq!(stats.total_unique_work_log_ids, 26);
    assert_eq!(stats.total_work_all_time.to_string(), "24999835418624");
    assert_eq!(stats.total_emissions_all_time.to_string(), "1395361974850288500000000");
    assert_eq!(stats.total_capped_rewards_all_time.to_string(), "54999464530233482198753");
    assert_eq!(stats.total_uncapped_rewards_all_time.to_string(), "837217107775305749999989");
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_epoch_povw_summary() {
    let db = common::setup_test_db().await;

    // Test epoch 3 summary
    let epoch3 = db
        .get_epoch_povw_summary(3)
        .await
        .expect("Failed to get epoch 3 PoVW summary")
        .expect("No epoch 3 PoVW summary found");

    assert_eq!(epoch3.epoch, 3);
    assert_eq!(epoch3.total_work.to_string(), "22364014854144");
    assert_eq!(epoch3.num_participants, 21);
    assert_eq!(epoch3.total_capped_rewards.to_string(), "40087246525823817857153");

    // Test epoch 4 summary
    let epoch4 = db
        .get_epoch_povw_summary(4)
        .await
        .expect("Failed to get epoch 4 PoVW summary")
        .expect("No epoch 4 PoVW summary found");

    assert_eq!(epoch4.epoch, 4);
    assert_eq!(epoch4.total_work.to_string(), "0"); // Epoch 4 has participants but no work yet
    assert_eq!(epoch4.num_participants, 10);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_rewards_by_epoch() {
    let db = common::setup_test_db().await;

    // Test top rewards for epoch 3
    let rewards = db
        .get_povw_rewards_by_epoch(3, 0, 3)
        .await
        .expect("Failed to get PoVW rewards for epoch 3");

    assert!(rewards.len() >= 3);

    // Check top earner
    let top = &rewards[0];
    assert_eq!(format!("{:#x}", top.work_log_id), "0x94072d2282cb2c718d23d5779a5f8484e2530f2a");
    assert_eq!(top.work_submitted.to_string(), "14928086204416");
    assert_eq!(top.actual_rewards.to_string(), "20000000000000000000000"); // 20000 ZKC
    assert!(top.is_capped);

    // Check second earner
    let second = &rewards[1];
    assert_eq!(format!("{:#x}", second.work_log_id), "0x0ab71eb0727536b179b2d009316b201b43a049fa");
    assert_eq!(second.work_submitted.to_string(), "1798892077056");
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_povw_rewards_aggregate() {
    let db = common::setup_test_db().await;

    // Test aggregate rewards for top performers
    let aggregates =
        db.get_povw_rewards_aggregate(0, 5).await.expect("Failed to get PoVW rewards aggregate");

    assert!(aggregates.len() >= 5);

    // Check top aggregate earner
    let top = &aggregates[0];
    assert_eq!(format!("{:#x}", top.work_log_id), "0x94072d2282cb2c718d23d5779a5f8484e2530f2a");
    assert_eq!(top.total_work_submitted.to_string(), "18245963022336");
    assert_eq!(top.epochs_participated, 3);

    // Check that actual_rewards <= uncapped_rewards (capping applied)
    assert!(top.total_actual_rewards <= top.total_uncapped_rewards);
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_all_epoch_povw_summaries() {
    let db = common::setup_test_db().await;

    let summaries = db
        .get_all_epoch_povw_summaries(0, 10)
        .await
        .expect("Failed to get all epoch PoVW summaries");

    // Should have epochs 0-4 (5 total)
    assert_eq!(summaries.len(), 5);

    // Note: epochs are returned in reverse order (4, 3, 2, 1, 0)
    // Just verify we have all expected epochs
    let all_epochs: Vec<u64> = summaries.iter().map(|s| s.epoch).collect();
    assert!(all_epochs.contains(&0));
    assert!(all_epochs.contains(&1));
    assert!(all_epochs.contains(&2));
    assert!(all_epochs.contains(&3));
    assert!(all_epochs.contains(&4));

    // Check that epochs 1, 2, 3 have actual work (4 has participants but no work yet)
    let epochs_with_work: Vec<u64> = summaries
        .iter()
        .filter(|s| s.total_work > alloy::primitives::U256::from(0))
        .map(|s| s.epoch)
        .collect();

    // Epochs 1, 2, 3 should have actual work submitted
    assert!(epochs_with_work.contains(&1));
    assert!(epochs_with_work.contains(&2));
    assert!(epochs_with_work.contains(&3));
}
