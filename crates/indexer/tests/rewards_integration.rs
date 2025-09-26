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
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use url::Url;

// Snapshot data structures for testing
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct RewardSnapshot {
    work_log_id: String,        // Hex string representation
    work_submitted: String,     // String representation of U256
    actual_rewards_zkc: String, // Formatted ZKC with 2 decimal places
    percentage: f64,
    is_capped: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct EpochSnapshot {
    epoch: u64,
    rewards: Vec<RewardSnapshot>,
}

// Helper function to format ZKC values with 2 decimal places
fn format_zkc(value: U256) -> String {
    let formatted = format_ether(value);
    if let Ok(num) = formatted.parse::<f64>() {
        format!("{:.2}", num)
    } else {
        formatted
    }
}

/// Generate a snapshot for a specific epoch
///
/// This function fetches the top 10 rewards for an epoch from the database
/// and creates a deterministic snapshot that can be compared across test runs.
///
/// # Arguments
/// * `db` - The rewards database interface
/// * `epoch` - The epoch number to generate a snapshot for
///
/// # Returns
/// An EpochSnapshot containing the formatted reward data
async fn generate_epoch_snapshot(
    db: &dyn RewardsIndexerDb,
    epoch: u64,
) -> anyhow::Result<EpochSnapshot> {
    let rewards = db.get_povw_rewards_by_epoch(epoch, 0, 10).await?;

    let reward_snapshots: Vec<RewardSnapshot> = rewards
        .into_iter()
        .map(|r| RewardSnapshot {
            work_log_id: format!("{:#x}", r.work_log_id),
            work_submitted: r.work_submitted.to_string(),
            actual_rewards_zkc: format_zkc(r.actual_rewards),
            percentage: (r.percentage * 100.0).round() / 100.0, // Round to 2 decimal places
            is_capped: r.is_capped,
        })
        .collect();

    Ok(EpochSnapshot { epoch, rewards: reward_snapshots })
}

/// Verify that an actual snapshot matches the expected snapshot
///
/// This function performs a deep comparison of all fields in the snapshot,
/// ensuring that the rewards data remains consistent across test runs.
///
/// # Arguments
/// * `actual` - The snapshot generated from the current test run
/// * `expected` - The expected snapshot stored in the test
///
/// # Returns
/// * `Ok(())` if snapshots match exactly
/// * `Err(String)` with a detailed error message if they differ
fn verify_snapshot(actual: &EpochSnapshot, expected: &EpochSnapshot) -> Result<(), String> {
    if actual.epoch != expected.epoch {
        return Err(format!("Epoch mismatch: expected {}, got {}", expected.epoch, actual.epoch));
    }

    if actual.rewards.len() != expected.rewards.len() {
        return Err(format!(
            "Epoch {}: Different number of rewards: expected {}, got {}",
            actual.epoch,
            expected.rewards.len(),
            actual.rewards.len()
        ));
    }

    for (i, (actual_reward, expected_reward)) in
        actual.rewards.iter().zip(expected.rewards.iter()).enumerate()
    {
        if actual_reward != expected_reward {
            return Err(format!(
                "Epoch {} reward #{}: Mismatch\n  Expected: {:?}\n  Actual:   {:?}",
                actual.epoch,
                i + 1,
                expected_reward,
                actual_reward
            ));
        }
    }

    tracing::info!("Epoch {} snapshot verified successfully", actual.epoch);

    Ok(())
}

/// Get expected snapshots for epochs 1-4
///
/// # How to Refresh Snapshots
///
/// 1. **Initial Capture**: When the expected rewards are empty (as they are initially),
///    the test will run in "capture mode" and print the JSON snapshots for each epoch.
///
/// 2. **Run the Test**: Execute the test with the required environment variable:
///    ```bash
///    ETH_RPC_URL='https://your-rpc-url' cargo test -p boundless-indexer \
///      --test rewards_integration -- --nocapture --ignored
///    ```
///
/// 3. **Copy the Output**: The test will print JSON snapshots like:
///    ```
///    === Captured Snapshot for Epoch 1 ===
///    {
///      "epoch": 1,
///      "rewards": [...]
///    }
///    ```
///
/// 4. **Update this Function**: Copy the JSON output and update the rewards vectors
///    below with the actual data. Convert the JSON to the Rust struct format.
///
/// 5. **Verify**: Run the test again. It should now verify the snapshots instead
///    of capturing them, ensuring consistency across runs.
///
/// # Note
/// These snapshots represent the expected state of epochs 1-4 on mainnet.
/// Since these epochs are complete, the data should never change, making them
/// ideal for regression testing.
fn get_expected_snapshots() -> Vec<EpochSnapshot> {
    vec![
        EpochSnapshot {
            epoch: 1,
            rewards: vec![RewardSnapshot {
                work_log_id: "0xade5c4b00ab283608928c29e55917899da8ac608".to_string(),
                work_submitted: "791052288".to_string(),
                actual_rewards_zkc: "6197.67".to_string(),
                percentage: 100.0,
                is_capped: true,
            }],
        },
        EpochSnapshot {
            epoch: 2,
            rewards: vec![
                RewardSnapshot {
                    work_log_id: "0x94072d2282cb2c718d23d5779a5f8484e2530f2a".to_string(),
                    work_submitted: "2528854704128".to_string(),
                    actual_rewards_zkc: "8666.67".to_string(),
                    percentage: 95.97,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xc559d9bef7df0076f71a42ea58723f429652b789".to_string(),
                    work_submitted: "103164936192".to_string(),
                    actual_rewards_zkc: "19.33".to_string(),
                    percentage: 3.91,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xcff679e6c8bd4fad045b56e6154aed94e30e2392".to_string(),
                    work_submitted: "2620391424".to_string(),
                    actual_rewards_zkc: "19.89".to_string(),
                    percentage: 0.09,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xefab3aeda1b67955d5838c2d620aed4bbe0ca540".to_string(),
                    work_submitted: "310640640".to_string(),
                    actual_rewards_zkc: "6.67".to_string(),
                    percentage: 0.01,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x15a9a6a719c89ecfd7fca1893b975d68ab2d77a9".to_string(),
                    work_submitted: "78839808".to_string(),
                    actual_rewards_zkc: "2.00".to_string(),
                    percentage: 0.0,
                    is_capped: true,
                },
            ],
        },
        EpochSnapshot {
            epoch: 3,
            rewards: vec![
                RewardSnapshot {
                    work_log_id: "0x94072d2282cb2c718d23d5779a5f8484e2530f2a".to_string(),
                    work_submitted: "14928086204416".to_string(),
                    actual_rewards_zkc: "20000.00".to_string(),
                    percentage: 66.75,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x0ab71eb0727536b179b2d009316b201b43a049fa".to_string(),
                    work_submitted: "1798892077056".to_string(),
                    actual_rewards_zkc: "133.33".to_string(),
                    percentage: 8.04,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x7b0dc4df73f268f3c64fcc3a2a92145d864c1b2f".to_string(),
                    work_submitted: "1486911455232".to_string(),
                    actual_rewards_zkc: "0.00".to_string(),
                    percentage: 6.64,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x411f9da23559ec7b6956b408eb9310a59fd48a9d".to_string(),
                    work_submitted: "1286460538880".to_string(),
                    actual_rewards_zkc: "269.27".to_string(),
                    percentage: 5.75,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x0164ec96442196a02931f57e7e20fa59cff43845".to_string(),
                    work_submitted: "706961784832".to_string(),
                    actual_rewards_zkc: "8825.20".to_string(),
                    percentage: 3.16,
                    is_capped: false,
                },
                RewardSnapshot {
                    work_log_id: "0xc9ebe28d3a2a61c11383e2ac7c774463fb944050".to_string(),
                    work_submitted: "646421643264".to_string(),
                    actual_rewards_zkc: "13.33".to_string(),
                    percentage: 2.89,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xefab3aeda1b67955d5838c2d620aed4bbe0ca540".to_string(),
                    work_submitted: "484144791552".to_string(),
                    actual_rewards_zkc: "4537.44".to_string(),
                    percentage: 2.16,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x72b480dc22d2b69651d451df9942f2f6d0ee8e69".to_string(),
                    work_submitted: "230875004928".to_string(),
                    actual_rewards_zkc: "1039.52".to_string(),
                    percentage: 1.03,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x73f6055c2d4e7c5c094ee29618142f217be44ea6".to_string(),
                    work_submitted: "200250261504".to_string(),
                    actual_rewards_zkc: "666.67".to_string(),
                    percentage: 0.89,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x70681bbacb02f27aa75a07044dabf499ad9a3666".to_string(),
                    work_submitted: "140230115328".to_string(),
                    actual_rewards_zkc: "1387.99".to_string(),
                    percentage: 0.62,
                    is_capped: true,
                },
            ],
        },
        EpochSnapshot {
            epoch: 4,
            rewards: vec![
                RewardSnapshot {
                    work_log_id: "0x94072d2282cb2c718d23d5779a5f8484e2530f2a".to_string(),
                    work_submitted: "4946507382784".to_string(),
                    actual_rewards_zkc: "46666.67".to_string(),
                    percentage: 20.27,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x70681bbacb02f27aa75a07044dabf499ad9a3666".to_string(),
                    work_submitted: "3682565308416".to_string(),
                    actual_rewards_zkc: "3153.99".to_string(),
                    percentage: 15.09,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x0164ec96442196a02931f57e7e20fa59cff43845".to_string(),
                    work_submitted: "2879995428864".to_string(),
                    actual_rewards_zkc: "32962.84".to_string(),
                    percentage: 11.8,
                    is_capped: false,
                },
                RewardSnapshot {
                    work_log_id: "0x10182508ccd485572f38882a73c1d472b63c43e6".to_string(),
                    work_submitted: "1705515417600".to_string(),
                    actual_rewards_zkc: "19520.39".to_string(),
                    percentage: 6.98,
                    is_capped: false,
                },
                RewardSnapshot {
                    work_log_id: "0xefab3aeda1b67955d5838c2d620aed4bbe0ca540".to_string(),
                    work_submitted: "1670123225088".to_string(),
                    actual_rewards_zkc: "4627.92".to_string(),
                    percentage: 6.84,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x68dfa9fe2b48bcfbcb2ed8b51b7dfa1a0a98de33".to_string(),
                    work_submitted: "1255673626624".to_string(),
                    actual_rewards_zkc: "5125.25".to_string(),
                    percentage: 5.14,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xc9ebe28d3a2a61c11383e2ac7c774463fb944050".to_string(),
                    work_submitted: "1193088892928".to_string(),
                    actual_rewards_zkc: "66.67".to_string(),
                    percentage: 4.88,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0x8335d28f192a1c10bf33725716ecacaa3cb5d581".to_string(),
                    work_submitted: "896476938240".to_string(),
                    actual_rewards_zkc: "3540.00".to_string(),
                    percentage: 3.67,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xdc6ceb47b323b799d2f75e5aafb303d8fce8d6e1".to_string(),
                    work_submitted: "773299470336".to_string(),
                    actual_rewards_zkc: "5334.00".to_string(),
                    percentage: 3.16,
                    is_capped: true,
                },
                RewardSnapshot {
                    work_log_id: "0xa8424595a39f874cb3357e3dd064cbce368b2627".to_string(),
                    work_submitted: "735298404352".to_string(),
                    actual_rewards_zkc: "3.23".to_string(),
                    percentage: 3.01,
                    is_capped: true,
                },
            ],
        },
    ]
}

/// Integration test for the rewards indexer with snapshot verification
///
/// This test performs the following:
/// 1. Runs the rewards indexer to fetch and process all blockchain data
/// 2. Verifies epochs 1-4 against stored snapshots to ensure consistency
/// 3. Displays current epoch data (epoch 5+) for manual verification
///
/// # Snapshot Testing
///
/// The test includes snapshot testing for historical epochs (1-4) which are complete
/// and should never change. This ensures that:
/// - The indexer logic remains consistent across code changes
/// - Historical data is correctly processed
/// - Any regressions are immediately caught
///
/// # Environment Variables
///
/// - `ETH_RPC_URL`: Required. The Ethereum RPC endpoint to use (e.g., Alchemy, Infura)
/// - `RISC0_DEV_MODE`: Optional. Set to 1 for faster test execution in development
///
/// # Running the Test
///
/// ```bash
/// # Standard run with snapshot verification
/// ETH_RPC_URL='https://your-rpc-url' cargo test -p boundless-indexer \
///   --test rewards_integration -- --nocapture --ignored
///
/// # Development mode (faster)
/// ETH_RPC_URL='https://your-rpc-url' RISC0_DEV_MODE=1 cargo test -p boundless-indexer \
///   --test rewards_integration -- --nocapture --ignored
/// ```
///
/// # Updating Snapshots
///
/// See the documentation for `get_expected_snapshots()` for instructions on how
/// to capture and update the snapshot data.
#[tokio::test]
#[ignore = "Requires ETH_RPC_URL to be set and makes real RPC calls"]
async fn test_rewards_indexer_integration() {
    // Setup tracing subscriber for real-time log output
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_target(true)
                .with_thread_ids(true)
                .with_level(true)
                .with_file(false)
                .with_line_number(false),
        )
        .with(EnvFilter::try_from_env("RUST_LOG").unwrap_or_else(|_| {
            EnvFilter::new("info,boundless_indexer=debug,boundless_rewards=debug")
        }))
        .try_init()
        .ok(); // Ignore error if already initialized
               // Get RPC URL from environment
    let rpc_url = std::env::var("ETH_RPC_URL").expect("ETH_RPC_URL must be set to run this test");
    let rpc_url = Url::parse(&rpc_url).expect("Invalid RPC URL");

    // Use mainnet deployment addresses
    let deployment = POVW_MAINNET;
    let _zkc_deployment = boundless_zkc::deployments::MAINNET;

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

    println!("=== Starting Rewards Indexer Integration Test ===");
    println!("RPC URL: {}", rpc_url);
    println!("ZKC Address: {:#x}", deployment.zkc_address);
    println!("veZKC Address: {:#x}", deployment.vezkc_address);
    println!("PoVW Accounting Address: {:#x}", deployment.povw_accounting_address);

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
    println!("🚀 Running rewards indexer...");
    let test_start = std::time::Instant::now();
    service.run().await.expect("Failed to run rewards indexer");
    println!(
        "✅ Indexer run completed successfully in {:.2}s!",
        test_start.elapsed().as_secs_f64()
    );

    // Get current epoch from database
    println!("📊 Displaying results...");
    let current_epoch = db
        .get_current_epoch()
        .await
        .expect("Failed to get current epoch")
        .expect("Current epoch not set");

    println!("=== Current Epoch: {} ===", current_epoch);

    // Snapshot testing for epochs 1-4 (finished epochs)
    // These epochs are complete and their data should never change, making them
    // perfect for regression testing. Any changes detected here indicate either:
    // 1. A bug in the indexer logic
    // 2. An unexpected change in the blockchain data (which shouldn't happen for historical epochs)
    println!("\n=== Snapshot Testing for Epochs 1-4 ===");
    let expected_snapshots = get_expected_snapshots();
    let mut snapshot_failures = Vec::new();

    for expected in &expected_snapshots {
        println!("Processing snapshot for epoch {}...", expected.epoch);
        match generate_epoch_snapshot(&*db, expected.epoch).await {
            Ok(actual) => {
                println!(
                    "Epoch {} snapshot generated with {} rewards",
                    actual.epoch,
                    actual.rewards.len()
                );

                // If we're in capture mode (empty expected rewards), print the JSON
                // This allows developers to easily capture the initial baseline data
                if expected.rewards.is_empty() {
                    if !actual.rewards.is_empty() {
                        println!("\n🎯 === Captured Snapshot for Epoch {} ===", actual.epoch);
                        println!("Copy the JSON below to update the expected snapshot:");
                        let json = serde_json::to_string_pretty(&actual)
                            .expect("Failed to serialize snapshot");
                        println!("{}", json);
                        println!("=== End of Epoch {} Snapshot ===\n", actual.epoch);
                    } else {
                        println!("⚠️  Epoch {} has no rewards data (empty snapshot)", actual.epoch);
                    }
                } else {
                    // Otherwise, verify the snapshot against expected data
                    println!(
                        "Verifying epoch {} snapshot (expected {} rewards)...",
                        actual.epoch,
                        expected.rewards.len()
                    );
                    if let Err(e) = verify_snapshot(&actual, expected) {
                        snapshot_failures.push(e);
                        println!("❌ Epoch {} snapshot verification failed", expected.epoch);
                    } else {
                        println!("✅ Epoch {} snapshot verified successfully", expected.epoch);
                    }
                }
            }
            Err(e) => {
                let msg =
                    format!("Failed to generate snapshot for epoch {}: {}", expected.epoch, e);
                println!("{}", msg);
                if !expected.rewards.is_empty() {
                    // Only fail if we expected rewards
                    snapshot_failures.push(msg);
                }
            }
        }
    }

    // Report snapshot testing results
    let has_expected_data = expected_snapshots.iter().any(|s| !s.rewards.is_empty());

    if has_expected_data {
        // We're in verification mode
        if !snapshot_failures.is_empty() {
            println!("\n=== Snapshot Verification Failures ===");
            for failure in &snapshot_failures {
                println!("  - {}", failure);
            }
            panic!("Snapshot verification failed for {} epochs", snapshot_failures.len());
        } else {
            println!("\n✅ All snapshot verifications passed!");
        }
    } else {
        // We're in capture mode
        println!("\n📸 === Snapshot Capture Mode ===");
        println!("The test is currently running in CAPTURE MODE because no expected snapshots are defined.");
        println!("To enable snapshot verification:");
        println!("1. Look for the '🎯 === Captured Snapshot' sections above");
        println!("2. Copy each JSON snapshot");
        println!("3. Update the get_expected_snapshots() function with the captured data");
        println!("4. Run the test again to verify snapshots");
    }

    // Print simplified summary
    println!("\n=== REWARDS SUMMARY ===\n");

    // All-time summaries
    println!("📊 ACROSS ALL EPOCHS\n");
    print_top_povw_all_time(&*db, 5).await;
    print_top_staking_all_time(&*db, 5).await;

    // Epoch 4 specific (or latest epoch if < 4)
    let display_epoch = if current_epoch >= 4 { 4 } else { current_epoch };
    println!("\n📅 EPOCH {} SNAPSHOT\n", display_epoch);
    print_top_povw_epoch(&*db, display_epoch, 5).await;
    print_top_staking_epoch(&*db, display_epoch, 5).await;

    // Delegation powers
    println!("\n🤝 DELEGATION POWERS\n");
    print_delegation_summary(&*db, 5).await;
}

