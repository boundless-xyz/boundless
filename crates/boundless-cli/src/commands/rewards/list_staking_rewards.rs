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
use clap::Args;

use crate::{config::GlobalConfig, indexer_client::IndexerClient};

/// List historical staking rewards for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsListStakingRewards {
    /// Address to query rewards for
    pub address: Address,

    /// Start epoch (optional)
    #[clap(long)]
    pub start_epoch: Option<u64>,

    /// End epoch (optional)
    #[clap(long)]
    pub end_epoch: Option<u64>,

    /// Show estimated rewards for current epoch (dry-run)
    #[clap(long)]
    pub dry_run: bool,
}

impl RewardsListStakingRewards {
    /// Run the list-staking-rewards command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Get chain ID from ETH_MAINNET_RPC_URL to determine indexer endpoint
        let rpc_url = global_config.require_rpc_url()?;
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch staking history
        let history = client.get_staking_history(self.address).await?;

        if history.entries.is_empty() {
            tracing::info!("No staking history found for address {}", self.address);
            return Ok(());
        }

        tracing::info!("=== Staking Rewards History for {} ===", self.address);

        if let Some(summary) = &history.summary {
            tracing::info!("Total Staked: {} ZKC", summary.total_staked);
            tracing::info!("Epochs Participated: {}", summary.epochs_participated);
            if summary.is_withdrawing {
                tracing::warn!("⚠️  Currently withdrawing");
            }
        }

        tracing::info!("\n=== Epoch History ===");
        for entry in &history.entries {
            tracing::info!(
                "Epoch {}: {} ZKC staked{}",
                entry.epoch,
                entry.staked_amount,
                if entry.is_withdrawing { " (withdrawing)" } else { "" }
            );
        }

        if self.dry_run {
            tracing::info!("\n=== Current Epoch Estimate (DRY RUN) ===");
            tracing::warn!("⚠️  These are estimates only and actual rewards may vary");
            // TODO: Fetch current epoch data and estimate rewards
        }

        Ok(())
    }
}
