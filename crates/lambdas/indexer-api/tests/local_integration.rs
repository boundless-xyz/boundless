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
    process::{Child, Stdio},
    time::Duration,
};
use tempfile::TempDir;

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

/// Test environment with indexer and API server
pub struct TestEnv {
    pub db_path: PathBuf,
    pub api_url: String,
    pub api_port: u16,
    _temp_dir: TempDir,
    api_process: Option<Child>,
}

impl TestEnv {
    /// Create a new test environment
    pub async fn new() -> anyhow::Result<Self> {
        // Check for ETH_RPC_URL
        let rpc_url = env::var("ETH_RPC_URL")
            .expect("ETH_RPC_URL environment variable must be set");

        // Create temp directory for database
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");

        // Build binaries
        println!("Building binaries...");
        Self::build_binaries()?;

        // Run indexer to populate database
        println!("Running indexer to populate database...");
        Self::run_indexer(&rpc_url, &db_path, 30)?;

        // Find available port
        let api_port = Self::find_available_port()?;

        // Start API server
        println!("Starting API server on port {}...", api_port);
        let api_process = Self::start_api_server(&db_path, api_port)?;

        // Wait for API to be ready
        let api_url = format!("http://127.0.0.1:{}", api_port);
        Self::wait_for_api(&api_url).await?;

        Ok(Self {
            db_path,
            api_url,
            api_port,
            _temp_dir: temp_dir,
            api_process: Some(api_process),
        })
    }

    /// Build required binaries (no longer needed with assert_cmd)
    fn build_binaries() -> anyhow::Result<()> {
        // assert_cmd automatically builds binaries, so this is now a no-op
        Ok(())
    }

    /// Run indexer to populate database
    fn run_indexer(rpc_url: &str, db_path: &PathBuf, duration_secs: u64) -> anyhow::Result<()> {
        // Create empty database file
        std::fs::File::create(db_path)?;

        let db_url = format!("sqlite:{}", db_path.display());
        println!("Indexer: Using database at {}", db_path.display());

        // Use assert_cmd to build and get the binary
        let mut cmd = Command::cargo_bin("rewards-indexer")?;

        // Configure the command
        cmd.args(&[
                "--rpc-url", rpc_url,
                "--vezkc-address", VEZKC_ADDRESS,
                "--zkc-address", ZKC_ADDRESS,
                "--povw-accounting-address", POVW_ACCOUNTING_ADDRESS,
                "--db", &db_url,
                "--interval", "600",
            ])
            .env("DATABASE_URL", &db_url)
            .env("RUST_LOG", "debug,sqlx=warn");

        // Get the path and use std::process::Command to spawn
        let program = cmd.get_program().to_owned();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_owned()).collect();

        let mut child_cmd = std::process::Command::new(&program);
        child_cmd.args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // Apply environment variables
        for (key, value) in cmd.get_envs() {
            if let Some(val) = value {
                child_cmd.env(key, val);
            }
        }

        println!("Indexer: Starting indexer process...");
        let mut child = child_cmd.spawn()?;

        // Wait until database is populated (check every second for up to duration_secs)
        println!("Indexer: Waiting for database to be populated...");
        let mut populated = false;
        for i in 0..duration_secs {
            std::thread::sleep(Duration::from_secs(1));

            // Check if database has data using sqlite3
            if Self::check_database_populated(db_path) {
                println!("Indexer: Database populated after {} seconds", i + 1);
                populated = true;
                break;
            }

            // Print progress every 5 seconds
            if (i + 1) % 5 == 0 {
                let size = std::fs::metadata(db_path)
                    .map(|m| m.len())
                    .unwrap_or(0);
                println!("Indexer: Still waiting... ({}/{} seconds, DB size: {} bytes)",
                         i + 1, duration_secs, size);
            }
        }

        // Stop the indexer
        println!("Indexer: Stopping indexer process...");
        child.kill()?;
        let _ = child.wait();

        // Final check
        let metadata = std::fs::metadata(db_path)?;
        println!("Indexer: Database size after indexing: {} bytes", metadata.len());

        if !populated {
            anyhow::bail!("Database was not populated after {} seconds", duration_secs);
        }

        Ok(())
    }

    /// Check if database has been populated with data
    fn check_database_populated(db_path: &PathBuf) -> bool {
        use std::process::Command;

        // Try to query the database for epoch count
        let output = Command::new("sqlite3")
            .arg(db_path)
            .arg("SELECT COUNT(*) FROM epochs;")
            .output();

        match output {
            Ok(result) if result.status.success() => {
                let count_str = String::from_utf8_lossy(&result.stdout);
                let count: i32 = count_str.trim().parse().unwrap_or(0);
                count > 0
            },
            _ => {
                // Fallback: check if database file has grown significantly
                std::fs::metadata(db_path)
                    .map(|m| m.len() > 10_000_000) // At least 10MB
                    .unwrap_or(false)
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
    fn start_api_server(db_path: &PathBuf, port: u16) -> anyhow::Result<Child> {
        let db_url = format!("sqlite:{}", db_path.display());
        println!("API: Starting server on port {} with database {}", port, db_path.display());

        // Use assert_cmd to build and get the binary
        let mut cmd = Command::cargo_bin("local-server")?;

        // Configure the command
        cmd.env("DB_URL", &db_url)
            .env("PORT", port.to_string())
            .env("RUST_LOG", "debug,tower_http=debug,sqlx=warn");

        // Get the path and use std::process::Command to spawn
        let program = cmd.get_program().to_owned();
        let args: Vec<_> = cmd.get_args().map(|s| s.to_owned()).collect();

        let mut child_cmd = std::process::Command::new(&program);
        child_cmd.args(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        // Apply environment variables
        for (key, value) in cmd.get_envs() {
            if let Some(val) = value {
                child_cmd.env(key, val);
            }
        }

        println!("API: Spawning server process...");
        let child = child_cmd.spawn()?;

        Ok(child)
    }

    /// Wait for API server to be ready
    async fn wait_for_api(api_url: &str) -> anyhow::Result<()> {
        let client = Client::new();
        let health_url = format!("{}/health", api_url);
        println!("API: Waiting for server to be ready at {}...", health_url);

        for i in 0..30 {
            if let Ok(response) = client.get(&health_url).send().await {
                if response.status().is_success() {
                    println!("API: Server is ready after {} attempts", i + 1);
                    return Ok(());
                } else {
                    println!("API: Health check returned status: {}", response.status());
                }
            } else {
                println!("API: Health check failed, retrying...");
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

impl Drop for TestEnv {
    fn drop(&mut self) {
        // Kill API server
        if let Some(mut child) = self.api_process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
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