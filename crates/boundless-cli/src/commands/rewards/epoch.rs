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
use boundless_zkc::{contracts::IZKC, deployments::Deployment};
use chrono::{DateTime, Utc};
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::indexer_client::IndexerClient;

/// Get information about the current epoch
#[derive(Args, Clone, Debug)]
pub struct RewardsEpoch {
    /// Configuration for the ZKC deployment to use
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsEpoch {
    /// Run the epoch command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Get deployment
        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context("Could not determine ZKC deployment from chain ID")?;

        // Get current epoch from on-chain ZKC contract
        let zkc = IZKC::new(deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let epoch_number = current_epoch.to::<u64>();

        // Create indexer client
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch staking and PoVW data from indexer
        let staking_result = indexer.get_epoch_staking(epoch_number).await;
        let povw_result = indexer.get_epoch_povw(epoch_number).await;

        println!("\n{} [{}]", "Epoch Details".bold(), network_name.blue().bold());
        println!("Current Epoch: {}", epoch_number.to_string().cyan().bold());

        // Handle errors gracefully
        let staking_data = match staking_result {
            Ok(data) => Some(data),
            Err(e) => {
                println!("{} Failed to fetch staking data: {}", "⚠".yellow(), e);
                None
            }
        };

        let povw_data = match povw_result {
            Ok(data) => Some(data),
            Err(e) => {
                println!("{} Failed to fetch PoVW data: {}", "⚠".yellow(), e);
                None
            }
        };

        // Use epoch timestamps from PoVW or staking data
        if let Some(ref povw) = povw_data {
            let start_time = DateTime::<Utc>::from_timestamp(povw.epoch_start_time as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "Invalid timestamp".to_string());

            let end_time = DateTime::<Utc>::from_timestamp(povw.epoch_end_time as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "Invalid timestamp".to_string());

            println!("Epoch Start: {}", start_time.dimmed());
            println!("Epoch End: {}", end_time.dimmed());

            // Calculate and display time remaining
            let now = chrono::Utc::now().timestamp() as u64;
            if now < povw.epoch_end_time {
                let remaining = povw.epoch_end_time - now;
                let days = remaining / 86400;
                let hours = (remaining % 86400) / 3600;
                let minutes = (remaining % 3600) / 60;

                println!(
                    "Time Remaining: {} days, {} hours, {} minutes",
                    days,
                    hours,
                    minutes
                );
            } else {
                println!("Time Remaining: {}", "Epoch has ended".yellow());
            }
        }

        if let Some(ref povw) = povw_data {
            println!("Number of Miners: {}", povw.num_participants.to_string().cyan());
            println!(
                "Amount of Submitted Work: {}",
                povw.total_work_formatted.yellow()
            );
        }

        if let Some(ref staking) = staking_data {
            println!("Number of Stakers: {}", staking.num_stakers.to_string().cyan());
            println!(
                "Amount of Staked ZKC: {}",
                staking.total_staked_formatted.green()
            );
        }

        Ok(())
    }
}
