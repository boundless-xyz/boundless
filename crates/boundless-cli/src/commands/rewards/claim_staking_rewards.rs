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
use boundless_zkc::contracts::IStakingRewards;
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};

/// Claim accumulated staking rewards
#[derive(Args, Clone, Debug)]
pub struct RewardsClaimStakingRewards {
    /// Address to receive the claimed rewards (defaults to signer address)
    #[arg(long)]
    pub recipient: Option<Address>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsClaimStakingRewards {
    /// Run the claim-staking-rewards command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Get RPC URL and private key for signing
        let rpc_url = rewards_config.require_rpc_url()?;

        let signer = rewards_config.require_reward_private_key()?;

        // Connect to provider with signer
        let provider = ProviderBuilder::new()
            .wallet(signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Get staking rewards contract address
        let staking_rewards_address = rewards_config.staking_rewards_address()?;

        // Create staking rewards contract instance
        let staking_rewards = IStakingRewards::new(staking_rewards_address, &provider);

        // Always use signer address as claim address
        let claim_address = signer.address();

        // Get recipient address (defaults to claim address)
        let recipient_address = self.recipient.unwrap_or(claim_address);

        println!(
            "\n{} [{}]",
            "Claiming Staking Rewards".bold(),
            network_name.blue().bold()
        );
        println!("  Reward Address: {}", format!("{:#x}", claim_address).cyan());
        if self.recipient.is_some() {
            println!("  Recipient Address: {}", format!("{:#x}", recipient_address).cyan());
        }

        // Get current epoch to check for claimable rewards
        println!("  {} Checking for claimable rewards...", "→".dimmed());
        let current_epoch = staking_rewards
            .getCurrentEpoch()
            .call()
            .await
            .context("Failed to query current epoch")?;

        // Check all epochs from 0 to current
        let epochs_to_check: Vec<alloy::primitives::U256> = (0..=current_epoch.to::<u64>())
            .map(alloy::primitives::U256::from)
            .collect();

        let unclaimed = staking_rewards
            .calculateUnclaimedRewards(claim_address, epochs_to_check.clone())
            .call()
            .await
            .context("Failed to query unclaimed staking rewards")?;

        let total_unclaimed: alloy::primitives::U256 = unclaimed.iter().sum();

        if total_unclaimed == alloy::primitives::U256::ZERO {
            println!("\n{} No staking rewards available to claim", "ℹ".blue().bold());
            return Ok(());
        }

        let formatted_amount = crate::format_amount(&format_ether(total_unclaimed));
        println!("  Claimable: {} {}", formatted_amount.green().bold(), "ZKC".green());

        // Claim rewards for the epochs with unclaimed rewards
        let epochs_to_claim: Vec<alloy::primitives::U256> = epochs_to_check
            .into_iter()
            .zip(unclaimed.iter())
            .filter(|(_, reward)| **reward > alloy::primitives::U256::ZERO)
            .map(|(epoch, _)| epoch)
            .collect();

        println!("  {} Submitting claim transaction...", "→".dimmed());

        // Use different claim method based on whether recipient is specified
        let tx = if self.recipient.is_some() {
            staking_rewards
                .claimRewardsToRecipient(epochs_to_claim, recipient_address)
                .send()
                .await
                .context("Failed to send claim transaction")?
        } else {
            staking_rewards
                .claimRewards(epochs_to_claim)
                .send()
                .await
                .context("Failed to send claim transaction")?
        };

        println!("  {} Waiting for confirmation...", "→".dimmed());
        let tx_hash = tx.watch().await.context("Failed to wait for transaction confirmation")?;

        println!(
            "\n{} Successfully claimed {} {}",
            "✓".green().bold(),
            formatted_amount.green().bold(),
            "ZKC".green()
        );
        println!("  Transaction: {}", format!("{:#x}", tx_hash).dimmed());
        println!("  Recipient:   {}", format!("{:#x}", recipient_address).green());

        Ok(())
    }
}
