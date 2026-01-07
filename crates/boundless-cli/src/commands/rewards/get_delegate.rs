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

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::{contracts::IRewards, deployments::Deployment};
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

/// Get the current reward delegate for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsGetDelegate {
    /// Address to check reward delegation for (defaults to configured staking address)
    #[clap(long)]
    pub address: Option<Address>,

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
        let rewards_config = self.rewards_config.load_and_validate()?;

        let address = self.address.or(rewards_config.staking_address).context(
            "No address provided.\n\n\
                To configure: run 'boundless rewards setup'\n\
                Or provide --address <ADDRESS>",
        )?;

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
            .context(
                "Could not determine ZKC deployment from chain ID; please specify deployment explicitly",
            )?;

        let rewards = IRewards::new(deployment.vezkc_address, &provider);

        let reward_delegate = rewards
            .rewardDelegates(address)
            .call()
            .await
            .context("Failed to query reward delegate")?;

        let reward_power = rewards
            .getStakingRewards(address)
            .call()
            .await
            .context("Failed to query reward power")?;

        let network_name = network_name_from_chain_id(Some(chain_id));
        let display = DisplayManager::with_network(network_name);

        display.header("Reward Delegation Status");

        if self.address.is_some() {
            display.address("Address", address);
        } else {
            display.item("Address", format!("{:#x} (from config)", address));
        }

        if reward_delegate == address {
            display.item("Reward Delegate", format!("{:#x} (self)", reward_delegate));
        } else {
            display.item_colored("Reward Delegate", format!("{:#x}", reward_delegate), "yellow");
        }

        display.balance("Reward Power", &format_eth(reward_power), "ZKC", "cyan");

        Ok(())
    }
}
