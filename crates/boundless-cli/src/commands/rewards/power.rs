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

use crate::display::network_name_from_chain_id;
use alloy::{
    primitives::{Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::{
    contracts::{IRewards, IZKC},
    deployments::Deployment,
};
use clap::Args;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    config_ext::RewardsConfigExt,
    display::{format_eth, DisplayManager},
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
        let rewards_config = self.rewards_config.load_and_validate()?;

        let address = if let Some(addr) = self.reward_address {
            addr
        } else if let Ok(signer) = rewards_config.require_reward_key_with_help() {
            signer.address()
        } else {
            use crate::config_file::{Config, Secrets};
            let config = Config::load().context("Failed to load config")?;
            let network =
                config.rewards.as_ref().context("Rewards not configured")?.network.clone();

            let secrets = Secrets::load().context("Failed to load secrets")?;
            let rewards_secrets = secrets
                .rewards_networks
                .get(&network)
                .context("No rewards secrets for current network")?;

            if let Some(ref addr_str) = rewards_secrets.reward_address {
                addr_str.parse().context("Invalid reward address")?
            } else {
                anyhow::bail!(
                    "No reward address configured. Configure a reward address or use --reward-address"
                )
            }
        };

        let rpc_url = rewards_config.require_rpc_url_with_help()?;

        let provider = ProviderBuilder::new()
            .connect(&rpc_url)
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let deployment = self
            .deployment
            .clone()
            .or_else(|| Deployment::from_chain_id(chain_id))
            .context("Could not determine ZKC deployment from chain ID")?;

        let network_name = network_name_from_chain_id(Some(chain_id));

        alloy::sol! {
            #[sol(rpc)]
            interface IERC721Votes {
                function balanceOf(address owner) external view returns (uint256);
            }
        }

        let rewards = IRewards::new(deployment.vezkc_address, &provider);

        let reward_power = rewards
            .getStakingRewards(address)
            .call()
            .await
            .context("Failed to query reward power")?;

        let client = IndexerClient::new_from_chain_id(chain_id)?;
        let metadata = client.get_staking_metadata().await.ok();

        let zkc = IZKC::new(deployment.zkc_address, &provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;

        let summary = client
            .get_epoch_staking(current_epoch.to::<u64>())
            .await
            .context("Failed to fetch epoch data")?;

        let total_staked = parse_amount(&summary.total_staked)?;

        let share_percentage = if total_staked > U256::ZERO {
            (reward_power * U256::from(1000000) / total_staked).to::<u64>() as f64 / 10000.0
        } else {
            100.0
        };

        let staking_emissions = zkc
            .getStakingEmissionsForEpoch(current_epoch)
            .call()
            .await
            .context("Failed to fetch staking emissions for current epoch")?;

        let your_estimated_rewards = if total_staked > U256::ZERO {
            staking_emissions * reward_power / total_staked
        } else {
            U256::ZERO
        };

        let max_povw_per_epoch = reward_power / U256::from(15);

        let display = DisplayManager::with_network(network_name);
        display.header("Reward Power");

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("Data last updated: {}", formatted_time));
        }

        display.address("Address", address);
        println!();
        display.balance("Your Reward Power", &format_eth(reward_power), "", "yellow");
        display.item("Share", format!("{:.4}%", share_percentage));

        println!();
        display.item("", format!("Estimated Staking Rewards (Epoch {})", current_epoch));
        display.note(&format!("Total Emissions:    {} ZKC", format_eth(staking_emissions)));
        display.note(&format!("Total Reward Power: {} ZKC", format_eth(total_staked)));
        display.note(&format!("Your Reward Power:  {} ZKC", format_eth(reward_power)));
        display.note(&format!("× Your Share:       {:.4}%", share_percentage));
        println!("  ─────────────────────────────────");
        display.balance(
            "Est. Rewards",
            &format!("~{}", format_eth(your_estimated_rewards)),
            "ZKC",
            "green",
        );

        println!();
        display.item("", "Maximum PoVW Rewards");
        display.balance(
            "Max per Epoch",
            &format!("{} (reward power / 15)", format_eth(max_povw_per_epoch)),
            "ZKC",
            "yellow",
        );

        Ok(())
    }
}
