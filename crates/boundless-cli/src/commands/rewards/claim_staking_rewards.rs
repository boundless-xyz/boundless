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
use boundless_zkc::contracts::IStakingRewards;
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

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
        let rewards_config = self.rewards_config.load_and_validate()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;
        let signer = rewards_config.require_reward_key_with_help()?;

        let provider = ProviderBuilder::new()
            .wallet(signer.clone())
            .connect(&rpc_url)
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let network_name = network_name_from_chain_id(Some(chain_id));
        let display = DisplayManager::with_network(network_name);

        let staking_rewards_address = rewards_config.staking_rewards_address()?;
        let staking_rewards = IStakingRewards::new(staking_rewards_address, &provider);

        let claim_address = signer.address();
        let recipient_address = self.recipient.unwrap_or(claim_address);

        display.header("Claiming Staking Rewards");
        display.address("Reward Address", claim_address);
        if self.recipient.is_some() {
            display.address("Recipient Address", recipient_address);
        }

        display.info("Checking for claimable rewards...");

        let current_epoch = staking_rewards
            .getCurrentEpoch()
            .call()
            .await
            .context("Failed to query current epoch")?;

        let epochs_to_check: Vec<alloy::primitives::U256> =
            (0..=current_epoch.to::<u64>()).map(alloy::primitives::U256::from).collect();

        let unclaimed = staking_rewards
            .calculateUnclaimedRewards(claim_address, epochs_to_check.clone())
            .call()
            .await
            .context("Failed to query unclaimed staking rewards")?;

        let total_unclaimed: alloy::primitives::U256 = unclaimed.iter().sum();

        if total_unclaimed == alloy::primitives::U256::ZERO {
            display.info("No staking rewards available to claim");
            return Ok(());
        }

        display.balance("Claimable", &format_eth(total_unclaimed), "ZKC", "green");

        let epochs_to_claim: Vec<alloy::primitives::U256> = epochs_to_check
            .into_iter()
            .zip(unclaimed.iter())
            .filter(|(_, reward)| **reward > alloy::primitives::U256::ZERO)
            .map(|(epoch, _)| epoch)
            .collect();

        display.info("Submitting claim transaction...");

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

        display.info("Waiting for confirmation...");
        let tx_hash = tx.watch().await.context("Failed to wait for transaction confirmation")?;

        display.success(&format!("Successfully claimed {} ZKC", format_eth(total_unclaimed)));
        display.tx_hash(tx_hash);
        display.item_colored("Recipient", format!("{:#x}", recipient_address), "green");

        Ok(())
    }
}
