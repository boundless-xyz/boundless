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
use boundless_zkc::contracts::IRewards;
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Delegate reward power to another address
#[derive(Args, Clone, Debug)]
pub struct RewardsDelegate {
    /// Address to delegate reward power to
    pub delegatee: Address,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsDelegate {
    /// Run the delegate command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.load_and_validate()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;
        let signer = rewards_config.require_staking_key_with_help()?;

        let provider = ProviderBuilder::new()
            .wallet(signer.clone())
            .connect_http(rpc_url.parse().context("Invalid RPC URL")?);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let vezkc_address = rewards_config.vezkc_address()?;
        let rewards = IRewards::new(vezkc_address, &provider);

        let current_delegate = rewards
            .rewardDelegates(signer.address())
            .call()
            .await
            .context("Failed to query current reward delegate")?;

        let network_name = network_name_from_chain_id(Some(chain_id));
        let display = DisplayManager::with_network(network_name);

        display.header("Delegating Reward Power");

        let from_label = if Some(signer.address()) == rewards_config.staking_address {
            format!("{:#x} (Staking Address)", signer.address())
        } else {
            format!("{:#x}", signer.address())
        };

        display.item("From", from_label);
        display.item_colored("Current", format!("{:#x}", current_delegate), "cyan");

        if current_delegate == self.delegatee {
            display.status("Status", "Already delegated (no action needed)", "green");
            display.success("Reward power is already delegated to this address");
            return Ok(());
        }

        let delegatee_label = if Some(self.delegatee) == rewards_config.reward_address {
            format!("{:#x} (Reward Address)", self.delegatee)
        } else {
            format!("{:#x}", self.delegatee)
        };

        display.item_colored("Delegating to", delegatee_label, "yellow");

        let tx = rewards
            .delegateRewards(self.delegatee)
            .send()
            .await
            .context("Failed to send reward delegation transaction")?;

        display.tx_hash(*tx.tx_hash());
        display.status("Status", "Waiting for confirmation...", "yellow");

        let tx_hash = tx.watch().await.context("Failed to wait for transaction confirmation")?;

        display.success("Successfully delegated!");
        display.item_colored("New Delegate", format!("{:#x}", self.delegatee), "cyan");
        display.item_colored("Transaction", format!("{:#x}", tx_hash), "cyan");

        Ok(())
    }
}