async fn print_top_povw_all_time(db: &dyn RewardsIndexerDb, limit: usize) {
    // Get summary stats to show last_updated_at
    if let Ok(Some(summary)) = db.get_povw_summary_stats().await {
        if let Some(updated_at) = &summary.updated_at {
            println!("Top {} PoVW Miners (All Time) [Last Updated: {}]", limit, updated_at);
        } else {
            println!("Top {} PoVW Miners (All Time)", limit);
        }
    } else {
        println!("Top {} PoVW Miners (All Time)", limit);
    }
    println!("{:<42} {:>20} {:>20}", "Work Log ID", "Total Work", "Total Rewards (ZKC)");
    println!("{}", "-".repeat(84));

    let aggregates = db
        .get_povw_rewards_aggregate(0, limit as u64)
        .await
        .expect("Failed to get PoVW aggregates");

    for agg in aggregates {
        println!(
            "{:<42} {:>20} {:>20}",
            format!("{:#x}", agg.work_log_id),
            format_u256_short(agg.total_work_submitted),
            format_zkc(agg.total_actual_rewards)
        );
    }
}

async fn print_top_staking_all_time(db: &dyn RewardsIndexerDb, limit: usize) {
    // Get summary stats to show last_updated_at
    if let Ok(Some(summary)) = db.get_staking_summary_stats().await {
        if let Some(updated_at) = &summary.updated_at {
            println!("\nTop {} Staking Positions (All Time) [Last Updated: {}]", limit, updated_at);
        } else {
            println!("\nTop {} Staking Positions (All Time)", limit);
        }
    } else {
        println!("\nTop {} Staking Positions (All Time)", limit);
    }
    println!("{:<42} {:>15} {:>15} {:>15}", "Staker Address", "Staked (ZKC)", "Generated", "Earned");
    println!("{}", "-".repeat(90));

    let aggregates = db
        .get_staking_positions_aggregate(0, limit as u64)
        .await
        .expect("Failed to get staking aggregates");

    for agg in aggregates {
        println!(
            "{:<42} {:>15} {:>15} {:>15}",
            format!("{:#x}", agg.staker_address),
            format_zkc(agg.total_staked),
            format_zkc(agg.total_rewards_generated),
            format_zkc(agg.total_rewards_earned)
        );
        // Show delegation if present
        if let Some(delegate) = agg.rewards_delegated_to {
            if delegate != agg.staker_address {
                println!("  └─ Rewards delegated to: {:#x}", delegate);
            }
        }
    }
}

