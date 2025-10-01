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

use anyhow::Result;
use clap::Args;

use crate::config::GlobalConfig;

/// Display configuration and environment variables
#[derive(Args, Clone, Debug)]
pub struct SetupConfig;

impl SetupConfig {
    /// Run the config command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        tracing::info!("=== Boundless CLI Configuration Status ===\n");

        // Check ETH_MAINNET_RPC_URL
        let eth_mainnet_configured = std::env::var("ETH_MAINNET_RPC_URL").is_ok();
        if eth_mainnet_configured {
            tracing::info!("✓ ETH_MAINNET_RPC_URL: Configured");
            // TODO: Verify it's actually chain ID 1
            tracing::info!("  → Rewards module: READY");
        } else {
            tracing::warn!("✗ ETH_MAINNET_RPC_URL: Not configured");
            tracing::warn!("  → Rewards module: NOT AVAILABLE");
        }

        // Check BOUNDLESS_RPC_URL
        let boundless_configured = std::env::var("BOUNDLESS_RPC_URL").is_ok() ||
                                    std::env::var("RPC_URL").is_ok();
        if boundless_configured {
            tracing::info!("✓ BOUNDLESS_RPC_URL: Configured");
            tracing::info!("  → Requestor module: READY");
            tracing::info!("  → Prover module: READY");
        } else {
            tracing::warn!("✗ BOUNDLESS_RPC_URL: Not configured");
            tracing::warn!("  → Requestor module: NOT AVAILABLE");
            tracing::warn!("  → Prover module: NOT AVAILABLE");
        }

        // Check PRIVATE_KEY
        let private_key_configured = std::env::var("PRIVATE_KEY").is_ok();
        if private_key_configured {
            tracing::info!("✓ PRIVATE_KEY: Configured");
        } else {
            tracing::warn!("✗ PRIVATE_KEY: Not configured (required for write operations)");
        }

        // Check PROVER_ADDRESS
        let prover_configured = std::env::var("PROVER_ADDRESS").is_ok();
        if prover_configured {
            tracing::info!("✓ PROVER_ADDRESS: Configured");
        } else {
            tracing::info!("○ PROVER_ADDRESS: Not configured (optional)");
        }

        // Check POVW work log configuration
        let work_log_configured = std::env::var("WORK_LOG_ID").is_ok() ||
                                   std::env::var("POVW_PRIVATE_KEY").is_ok();
        if work_log_configured {
            tracing::info!("✓ Work Log (WORK_LOG_ID/POVW_PRIVATE_KEY): Configured");
        } else {
            tracing::info!("○ Work Log configuration: Not configured (required for PoVW operations)");
        }

        // Check Indexer API
        let indexer_url = std::env::var("INDEXER_API_URL").ok();
        if let Some(url) = &indexer_url {
            tracing::info!("✓ INDEXER_API_URL: Configured (override)");
            tracing::info!("  → {}", url);
        } else if eth_mainnet_configured {
            // Show which default indexer will be used based on chain ID
            tracing::info!("○ INDEXER_API_URL: Using default based on chain");
            if let Ok(rpc_url) = std::env::var("ETH_MAINNET_RPC_URL") {
                // Try to detect chain and show which indexer will be used
                tracing::info!("  → Will auto-select indexer based on chain ID:");
                tracing::info!("    • Chain 1 (mainnet): https://api.boundless.market/");
                tracing::info!("    • Chain 11155111 (Sepolia): https://api-sepolia.boundless.market/");
            }
        } else {
            tracing::info!("○ INDEXER_API_URL: Not configured (optional, enables reward history)");
        }

        tracing::info!("\n=== Module Availability Summary ===");

        if boundless_configured && private_key_configured {
            tracing::info!("✓ Requestor: Fully operational");
            tracing::info!("✓ Prover: Fully operational");
        } else if boundless_configured {
            tracing::info!("⚠ Requestor: Read-only mode (no PRIVATE_KEY)");
            tracing::info!("⚠ Prover: Read-only mode (no PRIVATE_KEY)");
        } else {
            tracing::warn!("✗ Requestor: Not available");
            tracing::warn!("✗ Prover: Not available");
        }

        if eth_mainnet_configured {
            // Rewards always have history access (either override or auto-selected)
            tracing::info!("✓ Rewards: Fully operational with history");
            if private_key_configured || work_log_configured {
                tracing::info!("  → Can submit transactions");
            } else {
                tracing::info!("  → Read-only mode");
            }
        } else {
            tracing::warn!("✗ Rewards: Not available (ETH_MAINNET_RPC_URL required)");
        }

        Ok(())
    }
}