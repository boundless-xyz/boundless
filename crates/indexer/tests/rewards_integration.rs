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

use std::{sync::Arc, time::Duration};

use alloy::primitives::{utils::format_ether, U256};
use boundless_indexer::{
    db::rewards::{RewardsDb, RewardsIndexerDb},
    rewards::{RewardsIndexerService, RewardsIndexerServiceConfig},
};
use boundless_povw::deployments::MAINNET as POVW_MAINNET;
use tempfile::NamedTempFile;
use tracing_test::traced_test;
use url::Url;

#[tokio::test]
#[traced_test]
#[ignore = "Requires ETH_RPC_URL to be set and makes real RPC calls"]
async fn test_rewards_indexer_integration() {
    // Get RPC URL from environment
    let rpc_url = std::env::var("ETH_RPC_URL")
        .expect("ETH_RPC_URL must be set to run this test");
    let rpc_url = Url::parse(&rpc_url).expect("Invalid RPC URL");

    // Use mainnet deployment addresses
    let deployment = POVW_MAINNET;

    // Create test database
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let db_url = format!("sqlite:{}", temp_file.path().display());
    let db = Arc::new(RewardsDb::new(&db_url).await.expect("Failed to create database"));

    // Configure the rewards indexer
    let config = RewardsIndexerServiceConfig {
        interval: Duration::from_secs(60),
        retries: 3,
        start_block: None, // Will use default mainnet starting block
    };

    tracing::info!("=== Starting Rewards Indexer Integration Test ===");
    tracing::info!("RPC URL: {}", rpc_url);
    tracing::info!("ZKC Address: {:#x}", deployment.zkc_address);
    tracing::info!("veZKC Address: {:#x}", deployment.vezkc_address);
    tracing::info!("PoVW Accounting Address: {:#x}", deployment.povw_accounting_address);
    tracing::info!("");

    // Create rewards indexer service
    let mut service = RewardsIndexerService::new(
        rpc_url,
        deployment.vezkc_address,
        deployment.zkc_address,
        deployment.povw_accounting_address,
        &db_url,
        config,
    )
    .await
    .expect("Failed to create rewards indexer service");

    // Run the indexer once to fetch and process all data
    tracing::info!("Running rewards indexer");
    service.run().await.expect("Failed to run rewards indexer");
    tracing::info!("Indexer run completed successfully!");
    tracing::info!("");

    // Get current epoch from database
    let current_epoch = db.get_current_epoch()
        .await
        .expect("Failed to get current epoch")
        .expect("Current epoch not set");

    tracing::info!("=== Current Epoch: {} ===", current_epoch);
    tracing::info!("");

    // Print leaderboards for recent epochs
    let epochs_to_show = if current_epoch > 2 {
        vec![current_epoch, current_epoch - 1, current_epoch - 2, current_epoch - 3]
    } else {
        (0..=current_epoch).collect()
    };

    for epoch in epochs_to_show {
        tracing::info!("=== Epoch {} Leaderboard (Top 10) ===", epoch);
        print_epoch_leaderboard(&*db, epoch).await;
        tracing::info!("");
    }

    // Print aggregate leaderboard
    tracing::info!("=== Aggregate Leaderboard (All Time Top 20) ===");
    print_aggregate_leaderboard(&*db).await;
}

async fn print_epoch_leaderboard(db: &dyn RewardsIndexerDb, epoch: u64) {
    let rewards = db.get_povw_rewards_by_epoch(epoch, 0, 10)
        .await
        .expect("Failed to get epoch rewards");

    if rewards.is_empty() {
        tracing::info!("No rewards data for epoch {}", epoch);
        return;
    }

    tracing::info!("{:<44} {:>20} {:>4} {:>8} {:>8}",
        "Work Log ID", "Work Submitted", "Rewards (ZKC)", "% Share", "Capped");
    tracing::info!("{}", "-".repeat(104));

    for reward in rewards {
        let work_submitted = format_u256_short(reward.work_submitted);
        let actual_rewards = format_ether(reward.actual_rewards);
        let percentage = format!("{:.2}%", reward.percentage);
        let capped = if reward.is_capped { "Yes" } else { "No" };

        tracing::info!("{:<44} {:>20} {:>20} {:>8} {:>8}",
            format!("{:#x}", reward.work_log_id),
            work_submitted,
            actual_rewards,
            percentage,
            capped
        );
    }
}

async fn print_aggregate_leaderboard(db: &dyn RewardsIndexerDb) {
    let aggregates = db.get_povw_rewards_aggregate(0, 20)
        .await
        .expect("Failed to get aggregate rewards");

    if aggregates.is_empty() {
        tracing::info!("No aggregate rewards data");
        return;
    }

    tracing::info!("{:<44} {:>20} {:>20} {:>20} {:>8}",
        "Work Log ID", "Total Work", "Actual Rewards (ZKC)", "Uncapped Rewards", "Epochs");
    tracing::info!("{}", "-".repeat(116));

    for (rank, aggregate) in aggregates.iter().enumerate() {
        let total_work = format_u256_short(aggregate.total_work_submitted);
        let total_actual = format_ether(aggregate.total_actual_rewards);
        let total_uncapped = format_ether(aggregate.total_uncapped_rewards);

        tracing::info!("#{:<3} {:<40} {:>20} {:>20} {:>20} {:>8}",
            rank + 1,
            format!("{:#x}", aggregate.work_log_id),
            total_work,
            total_actual,
            total_uncapped,
            aggregate.epochs_participated
        );
    }
}


// Format U256 in a shorter readable format
fn format_u256_short(value: U256) -> String {
    if value == U256::ZERO {
        return "0".to_string();
    }

    value.to_string()
    // let len = value_str.len();

    // if len > 12 {
    //     // Use scientific notation for very large numbers
    //     format!("{}e{}", &value_str[0..4], len - 1)
    // } else {
    // }
}