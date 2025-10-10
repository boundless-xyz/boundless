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
    primitives::Address,
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::contracts::IRewards;
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};

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
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let signer = rewards_config.require_staking_private_key()?;

        // Connect to provider with signer
        let provider = ProviderBuilder::new().wallet(signer.clone()).on_http(rpc_url);

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        // Get veZKC (staking) contract address
        let vezkc_address = rewards_config.vezkc_address()?;

        // Create rewards contract instance
        let rewards = IRewards::new(vezkc_address, &provider);

        // Check current reward delegation
        let current_delegate = rewards
            .rewardDelegates(signer.address())
            .call()
            .await
            .context("Failed to query current reward delegate")?;

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        println!("\n{} [{}]", "Delegating Reward Power".bold(), network_name.blue().bold());

        let from_label = if Some(signer.address()) == rewards_config.staking_address {
            format!("{} {}", format!("{:#x}", signer.address()).dimmed(), "(Staking Address)".dimmed())
        } else {
            format!("{:#x}", signer.address()).dimmed().to_string()
        };

        println!("  From:          {}", from_label);
        println!("  Current:       {}", format!("{:#x}", current_delegate).cyan());

        if current_delegate == self.delegatee {
            println!("  Status:        {} {}", "Already delegated".green().bold(), "(no action needed)".dimmed());
            println!("\n{} {}", "✓".green(), "Reward power is already delegated to this address".green());
            return Ok(());
        }

        let delegatee_label = if Some(self.delegatee) == rewards_config.reward_address {
            format!("{} {}", format!("{:#x}", self.delegatee).yellow(), "(Reward Address)".dimmed())
        } else {
            format!("{:#x}", self.delegatee).yellow().to_string()
        };

        println!("  Delegating to: {}", delegatee_label);

        // Execute delegation transaction
        let tx = rewards
            .delegateRewards(self.delegatee)
            .send()
            .await
            .context("Failed to send reward delegation transaction")?;

        println!("\n  Transaction:   {}", format!("{:#x}", tx.tx_hash()).dimmed());
        println!("  Status:        {}", "Waiting for confirmation...".yellow());

        // Wait for transaction confirmation
        let tx_hash = tx.watch().await.context("Failed to wait for transaction confirmation")?;

        println!("\n{} {}", "✓".green().bold(), "Successfully delegated!".green().bold());
        println!("  New Delegate:  {}", format!("{:#x}", self.delegatee).cyan());
        println!("  Transaction:   {}", format!("{:#x}", tx_hash).cyan());

        Ok(())
    }
}
