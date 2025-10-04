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

use alloy::{
    primitives::{utils::format_ether, Address},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::{contracts::IRewards, deployments::Deployment};
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};

/// Get the current reward delegate for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsGetDelegate {
    /// Address to check reward delegation for
    pub address: Address,

    /// Configuration for the ZKC deployment to use.
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsGetDelegate {
    /// Run the get-delegate command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let deployment = self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id))
            .context("could not determine ZKC deployment from chain ID; please specify deployment explicitly")?;

        // Create IRewards contract instance
        let rewards = IRewards::new(deployment.vezkc_address, &provider);

        // Query reward delegate and reward power
        let reward_delegate = rewards
            .rewardDelegates(self.address)
            .call()
            .await
            .context("Failed to query reward delegate")?;

        let reward_power = rewards
            .getStakingRewards(self.address)
            .call()
            .await
            .context("Failed to query reward power")?;

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        println!("\n{} [{}]", "Reward Delegation Status".bold(), network_name.blue().bold());
        println!("  Address: {}", format!("{:#x}", self.address).dimmed());

        // Check if self-delegated or delegated to another address
        if reward_delegate == self.address {
            println!("  Reward Delegate: {} {}", format!("{:#x}", reward_delegate).green(), "(self)".dimmed());
        } else {
            println!("  Reward Delegate: {}", format!("{:#x}", reward_delegate).yellow());
        }

        let reward_power_formatted = crate::format_amount(&format_ether(reward_power));
        println!("  Reward Power: {} {}", reward_power_formatted.cyan().bold(), "ZKC".cyan());

        Ok(())
    }
}
