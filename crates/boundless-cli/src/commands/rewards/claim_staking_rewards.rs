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
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Context, Result};
use clap::Args;
use boundless_zkc::contracts::IStakingRewards;

use crate::config::GlobalConfig;

/// Claim accumulated staking rewards
#[derive(Args, Clone, Debug)]
pub struct RewardsClaimStakingRewards {
    /// Address to claim rewards for (defaults to wallet address)
    pub address: Option<Address>,
}

impl RewardsClaimStakingRewards {
    /// Run the claim-staking-rewards command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Get RPC URL and private key for signing
        let rpc_url = global_config.require_rpc_url()
            .context("ETH_MAINNET_RPC_URL is required for rewards commands")?;

        let private_key = global_config.require_prover_private_key()
            .context("PROVER_PRIVATE_KEY is required for claiming rewards")?;

        // Create signer from private key
        let signer: PrivateKeySigner = private_key.parse()
            .context("Failed to parse private key")?;

        // Connect to provider with signer
        let provider = ProviderBuilder::new()
            .wallet(signer.clone())
            .on_http(rpc_url);

        // Verify we're on mainnet (chain ID 1)
        let chain_id = provider.get_chain_id().await
            .context("Failed to get chain ID")?;

        if chain_id != 1 {
            bail!("Rewards commands require connection to Ethereum mainnet (chain ID 1), got chain ID {}", chain_id);
        }

        // Get staking rewards contract address
        let staking_rewards_address = global_config.staking_rewards_address()
            .context("STAKING_REWARDS_ADDRESS environment variable is required")?;

        // Create staking rewards contract instance
        let staking_rewards = IStakingRewards::new(staking_rewards_address, &provider);

        // Get address to claim for (defaults to signer address)
        let claim_address = self.address.unwrap_or(signer.address());

        // Get current epoch to check for claimable rewards
        let current_epoch = staking_rewards.getCurrentEpoch().call().await
            .context("Failed to query current epoch")?;

        // Check unclaimed rewards for recent epochs (last 10 epochs)
        let epochs_to_check: Vec<alloy::primitives::U256> = (0..10)
            .map(|i| {
                if current_epoch > alloy::primitives::U256::from(i) {
                    current_epoch - alloy::primitives::U256::from(i)
                } else {
                    alloy::primitives::U256::ZERO
                }
            })
            .filter(|&e| e > alloy::primitives::U256::ZERO)
            .collect();

        let unclaimed = staking_rewards.calculateUnclaimedRewards(claim_address, epochs_to_check.clone()).call().await
            .context("Failed to query unclaimed staking rewards")?;

        let total_unclaimed: alloy::primitives::U256 = unclaimed.iter().sum();

        if total_unclaimed == alloy::primitives::U256::ZERO {
            tracing::info!("No staking rewards available to claim for {:#x}", claim_address);
            return Ok(());
        }

        tracing::info!(
            "Claimable staking rewards for {:#x}: {} ZKC",
            claim_address,
            format_ether(total_unclaimed)
        );

        // Execute claim transaction
        tracing::info!("Claiming staking rewards...");

        // Claim rewards for the epochs with unclaimed rewards
        let epochs_to_claim: Vec<alloy::primitives::U256> = epochs_to_check.into_iter()
            .zip(unclaimed.iter())
            .filter(|(_, reward)| **reward > alloy::primitives::U256::ZERO)
            .map(|(epoch, _)| epoch)
            .collect();

        let tx = staking_rewards.claimRewards(epochs_to_claim)
            .send()
            .await
            .context("Failed to send claim transaction")?;

        tracing::info!("Transaction sent: {:#x}", tx.tx_hash());
        tracing::info!("Waiting for confirmation...");

        // Wait for transaction confirmation
        let tx_hash = tx.watch()
            .await
            .context("Failed to wait for transaction confirmation")?;

        tracing::info!("Successfully claimed staking rewards!");
        tracing::info!("Transaction: {:#x}", tx_hash);
        tracing::info!("Rewards have been successfully claimed");

        Ok(())
    }
}
