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

use std::{env, sync::Arc, time::Duration};

use boundless_indexer::{
    db::rewards::{RewardsDb, RewardsIndexerDb},
    rewards::{RewardsIndexerService, RewardsIndexerServiceConfig},
};
use tempfile::NamedTempFile;
use tokio::sync::OnceCell;
use tracing_subscriber::EnvFilter;
use url::Url;

// Contract addresses for mainnet
const VEZKC_ADDRESS: &str = "0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4";
const ZKC_ADDRESS: &str = "0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555";
const POVW_ACCOUNTING_ADDRESS: &str = "0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8";

// Test limits for faster execution
const END_EPOCH: u64 = 4;
const END_BLOCK: u64 = 23395398;

// Store both the database and temp file to keep the file alive
// The RewardsDb type implements Send + Sync, unlike the trait object
struct TestDbState {
    db: Arc<RewardsDb>,
    _temp_file: NamedTempFile, // Kept alive as long as TestDbState exists
}

// Static storage for the shared test database. Ensures each test doesn't need to re-index from chain.
static TEST_DB: OnceCell<TestDbState> = OnceCell::const_new();

/// Get the shared test database, initializing it on first access
pub async fn setup_test_db() -> Arc<dyn RewardsIndexerDb> {
    let state = TEST_DB.get_or_init(|| async { initialize_test_db().await }).await;

    // Return the database as a trait object
    state.db.clone() as Arc<dyn RewardsIndexerDb>
}

async fn initialize_test_db() -> TestDbState {
    let _ = tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).try_init();

    // Get RPC URL from environment
    let rpc_url = env::var("ETH_MAINNET_RPC_URL")
        .expect("ETH_MAINNET_RPC_URL environment variable must be set");

    // Create temporary database file
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    let db_path = temp_file.path().to_str().expect("Invalid temp path");
    let db_url = format!("sqlite:{}", db_path);

    tracing::info!("Creating test database at: {}", db_path);

    // Create database connection
    let db = Arc::new(RewardsDb::new(&db_url).await.expect("Failed to create database"));

    // Configure indexer
    let config = RewardsIndexerServiceConfig {
        interval: Duration::from_secs(600),
        retries: 3,
        start_block: None,
        end_block: Some(END_BLOCK),
        end_epoch: Some(END_EPOCH),
        epochs_to_process: Some(10),
    };

    let mut service = RewardsIndexerService::new(
        Url::parse(&rpc_url).expect("Invalid RPC URL"),
        VEZKC_ADDRESS.parse().expect("Invalid veZKC address"),
        ZKC_ADDRESS.parse().expect("Invalid ZKC address"),
        POVW_ACCOUNTING_ADDRESS.parse().expect("Invalid PoVW address"),
        &db_url,
        config,
    )
    .await
    .expect("Failed to create indexer service");

    tracing::info!("Running indexer up to epoch {} (block {})", END_EPOCH, END_BLOCK);
    service.run().await.expect("Failed to run indexer");
    tracing::info!("Indexer completed successfully");

    TestDbState { db, _temp_file: temp_file }
}
