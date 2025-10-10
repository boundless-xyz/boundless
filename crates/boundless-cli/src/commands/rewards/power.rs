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
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::{contracts::{IRewards, IZKC}, deployments::Deployment};
use clap::Args;
use colored::Colorize;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    indexer_client::{parse_amount, IndexerClient},
};

/// Check reward power and earning potential
#[derive(Args, Clone, Debug)]
pub struct RewardsPower {
    /// Reward address to check power for (defaults to configured reward address)
    #[arg(long)]
    pub reward_address: Option<Address>,

    /// Configuration for the ZKC deployment to use
    #[clap(flatten, next_help_heading = "ZKC Deployment")]
    pub deployment: Option<Deployment>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsPower {
    /// Run the power command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Get address - from argument or from config reward address
        let address = if let Some(addr) = self.reward_address {
            addr
        } else {
            // Try to get reward address from config
            if let Ok(signer) = rewards_config.require_reward_private_key() {
                signer.address()
            } else {
                use crate::config_file::{Config, Secrets};
                let config = Config::load().context("Failed to load config")?;
                let network = config
                    .rewards
                    .as_ref()
                    .context("Rewards not configured")?
                    .network
                    .clone();

                let secrets = Secrets::load().context("Failed to load secrets")?;
                let rewards_secrets = secrets
                    .rewards_networks
                    .get(&network)
                    .context("No rewards secrets for current network")?;

                if let Some(ref addr_str) = rewards_secrets.reward_address {
                    addr_str.parse().context("Invalid reward address")?
                } else {
                    anyhow::bail!("No reward address configured. Configure a reward address or use --reward-address")
                }
            }
        };

        let rpc_url = rewards_config.require_rpc_url()?;

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        // Get chain ID
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context("Could not determine ZKC deployment from chain ID")?;

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Create contract instances
        alloy::sol! {
            #[sol(rpc)]
            interface IERC721Votes {
                function balanceOf(address owner) external view returns (uint256);
            }
        }

        let vezkc = IERC721Votes::new(deployment.vezkc_address, &provider);
        let rewards = IRewards::new(deployment.vezkc_address, &provider);

        // Query staked balance
        let staked_balance = vezkc
            .balanceOf(address)
            .call()
            .await
            .context("Failed to query staked balance")?;

        // Query reward power
        let reward_power = rewards
            .getStakingRewards(address)
            .call()
            .await
            .context("Failed to query reward power")?;

        // Query indexer for epoch data
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch staking metadata for last updated timestamp
        let metadata = client.get_staking_metadata().await.ok();

        let zkc = IZKC::new(deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        // Get current epoch from indexer (use epoch 0 as fallback to get latest)
        // We just need total staked, not specific epoch data
        let summary = client.get_epoch_staking(current_epoch.to::<u64>()).await.context(
            "Failed to fetch epoch data.",
        )?;

        let total_staked = parse_amount(&summary.total_staked)?;

        // Calculate share percentage
        let share_percentage = if total_staked > U256::ZERO {
            (reward_power * U256::from(1000000) / total_staked).to::<u64>() as f64 / 10000.0
        } else {
            100.0
        };

        // Get actual staking emissions from the contract for the current epoch
        let staking_emissions = zkc.getStakingEmissionsForEpoch(current_epoch)
            .call()
            .await
            .context("Failed to fetch staking emissions for current epoch")?;

        // Calculate estimates
        let your_estimated_rewards = if total_staked > U256::ZERO {
            staking_emissions * reward_power / total_staked
        } else {
            U256::ZERO
        };

        // Calculate PoVW cap
        let max_povw_per_epoch = reward_power / U256::from(15);

        // Format values
        let reward_power_formatted = crate::format_amount(&format_ether(reward_power));
        let total_staked_formatted = crate::format_amount(&format_ether(total_staked));
        let staking_emissions_formatted = crate::format_amount(&format_ether(staking_emissions));
        let estimated_rewards_formatted = crate::format_amount(&format_ether(your_estimated_rewards));
        let max_povw_formatted = crate::format_amount(&format_ether(max_povw_per_epoch));

        // Display results
        println!("\n{} [{}]", "Reward Power".bold(), network_name.blue().bold());

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            println!("  Data last updated: {}", formatted_time.dimmed());
        }

        println!("  Address: {}", format!("{:#x}", address).dimmed());
        println!();
        println!("  Your Reward Power:     {}", reward_power_formatted.yellow().bold());
        println!("  Share:                 {:.4}%", share_percentage);

        println!("\n{} (Epoch {})", "Estimated Staking Rewards".bold(), current_epoch);
        println!("  Total Emissions:       {} ZKC", staking_emissions_formatted.dimmed());
        println!("  Total Reward Power:    {} ZKC", total_staked_formatted.dimmed());
        println!("  Your Reward Power:     {} ZKC", reward_power_formatted.dimmed());
        println!("  × Your Share:          {:.4}%", share_percentage);
        println!("  ─────────────────────────────────");
        println!("  Est. Rewards:          ~{} {}", estimated_rewards_formatted.green(), "ZKC".green());

        println!("\n{}", "Maximum PoVW Rewards".bold());
        println!("  Max PoVW per Epoch:    {} {} {}", max_povw_formatted.yellow().bold(), "ZKC".yellow(), "(reward power / 15)".dimmed());

        Ok(())
    }
}
