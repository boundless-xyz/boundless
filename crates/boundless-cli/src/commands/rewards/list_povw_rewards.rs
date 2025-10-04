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

use crate::{
    config::{GlobalConfig, RewardsConfig},
    indexer_client::{parse_amount, IndexerClient},
};

/// List historical PoVW rewards for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsListPovwRewards {
    /// Work log ID (address) to query rewards for
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

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsListPovwRewards {
    /// Run the list-povw-rewards command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch PoVW history
        let history = client.get_povw_history(self.address).await?;

        if history.entries.is_empty() {
            tracing::info!("No PoVW history found for work log ID {}", self.address);
            return Ok(());
        }

        tracing::info!("=== PoVW Rewards History for {} ===", self.address);

        if let Some(summary) = &history.summary {
            let total_work = parse_amount(&summary.total_work_submitted)?;
            let total_actual = parse_amount(&summary.total_actual_rewards)?;
            let total_uncapped = parse_amount(&summary.total_uncapped_rewards)?;

            tracing::info!("\n=== Lifetime Summary ===");
            tracing::info!("Total Work Submitted: {}", format_ether(total_work));
            tracing::info!("Total Actual Rewards: {} ZKC", format_ether(total_actual));
            tracing::info!("Total Uncapped Rewards: {} ZKC", format_ether(total_uncapped));
            tracing::info!("Epochs Participated: {}", summary.epochs_participated);

            // Check if rewards were capped
            if total_actual < total_uncapped {
                let capped_amount = total_uncapped - total_actual;
                tracing::warn!(
                    "⚠️  Rewards were capped by {} ZKC due to staking limits",
                    format_ether(capped_amount)
                );
            }
        }

        tracing::info!("\n=== Epoch History ===");
        for entry in &history.entries {
            let work = parse_amount(&entry.work_submitted)?;
            let actual = parse_amount(&entry.actual_rewards)?;
            let uncapped = parse_amount(&entry.uncapped_rewards)?;

            tracing::info!(
                "Epoch {}: Work={}, Rewards={} ZKC ({:.1}% of work){}",
                entry.epoch,
                format_ether(work),
                format_ether(actual),
                entry.percentage,
                if entry.is_capped { " [CAPPED]" } else { "" }
            );

            if entry.is_capped {
                let cap = parse_amount(&entry.reward_cap)?;
                let staked = parse_amount(&entry.staked_amount)?;
                tracing::info!(
                    "  → Capped at {} ZKC (staked: {} ZKC)",
                    format_ether(cap),
                    format_ether(staked)
                );
                tracing::info!("  → Uncapped would have been: {} ZKC", format_ether(uncapped));
            }
        }

        if self.dry_run {
            tracing::info!("\n=== Current Epoch Estimate (DRY RUN) ===");
            tracing::warn!("⚠️  These are estimates only and actual rewards may vary");

            // Get current epoch data
            // TODO: Query actual current epoch from contract
            let current_epoch = 5; // Placeholder

            match client.get_epoch_povw(current_epoch).await {
                Ok(epoch_summary) => {
                    let total_work = parse_amount(&epoch_summary.total_work)?;
                    let total_emissions = parse_amount(&epoch_summary.total_emissions)?;

                    tracing::info!("Current Epoch {}: ", current_epoch);
                    tracing::info!("  Total Work Submitted: {}", format_ether(total_work));
                    tracing::info!("  Total Emissions: {} ZKC", format_ether(total_emissions));
                    tracing::info!("  Participants: {}", epoch_summary.num_participants);

                    tracing::info!("\n💡 To maximize rewards:");
                    tracing::info!("  - Submit work early in the epoch");
                    tracing::info!("  - Ensure adequate ZKC staking to avoid caps");
                    tracing::info!("  - Monitor your reward cap: 2.5x your staked amount");
                }
                Err(e) => {
                    tracing::warn!("Unable to fetch current epoch data: {}", e);
                }
            }
        }

        Ok(())
    }
}
