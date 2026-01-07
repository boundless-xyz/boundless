// Copyright 2026 Boundless Foundation, Inc.
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

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};
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
        let rewards_config = self.rewards_config.load_and_validate()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;

        let provider = ProviderBuilder::new()
            .connect(&rpc_url)
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = network_name_from_chain_id(Some(chain_id));

        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context("Could not determine ZKC deployment from chain ID")?;

        let zkc = IZKC::new(deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let epoch_number = current_epoch.to::<u64>();

        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        let staking_metadata = indexer.get_staking_metadata().await.ok();
        let povw_metadata = indexer.get_povw_metadata().await.ok();

        let staking_result = indexer.get_epoch_staking(epoch_number).await;
        let povw_result = indexer.get_epoch_povw(epoch_number).await;

        let display = DisplayManager::with_network(network_name);
        display.header("Epoch Details");

        if let Some(ref meta) = staking_metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("Staking data last updated: {}", formatted_time));
        }

        if let Some(ref meta) = povw_metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("PoVW data last updated: {}", formatted_time));
        }

        display.item_colored("Current Epoch", epoch_number, "cyan");

        let staking_data = match staking_result {
            Ok(data) => Some(data),
            Err(e) => {
                display.warning(&format!("Failed to fetch staking data: {}", e));
                None
            }
        };

        let povw_data = match povw_result {
            Ok(data) => Some(data),
            Err(e) => {
                display.warning(&format!("Failed to fetch PoVW data: {}", e));
                None
            }
        };

        if let Some(ref povw) = povw_data {
            let start_time = DateTime::<Utc>::from_timestamp(povw.epoch_start_time as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "Invalid timestamp".to_string());

            let end_time = DateTime::<Utc>::from_timestamp(povw.epoch_end_time as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "Invalid timestamp".to_string());

            display.item("Epoch Start", start_time);
            display.item("Epoch End", end_time);

            let now = chrono::Utc::now().timestamp() as u64;
            if now < povw.epoch_end_time {
                let remaining = povw.epoch_end_time - now;
                let days = remaining / 86400;
                let hours = (remaining % 86400) / 3600;
                let minutes = (remaining % 3600) / 60;

                display.item(
                    "Time Remaining",
                    format!("{} days, {} hours, {} minutes", days, hours, minutes),
                );
            } else {
                display.item_colored("Time Remaining", "Epoch has ended", "yellow");
            }
        }

        if let Some(ref povw) = povw_data {
            display.item_colored("Number of Miners", povw.num_participants, "cyan");
            display.item_colored("Submitted Work", &povw.total_work_formatted, "yellow");
        }

        if let Some(ref staking) = staking_data {
            display.item_colored("Number of Stakers", staking.num_stakers, "cyan");
            display.item_colored("Staked ZKC", &staking.total_staking_power_formatted, "green");
        }

        Ok(())
    }
}
