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

//! Test utilities for local integration tests

use assert_cmd::Command;
use reqwest::Client;
use serde::Deserialize;
use std::{
    env,
    net::TcpListener,
    path::PathBuf,
    time::Duration,
};
use tempfile::TempDir;
use tokio::process::{Child, Command as TokioCommand};
use tracing::{debug, info};

// Test modules
#[path = "local_integration/povw.rs"]
pub mod povw_tests;

#[path = "local_integration/staking.rs"]
pub mod staking_tests;

#[path = "local_integration/delegations.rs"]
pub mod delegations_tests;

#[path = "local_integration/docs.rs"]
pub mod docs_tests;

// Contract addresses for mainnet
const VEZKC_ADDRESS: &str = "0xE8Ae8eE8ffa57F6a79B6Cbe06BAFc0b05F3ffbf4";
const ZKC_ADDRESS: &str = "0x000006c2A22ff4A44ff1f5d0F2ed65F781F55555";
const POVW_ACCOUNTING_ADDRESS: &str = "0x319bd4050b2170a7aE3Ead3E6d5AB8a5c7cFBDF8";

// Indexer limits for faster tests
const END_EPOCH: u32 = 4;
const END_BLOCK: u32 = 23395398;

/// Test environment for a single test
pub struct TestEnv {
    api_url: String,
    _temp_dir: TempDir,
    _api_process: Child,
}

impl TestEnv {
    /// Get the API URL
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    /// Create a new test environment
    pub async fn new() -> anyhow::Result<Self> {
        // Initialize tracing if not already done
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();

        info!("Creating test environment...");

        // Check for ETH_RPC_URL
        let rpc_url = env::var("ETH_RPC_URL")
            .expect("ETH_RPC_URL environment variable must be set");

        // Create temp directory for database
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // Build binaries using assert_cmd (which handles cargo build automatically)
        info!("Building binaries...");

        // Run indexer to populate database
        info!("Running indexer to populate database...");
        Self::run_indexer(&rpc_url, &db_path).await?;

        // Find available port
        let api_port = Self::find_available_port()?;

        // Start API server
        info!("Starting API server on port {}...", api_port);
        let api_process = Self::start_api_server(&db_path, api_port).await?;

        // Wait for API to be ready
        let api_url = format!("http://127.0.0.1:{}", api_port);
        Self::wait_for_api(&api_url).await?;

        Ok(TestEnv {
            api_url,
            _temp_dir: temp_dir,
            _api_process: api_process,
        })
    }

