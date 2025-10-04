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
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::indexer_client::IndexerClient;

/// Claim available PoVW rewards for a prover address
#[derive(Args, Clone, Debug)]
pub struct RewardsClaimPovwRewards {
    /// Prover address to claim rewards for (defaults to wallet address)
    pub prover_address: Option<Address>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsClaimPovwRewards {
    /// Run the claim-povw-rewards command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        // Get prover address (use provided address or wallet address)
        let prover_address = if let Some(addr) = self.prover_address {
            addr
        } else {
            std::env::var("PROVER_ADDRESS")
                .context("No prover address provided and PROVER_ADDRESS environment variable not set")?
                .parse()
                .context("Failed to parse PROVER_ADDRESS")?
        };

        // Create indexer client based on chain ID
        let indexer = IndexerClient::new_from_chain_id(chain_id)?;

        // Query claimable rewards from indexer
        tracing::info!("Checking claimable PoVW rewards for prover {:#x}...", prover_address);

        let claimable_rewards = indexer
            .get_claimable_povw_rewards(prover_address)
            .await
            .context("Failed to query claimable PoVW rewards from indexer")?;

        if claimable_rewards.claimable_amount == 0 {
            tracing::info!("No claimable PoVW rewards available for {:#x}", prover_address);
            return Ok(());
        }

        // Display claimable amount
        tracing::info!(
            "Claimable PoVW rewards: {} ZKC",
            format_ether(alloy::primitives::U256::from(claimable_rewards.claimable_amount))
        );

        // TODO: Implement actual claiming transaction once PoVW contracts are deployed
        // This would involve:
        // 1. Getting claim proof from indexer
        // 2. Calling the PoVW mint contract with the proof
        // 3. Waiting for transaction confirmation

        tracing::warn!(
            "Note: Claiming functionality not yet implemented - PoVW contracts pending deployment"
        );
        tracing::info!(
            "For now, rewards accumulate and will be claimable once contracts are deployed"
        );

        Ok(())
    }
}
