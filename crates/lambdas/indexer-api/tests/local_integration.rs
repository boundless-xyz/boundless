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
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tempfile::NamedTempFile;
use tokio::{
    process::{Child, Command as TokioCommand},
    sync::OnceCell,
};
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

/// Shared test environment that persists across all tests
struct SharedTestEnv {
    api_url: String,
    _temp_file: NamedTempFile, // Keep the database file alive
    _api_process: Child,
}

/// Test environment handle for individual tests
pub struct TestEnv {
    api_url: String,
}

// Static storage for the shared test environment
static SHARED_TEST_ENV: OnceCell<Arc<SharedTestEnv>> = OnceCell::const_new();

impl TestEnv {
    /// Get the API URL
    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    /// Get or create the shared test environment
    pub async fn shared() -> Self {
        let shared_env = SHARED_TEST_ENV
            .get_or_init(|| async {
                Arc::new(
                    SharedTestEnv::initialize()
                        .await
                        .expect("Failed to initialize test environment"),
                )
            })
            .await;

        TestEnv { api_url: shared_env.api_url.clone() }
    }

    /// Make a GET request to the API
    pub async fn get<T: for<'de> Deserialize<'de>>(&self, path: &str) -> anyhow::Result<T> {
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

impl SharedTestEnv {
    /// Initialize the shared test environment (called only once)
    async fn initialize() -> anyhow::Result<Self> {
        // Initialize tracing if not already done
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .try_init();

        info!("Creating shared test environment...");

        // Check for ETH_MAINNET_RPC_URL
        let rpc_url = env::var("ETH_MAINNET_RPC_URL")
            .expect("ETH_MAINNET_RPC_URL environment variable must be set");

        // Create temp file for database
        let temp_file = NamedTempFile::new()?;
        let db_path = temp_file.path().to_path_buf();

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

        Ok(SharedTestEnv { api_url, _temp_file: temp_file, _api_process: api_process })
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
            .args([
                "--rpc-url",
                rpc_url,
                "--vezkc-address",
                VEZKC_ADDRESS,
                "--zkc-address",
                ZKC_ADDRESS,
                "--povw-accounting-address",
                POVW_ACCOUNTING_ADDRESS,
                "--db",
                &db_url,
                "--interval",
                "600",
                "--end-epoch",
                &END_EPOCH.to_string(),
                "--end-block",
                &END_BLOCK.to_string(),
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

        // Wait for indexer to complete (it should exit when it reaches --end-block)
        info!("Waiting for indexer to complete (will exit at block {})...", END_BLOCK);

        // Set a timeout for the indexer to complete
        let timeout = Duration::from_secs(120);
        let start = std::time::Instant::now();

        loop {
            // Check if process has exited
            match child.try_wait() {
                Ok(Some(status)) => {
                    info!("Indexer exited with status: {:?}", status);
                    if !status.success() {
                        anyhow::bail!("Indexer exited with error: {:?}", status);
                    }
                    break;
                }
                Ok(None) => {
                    // Process still running
                    if start.elapsed() > timeout {
                        info!("Timeout reached, killing indexer...");
                        child.kill().await?;
                        let _ = child.wait().await;
                        anyhow::bail!(
                            "Indexer did not complete within {} seconds",
                            timeout.as_secs()
                        );
                    }

                    // Print progress every 5 seconds
                    if start.elapsed().as_secs() % 5 == 0 {
                        let size = std::fs::metadata(db_path).map(|m| m.len()).unwrap_or(0);
                        debug!(
                            "Still indexing... (elapsed: {}s, DB size: {} bytes)",
                            start.elapsed().as_secs(),
                            size
                        );
                    }

                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                Err(e) => {
                    anyhow::bail!("Error checking indexer status: {}", e);
                }
            }
        }

        info!("Indexer completed successfully");

        Ok(())
    }

    /// Find an available port for the API server
    fn find_available_port() -> anyhow::Result<u16> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Ok(port)
    }

    /// Start the API server
    async fn start_api_server(db_path: &Path, port: u16) -> anyhow::Result<Child> {
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
}
