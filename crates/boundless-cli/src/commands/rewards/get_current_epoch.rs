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
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::indexer_client::IndexerClient;

/// Get current epoch information including timing, stakers, and miners
#[derive(Args, Clone, Debug)]
pub struct RewardsGetCurrentEpoch {
    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsGetCurrentEpoch {
    /// Run the get-current-epoch command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Create indexer client
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Get current epoch info
        let epoch_info = indexer
            .get_current_epoch_info()
            .await
            .context("Failed to query current epoch info")?;

        println!("\n{} [{}]", "Current Epoch".bold(), network_name.blue().bold());
        println!("  Epoch:  {}", epoch_info.epoch_number.to_string().cyan().bold());

        // Convert timestamps to human-readable dates
        let start_time = DateTime::<Utc>::from_timestamp(epoch_info.start_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        let end_time = DateTime::<Utc>::from_timestamp(epoch_info.end_timestamp as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "Invalid timestamp".to_string());

        println!("  Start:  {}", start_time.dimmed());
        println!("  End:    {}", end_time.dimmed());

        // Calculate and display time remaining
        let now = chrono::Utc::now().timestamp() as u64;
        if now < epoch_info.end_timestamp {
            let remaining = epoch_info.end_timestamp - now;
            let days = remaining / 86400;
            let hours = (remaining % 86400) / 3600;
            let minutes = (remaining % 3600) / 60;

            if days > 0 {
                println!(
                    "  Time Remaining: {} days, {} hours, {} minutes",
                    days.to_string().green(),
                    hours,
                    minutes
                );
            } else if hours > 0 {
                println!(
                    "  Time Remaining: {} hours, {} minutes",
                    hours.to_string().yellow(),
                    minutes
                );
            } else {
                println!("  Time Remaining: {} minutes", minutes.to_string().red());
            }
        } else {
            println!("  {}", "Epoch has ended, awaiting finalization".yellow());
        }

        // Fetch staking and PoVW data
        let staking_data = indexer.get_current_epoch_staking(epoch_info.epoch_number).await.ok();
        let povw_data = indexer.get_current_epoch_povw(epoch_info.epoch_number).await.ok();

        println!();
        if let Some(ref staking) = staking_data {
            if let Some(ref summary) = staking.summary {
                println!("  Stakers: {}", summary.num_stakers.to_string().cyan().bold());
            }
        }

        if let Some(ref povw) = povw_data {
            if let Some(ref summary) = povw.summary {
                println!("  Miners:  {}", summary.num_participants.to_string().yellow().bold());
            }
        }

        Ok(())
    }
}
