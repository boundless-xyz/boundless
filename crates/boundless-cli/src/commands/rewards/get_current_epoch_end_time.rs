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
use clap::Args;
use chrono::{DateTime, Utc};

use crate::config::GlobalConfig;
use crate::indexer_client::IndexerClient;

/// Get the end time of the current PoVW epoch
#[derive(Args, Clone, Debug)]
pub struct RewardsGetCurrentEpochEndTime;

impl RewardsGetCurrentEpochEndTime {
    /// Run the get-current-epoch-end-time command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Get RPC URL and chain ID
        let rpc_url = global_config.require_rpc_url()
            .context("ETH_MAINNET_RPC_URL is required for rewards commands")?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        let chain_id = provider.get_chain_id().await
            .context("Failed to get chain ID")?;

        if chain_id != 1 {
            bail!("Rewards commands require connection to Ethereum mainnet (chain ID 1), got chain ID {}", chain_id);
        }

        // Create indexer client based on chain ID
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Query current epoch info from indexer
        let epoch_info = indexer.get_current_epoch_info().await
            .context("Failed to query current epoch info from indexer")?;

        // Convert end timestamp to human-readable date
        let end_time = DateTime::<Utc>::from_timestamp(epoch_info.end_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        tracing::info!("Current epoch (#{}) ends at: {}", epoch_info.epoch_number, end_time);

        // Calculate and display time remaining
        let now = chrono::Utc::now().timestamp() as u64;
        if now < epoch_info.end_timestamp {
            let remaining = epoch_info.end_timestamp - now;
            let days = remaining / 86400;
            let hours = (remaining % 86400) / 3600;
            let minutes = (remaining % 3600) / 60;
            let seconds = remaining % 60;

            if days > 0 {
                tracing::info!("Time remaining: {} days, {} hours, {} minutes", days, hours, minutes);
            } else if hours > 0 {
                tracing::info!("Time remaining: {} hours, {} minutes, {} seconds", hours, minutes, seconds);
            } else if minutes > 0 {
                tracing::info!("Time remaining: {} minutes, {} seconds", minutes, seconds);
            } else {
                tracing::info!("Time remaining: {} seconds", seconds);
            }
        } else {
            tracing::info!("Note: Current epoch has ended and is awaiting finalization");
        }

        // Display epoch status
        tracing::info!("Epoch status: {}", epoch_info.status);

        Ok(())
    }
}