async fn print_top_povw_epoch(db: &dyn RewardsIndexerDb, epoch: u64, limit: usize) {
    // Get epoch summary to show last_updated_at
    if let Ok(Some(summary)) = db.get_epoch_povw_summary(epoch).await {
        if let Some(updated_at) = &summary.updated_at {
            println!("Top {} PoVW Miners (Epoch {}) [Last Updated: {}]", limit, epoch, updated_at);
        } else {
            println!("Top {} PoVW Miners (Epoch {})", limit, epoch);
        }
    } else {
        println!("Top {} PoVW Miners (Epoch {})", limit, epoch);
    }
    println!("{:<42} {:>20} {:>20} {:>8}", "Work Log ID", "Work Submitted", "Rewards (ZKC)", "Capped");
    println!("{}", "-".repeat(92));

    let rewards = db
        .get_povw_rewards_by_epoch(epoch, 0, limit as u64)
        .await
        .expect("Failed to get epoch PoVW rewards");

    if rewards.is_empty() {
        println!("  No PoVW data for epoch {}", epoch);
    } else {
        for reward in rewards {
            println!(
                "{:<42} {:>20} {:>20} {:>8}",
                format!("{:#x}", reward.work_log_id),
                format_u256_short(reward.work_submitted),
                format_zkc(reward.actual_rewards),
                if reward.is_capped { "Yes" } else { "No" }
            );
        }
    }
}