    /// Run indexer to populate database
    async fn run_indexer(rpc_url: &str, db_path: &PathBuf) -> anyhow::Result<()> {
        // Create empty database file
        std::fs::File::create(db_path)?;

        let db_url = format!("sqlite:{}", db_path.display());
        info!("Using database at {}", db_path.display());

        // Use assert_cmd to get the path to the binary
        let cmd = Command::cargo_bin("rewards-indexer")?;
        let program = cmd.get_program();

        // Build command with tokio
        let mut child = TokioCommand::new(program)
            .args(&[
                "--rpc-url", rpc_url,
                "--vezkc-address", VEZKC_ADDRESS,
                "--zkc-address", ZKC_ADDRESS,
                "--povw-accounting-address", POVW_ACCOUNTING_ADDRESS,
                "--db", &db_url,
                "--interval", "600",
                "--end-epoch", &END_EPOCH.to_string(),
                "--end-block", &END_BLOCK.to_string(),
            ])
            .env("DATABASE_URL", &db_url)
            .env("RUST_LOG", "debug,sqlx=warn")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Spawn tasks to read and log output
        let stdout = child.stdout.take().expect("Failed to take stdout");
        let stderr = child.stderr.take().expect("Failed to take stderr");

        // Read stdout in background
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(target: "indexer", "{}", line);
            }
        });

        // Read stderr in background
        tokio::spawn(async move {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                debug!(target: "indexer", "stderr: {}", line);
            }
        });

        // Wait for database to be populated
        info!("Waiting for database to be populated...");
        let mut populated = false;
        for i in 0..60 {
            tokio::time::sleep(Duration::from_secs(1)).await;

            if Self::check_database_populated(db_path).await {
                info!("Database populated after {} seconds", i + 1);
                populated = true;
                break;
            }

            // Print progress every 5 seconds
            if (i + 1) % 5 == 0 {
                let size = std::fs::metadata(db_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                info!("Still waiting... ({}/60 seconds, DB size: {} bytes)", i + 1, size);
            }
        }

        // Stop the indexer
        info!("Stopping indexer...");
        child.kill().await?;
        let _ = child.wait().await;

        if !populated {
            anyhow::bail!("Database was not populated after 60 seconds");
        }

        Ok(())
    }

    /// Check if database has been populated with data
    async fn check_database_populated(db_path: &PathBuf) -> bool {
        use sqlx::sqlite::SqlitePool;

        let db_url = format!("sqlite:{}", db_path.display());

        match SqlitePool::connect(&db_url).await {
            Ok(pool) => {
                // Query for count of epochs in the epoch_povw_summary table
                let query = "SELECT COUNT(*) FROM epoch_povw_summary";
                match sqlx::query_scalar::<_, i64>(query).fetch_one(&pool).await {
                    Ok(count) => {
                        debug!("Found {} epochs in epoch_povw_summary table", count);
                        // We expect END_EPOCH epochs based on --end-epoch parameter
                        if count >= END_EPOCH as i64 {
                            info!("Found {} epochs, sleeping 3 more seconds for other tables to populate...", count);
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            true
                        } else {
                            false
                        }
                    },
                    Err(e) => {
                        // Table might not exist yet or query failed
                        if e.to_string().contains("no such table") {
                            debug!("epoch_povw_summary table doesn't exist yet");
                        } else {
                            debug!("Failed to query epoch_povw_summary: {}", e);
                        }
                        false
                    }
                }
            },
            Err(e) => {
                debug!("Failed to connect to database: {}", e);
                false
            }
        }
    }

    /// Find an available port for the API server
    fn find_available_port() -> anyhow::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok(port)
    }

    /// Start the API server
    async fn start_api_server(db_path: &PathBuf, port: u16) -> anyhow::Result<Child> {
        let db_url = format!("sqlite:{}", db_path.display());
        info!("Starting API server on port {} with database {}", port, db_path.display());

        // Use assert_cmd to get the path to the binary
        let cmd = Command::cargo_bin("local-server")?;
        let program = cmd.get_program();

        // Build command with tokio
        let child = TokioCommand::new(program)
            .env("DB_URL", &db_url)
            .env("PORT", port.to_string())
            .env("RUST_LOG", "debug,tower_http=debug,sqlx=warn")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        Ok(child)
    }

    /// Wait for API server to be ready
    async fn wait_for_api(api_url: &str) -> anyhow::Result<()> {
        let client = Client::new();
        let health_url = format!("{}/health", api_url);
        info!("Waiting for API server to be ready at {}...", health_url);

        for i in 0..30 {
            if let Ok(response) = client.get(&health_url).send().await {
                if response.status().is_success() {
                    info!("API server is ready after {} attempts", i + 1);
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }

        anyhow::bail!("API server did not start within 15 seconds")
    }

    /// Make a GET request to the API
    pub async fn get<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str
    ) -> anyhow::Result<T> {
        let url = format!("{}{}", self.api_url, path);
        let client = Client::new();
        let response = client.get(&url).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await?;
            anyhow::bail!("Request failed with status {}: {}", status, text);
        }

        Ok(response.json().await?)
    }
}

// Common response structures
#[derive(Debug, Deserialize)]
pub struct PaginatedResponse<T> {
    pub entries: Vec<T>,
    pub pagination: PaginationMetadata,
}

#[derive(Debug, Deserialize)]
pub struct PaginationMetadata {
    pub count: usize,
    pub offset: u64,
    pub limit: u64,
}

#[derive(Debug, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}