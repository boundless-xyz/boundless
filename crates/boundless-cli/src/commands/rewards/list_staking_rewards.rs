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
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_zkc::contracts::IStakingRewards;
use clap::Args;
use colored::Colorize;
use std::collections::HashMap;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    indexer_client::{parse_amount, IndexerClient},
};

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

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsListStakingRewards {
    /// Run the list-staking-rewards command
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

        // Fetch staking history
        let history = client.get_staking_history(self.address).await?;

        if history.entries.is_empty() {
            println!("\n{} No staking history found for address {}", "ℹ".blue().bold(), format!("{:#x}", self.address).dimmed());
            return Ok(());
        }

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Get staking rewards contract for claim status checking
        let staking_rewards_address = rewards_config.staking_rewards_address()?;
        let staking_rewards = IStakingRewards::new(staking_rewards_address, &provider);

        // Get current epoch to set default range
        let current_epoch = staking_rewards
            .getCurrentEpoch()
            .call()
            .await
            .context("Failed to query current epoch")?
            .to::<u64>();

        // Set epoch range with defaults
        let end_epoch = self.end_epoch.unwrap_or(current_epoch);
        let start_epoch = self.start_epoch.unwrap_or_else(|| {
            if end_epoch >= 5 {
                end_epoch - 5
            } else {
                0
            }
        });

        // Filter entries by epoch range and create lookup map
        let mut entry_by_epoch: HashMap<u64, &crate::indexer_client::StakingEntry> = HashMap::new();
        for entry in &history.entries {
            if entry.epoch >= start_epoch && entry.epoch <= end_epoch {
                entry_by_epoch.insert(entry.epoch, entry);
            }
        }

        // Group epochs by claiming address and bulk check claim status
        let mut epochs_by_claimer: HashMap<Address, Vec<U256>> = HashMap::new();
        for entry in entry_by_epoch.values() {
            let claiming_address = if let Some(ref delegate_str) = entry.rewards_delegated_to {
                delegate_str.parse::<Address>().context("Invalid delegate address")?
            } else {
                self.address
            };
            epochs_by_claimer
                .entry(claiming_address)
                .or_insert_with(Vec::new)
                .push(U256::from(entry.epoch));
        }

        // Bulk query unclaimed rewards for each claiming address
        let mut unclaimed_map: HashMap<(Address, u64), U256> = HashMap::new();
        for (claiming_address, epochs) in epochs_by_claimer {
            let unclaimed_amounts = staking_rewards
                .calculateUnclaimedRewards(claiming_address, epochs.clone())
                .call()
                .await
                .context("Failed to query unclaimed rewards")?;

            for (epoch, unclaimed) in epochs.iter().zip(unclaimed_amounts.iter()) {
                unclaimed_map.insert((claiming_address, epoch.to::<u64>()), *unclaimed);
            }
        }

        println!("\n{} [{}]", "Staking Rewards History".bold(), network_name.blue().bold());
        println!("  Address: {}", format!("{:#x}", self.address).dimmed());
        println!("  Epoch Range: {}-{}", start_epoch, end_epoch);

        if let Some(summary) = &history.summary {
            let total_staked = parse_amount(&summary.total_staked)?;
            let total_rewards = parse_amount(&summary.total_rewards_generated)?;
            let total_staked_formatted = crate::format_amount(&format_ether(total_staked));
            let total_rewards_formatted = crate::format_amount(&format_ether(total_rewards));

            println!();
            println!("  Total Staked:             {} {}", total_staked_formatted.yellow().bold(), "ZKC".yellow());
            println!("  Total Rewards Generated:  {} {}", total_rewards_formatted.green().bold(), "ZKC".green());
            println!("  Epochs Participated:      {}", summary.epochs_participated);
            if summary.is_withdrawing {
                println!("  Status:                   {}", "Withdrawing".red().bold());
            }
        }

        println!("\n{}", "Epoch History".bold());
        for epoch in start_epoch..=end_epoch {
            println!();
            println!("  {} {}", "Epoch".dimmed(), epoch.to_string().cyan().bold());

            if let Some(entry) = entry_by_epoch.get(&epoch) {
                let staked = parse_amount(&entry.staked_amount)?;
                let rewards = parse_amount(&entry.rewards_generated)?;
                let staked_formatted = crate::format_amount(&format_ether(staked));
                let rewards_formatted = crate::format_amount(&format_ether(rewards));

                let claiming_address = if let Some(ref delegate_str) = entry.rewards_delegated_to {
                    delegate_str.parse::<Address>().context("Invalid delegate address")?
                } else {
                    self.address
                };

                let is_claimed = unclaimed_map
                    .get(&(claiming_address, entry.epoch))
                    .map(|unclaimed| *unclaimed == U256::ZERO)
                    .unwrap_or(true);

                println!("    Staked:             {} {}", staked_formatted.yellow(), "ZKC".yellow());
                println!("    Generated Rewards:  {} {}", rewards_formatted.green(), "ZKC".green());
                if let Some(ref delegate_str) = entry.rewards_delegated_to {
                    println!("    Delegated To:       {}", delegate_str.dimmed());
                }
                println!(
                    "    Claimed:            {}",
                    if is_claimed { "✓".green() } else { "✗".red() }
                );
                if let Some(rank) = entry.rank {
                    println!("    Rank:               {}", rank);
                }
                if entry.is_withdrawing {
                    println!("    Status:             {}", "Withdrawing".red());
                }
            } else {
                println!("    Staked:             {} {}", "0".dimmed(), "ZKC".dimmed());
                println!("    Generated Rewards:  {} {}", "0".dimmed(), "ZKC".dimmed());
            }
        }

        if self.dry_run {
            println!("\n{}", "Current Epoch Estimate (DRY RUN)".bold());
            println!("  {} These are estimates only and actual rewards may vary", "⚠".yellow().bold());
            // TODO: Fetch current epoch data and estimate rewards
        }

        Ok(())
    }
}