async fn print_top_staking_epoch(db: &dyn RewardsIndexerDb, epoch: u64, limit: usize) {
    // Get epoch summary to show last_updated_at
    if let Ok(Some(summary)) = db.get_epoch_staking_summary(epoch).await {
        if let Some(updated_at) = &summary.updated_at {
            println!("\nTop {} Stakers (Epoch {}) [Last Updated: {}]", limit, epoch, updated_at);
        } else {
            println!("\nTop {} Stakers (Epoch {})", limit, epoch);
        }
    } else {
        println!("\nTop {} Stakers (Epoch {})", limit, epoch);
    }

    // Get staking positions
    let positions = db
        .get_staking_positions_by_epoch(epoch, 0, limit as u64)
        .await
        .expect("Failed to get epoch staking positions");

    // Get staking rewards for the same epoch to show earned amounts
    let rewards = db
        .get_staking_rewards_by_epoch(epoch, 0, limit as u64 * 2) // Get more to find all addresses
        .await
        .expect("Failed to get staking rewards");

    // Create a map of address -> rewards_earned for quick lookup
    let mut earned_map = std::collections::HashMap::new();
    for reward in rewards {
        earned_map.insert(reward.staker_address, reward.rewards_earned);
    }

    // Get epoch summary for rewards info
    let summary = db
        .get_epoch_staking_summary(epoch)
        .await
        .expect("Failed to get epoch staking summary");

    // Get total staked from summary for accurate percentage calculation
    let total_staked = summary.as_ref().map(|s| s.total_staked).unwrap_or(U256::ZERO);

    if let Some(ref summary) = summary {
        println!("Total Staking Emissions: {} ZKC", format_zkc(summary.total_staking_emissions));
        println!("Total Staked: {} ZKC", format_zkc(summary.total_staked));
    }

    println!("{:<42} {:>12} {:>8} {:>12} {:>12}", "Staker Address", "Staked (ZKC)", "%", "Generated", "Earned");
    println!("{}", "-".repeat(90));

    if positions.is_empty() {
        println!("  No staking data for epoch {}", epoch);
    } else {
        for position in positions {
            // Get the earned amount for this address (might be 0 if delegated)
            let earned = earned_map.get(&position.staker_address).copied().unwrap_or(U256::ZERO);

            // Calculate percentage of total stake
            let percentage = if total_staked > U256::ZERO {
                (position.staked_amount * U256::from(10000) / total_staked).to::<u64>() as f64 / 100.0
            } else {
                0.0
            };

            // Show staking position with both generated and earned rewards
            println!(
                "{:<42} {:>12} {:>7.2}% {:>12} {:>12}",
                format!("{:#x}", position.staker_address),
                format_zkc(position.staked_amount),
                percentage,
                format_zkc(position.rewards_generated),
                format_zkc(earned)
            );
            // Show delegation if present
            if let Some(delegate) = position.rewards_delegated_to {
                if delegate != position.staker_address {
                    println!("  └─ Rewards delegated to: {:#x}", delegate);
                }
            }
        }
    }
}

