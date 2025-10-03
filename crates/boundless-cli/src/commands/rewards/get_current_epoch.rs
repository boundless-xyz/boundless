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

use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::indexer_client::IndexerClient;

/// Get current PoVW epoch information
#[derive(Args, Clone, Debug)]
pub struct RewardsGetCurrentEpoch {
    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsGetCurrentEpoch {
    /// Run the get-current-epoch command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        if chain_id != 1 {
            bail!("Rewards commands require connection to Ethereum mainnet (chain ID 1), got chain ID {}", chain_id);
        }

        // Create indexer client based on chain ID
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Query current epoch info from indexer
        let epoch_info = indexer
            .get_current_epoch_info()
            .await
            .context("Failed to query current epoch info from indexer")?;

        // Display epoch information
        tracing::info!("Current PoVW Epoch Information:");
        tracing::info!("  Epoch number: {}", epoch_info.epoch_number);
        tracing::info!("  Status: {}", epoch_info.status);

        // Convert timestamps to human-readable dates
        let start_time = DateTime::<Utc>::from_timestamp(epoch_info.start_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        let end_time = DateTime::<Utc>::from_timestamp(epoch_info.end_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        tracing::info!("  Start time: {}", start_time);
        tracing::info!("  End time: {}", end_time);

        // Display progress if epoch is active
        if epoch_info.status == "active" {
            let now = chrono::Utc::now().timestamp() as u64;
            if now >= epoch_info.start_timestamp && now <= epoch_info.end_timestamp {
                let duration = epoch_info.end_timestamp - epoch_info.start_timestamp;
                let elapsed = now - epoch_info.start_timestamp;
                let progress = (elapsed as f64 / duration as f64 * 100.0).min(100.0);
                tracing::info!("  Progress: {:.1}%", progress);

                let remaining = epoch_info.end_timestamp - now;
                let days = remaining / 86400;
                let hours = (remaining % 86400) / 3600;
                let minutes = (remaining % 3600) / 60;
                tracing::info!(
                    "  Time remaining: {} days, {} hours, {} minutes",
                    days,
                    hours,
                    minutes
                );
            }
        }

        // Display additional info if available
        if let Some(total_work) = epoch_info.total_work {
            tracing::info!("  Total work submitted: {}", total_work);
        }

        if let Some(participants) = epoch_info.participants_count {
            tracing::info!("  Active participants: {}", participants);
        }

        Ok(())
    }
}