async fn print_delegation_summary(db: &dyn RewardsIndexerDb, limit: usize) {
    // Get current epoch
    let current_epoch = db
        .get_current_epoch()
        .await
        .expect("Failed to get current epoch")
        .expect("Current epoch not set");

    // Vote delegation powers
    println!("Vote Power Delegates (Top {})", limit);
    println!("{:<42} {:>20} {:>12}", "Delegate Address", "Vote Power (ZKC)", "Delegators");
    println!("{}", "-".repeat(76));

    let vote_powers = db
        .get_vote_delegation_powers_by_epoch(current_epoch, 0, limit as u64)
        .await
        .expect("Failed to get vote delegation powers");

    if vote_powers.is_empty() {
        println!("  No vote delegations in epoch {}", current_epoch);
    } else {
        for power in vote_powers {
            println!(
                "{:<42} {:>20} {:>12}",
                format!("{:#x}", power.delegate_address),
                format_zkc(power.vote_power),
                power.delegator_count
            );
        }
    }

    // Reward delegation powers
    println!("\nReward Power Delegates (Top {})", limit);
    println!("{:<42} {:>20} {:>12}", "Delegate Address", "Reward Power (ZKC)", "Delegators");
    println!("{}", "-".repeat(76));

    let reward_powers = db
        .get_reward_delegation_powers_by_epoch(current_epoch, 0, limit as u64)
        .await
        .expect("Failed to get reward delegation powers");

    if reward_powers.is_empty() {
        println!("  No reward delegations in epoch {}", current_epoch);
    } else {
        for power in reward_powers {
            println!(
                "{:<42} {:>20} {:>12}",
                format!("{:#x}", power.delegate_address),
                format_zkc(power.reward_power),
                power.delegator_count
            );
        }
    }
}

// Removed old verbose print functions - using simplified versions above

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

// Removed old verbose functions

#[allow(dead_code)]
async fn test_address_history_methods(db: &dyn RewardsIndexerDb) {
    use alloy::primitives::U256;

    // First, get the top staker from the aggregate staking positions
    let top_stakers =
        db.get_staking_positions_aggregate(0, 1).await.expect("Failed to get top stakers");

    if top_stakers.is_empty() {
        println!("No staking data available to test address history methods");
        return;
    }

    let top_staker = &top_stakers[0];
    println!("\n📍 Testing address history for top staker: {:#x}", top_staker.staker_address);
    println!("   Total staked: {} ZKC", format_zkc(top_staker.total_staked));
    println!("   Epochs participated: {}", top_staker.epochs_participated);

    // Test get_staking_history_by_address
    println!("\n1️⃣ Testing get_staking_history_by_address...");
    let staking_history = db
        .get_staking_history_by_address(
            top_staker.staker_address,
            None, // No start epoch limit
            None, // No end epoch limit
        )
        .await
        .expect("Failed to get staking history");

    println!("   Found {} epochs of staking history", staking_history.len());
    if !staking_history.is_empty() {
        // Show first 3 epochs
        for position in staking_history.iter().take(3) {
            println!(
                "   Epoch {}: {} ZKC, withdrawing: {}",
                position.epoch,
                format_zkc(position.staked_amount),
                position.is_withdrawing
            );
            if let Some(rewards_delegate) = position.rewards_delegated_to {
                println!("      Rewards delegated to: {:#x}", rewards_delegate);
            }
            if let Some(votes_delegate) = position.votes_delegated_to {
                println!("      Votes delegated to: {:#x}", votes_delegate);
            }
        }
        if staking_history.len() > 3 {
            println!("   ... and {} more epochs", staking_history.len() - 3);
        }
    }

    // Test get_povw_rewards_history_by_address
    println!("\n2️⃣ Testing get_povw_rewards_history_by_address...");
    let povw_history = db
        .get_povw_rewards_history_by_address(top_staker.staker_address, None, None)
        .await
        .expect("Failed to get PoVW rewards history");

    println!("   Found {} epochs of PoVW rewards", povw_history.len());
    if !povw_history.is_empty() {
        let total_rewards: U256 =
            povw_history.iter().map(|r| r.actual_rewards).fold(U256::ZERO, |acc, r| acc + r);
        println!("   Total rewards earned: {} ZKC", format_zkc(total_rewards));

        // Show first 3 epochs
        for reward in povw_history.iter().take(3) {
            println!(
                "   Epoch {}: {} ZKC rewards ({:.2}% of epoch, capped: {})",
                reward.epoch,
                format_zkc(reward.actual_rewards),
                reward.percentage,
                reward.is_capped
            );
        }
        if povw_history.len() > 3 {
            println!("   ... and {} more epochs", povw_history.len() - 3);
        }
    }

    // Test delegation history methods
    println!("\n3️⃣ Testing get_vote_delegations_received_history...");
    let vote_delegations = db
        .get_vote_delegations_received_history(top_staker.staker_address, None, None)
        .await
        .expect("Failed to get vote delegations history");

    if !vote_delegations.is_empty() {
        println!("   Address received vote delegations in {} epochs", vote_delegations.len());
        for delegation in vote_delegations.iter().take(2) {
            println!(
                "   Epoch {}: {} ZKC vote power from {} delegators",
                delegation.epoch,
                format_zkc(delegation.vote_power),
                delegation.delegator_count
            );
        }
    } else {
        println!("   No vote delegations received");
    }

    println!("\n4️⃣ Testing get_reward_delegations_received_history...");
    let reward_delegations = db
        .get_reward_delegations_received_history(top_staker.staker_address, None, None)
        .await
        .expect("Failed to get reward delegations history");

    if !reward_delegations.is_empty() {
        println!("   Address received reward delegations in {} epochs", reward_delegations.len());
        for delegation in reward_delegations.iter().take(2) {
            println!(
                "   Epoch {}: {} ZKC reward power from {} delegators",
                delegation.epoch,
                format_zkc(delegation.reward_power),
                delegation.delegator_count
            );
        }
    } else {
        println!("   No reward delegations received");
    }

    // Test with epoch range limits
    println!("\n5️⃣ Testing with epoch range limits...");
    let current_epoch =
        db.get_current_epoch().await.expect("Failed to get current epoch").unwrap_or(0);

    if current_epoch > 2 {
        let limited_history = db
            .get_staking_history_by_address(
                top_staker.staker_address,
                Some(current_epoch - 2), // Start 2 epochs ago
                Some(current_epoch),     // End at current epoch
            )
            .await
            .expect("Failed to get limited staking history");

        println!(
            "   Limited query (epochs {}-{}): found {} epochs",
            current_epoch - 2,
            current_epoch,
            limited_history.len()
        );
    }

    println!("\n✅ Address history methods test completed successfully!");
}
