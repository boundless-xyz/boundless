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

use crate::display::format_amount;
use crate::display::network_name_from_chain_id;
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
    display::DisplayManager,
    indexer_client::{parse_amount, IndexerClient},
};

/// List historical staking rewards for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsListStakingRewards {
    /// Address to query rewards for (defaults to configured reward address)
    #[clap(long)]
    pub address: Option<Address>,

    /// Start epoch (optional)
    #[clap(long)]
    pub start_epoch: Option<u64>,

    /// End epoch (optional)
    #[clap(long)]
    pub end_epoch: Option<u64>,

    /// Show estimated rewards for current epoch (dry-run)
    #[clap(long)]
    pub dry_run: bool,

    /// Display results in ascending order (oldest first)
    #[clap(long)]
    pub asc: bool,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsListStakingRewards {
    /// Run the list-staking-rewards command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Use provided address or default to reward address from config
        let address = self.address.or(rewards_config.reward_address)
            .context("No address provided.\n\nTo configure: run 'boundless rewards setup'\nOr provide --address <ADDRESS>")?;

        let rpc_url = rewards_config.require_rpc_url()?;
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch staking metadata for last updated timestamp
        let metadata = client.get_staking_metadata().await.ok();

        // Fetch reward delegation history - shows epochs where this address receives delegated power
        let delegation_history = client.get_reward_delegation_history(address).await?;

        let network_name = network_name_from_chain_id(Some(chain_id));
        let display = DisplayManager::with_network(network_name);

        if delegation_history.entries.is_empty() {
            display.info(&format!("No reward delegation history found for address {:#x}", address));
            return Ok(());
        }

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
        let start_epoch = self.start_epoch.unwrap_or_else(|| end_epoch.saturating_sub(5));

        // Create epoch data structures
        struct EpochRewardData {
            power: U256,
            delegators: Vec<String>,
            delegator_count: u64,
        }

        let mut epoch_data: HashMap<u64, EpochRewardData> = HashMap::new();

        // Filter delegation entries by epoch range
        for entry in &delegation_history.entries {
            if let Some(epoch) = entry.epoch {
                if epoch >= start_epoch && epoch <= end_epoch {
                    let power = parse_amount(&entry.power)?;
                    epoch_data.insert(
                        epoch,
                        EpochRewardData {
                            power,
                            delegators: entry.delegators.clone(),
                            delegator_count: entry.delegator_count,
                        },
                    );
                }
            }
        }

        // Fetch epoch staking data for reward calculation and check claim status
        let epochs_for_claim_check: Vec<U256> = epoch_data.keys().map(|&e| U256::from(e)).collect();

        // Bulk query unclaimed rewards - rewards go to the address we're querying
        let unclaimed_amounts = if !epochs_for_claim_check.is_empty() {
            staking_rewards
                .calculateUnclaimedRewards(address, epochs_for_claim_check.clone())
                .call()
                .await
                .context("Failed to query unclaimed rewards")?
        } else {
            vec![]
        };

        let mut unclaimed_map: HashMap<u64, U256> = HashMap::new();
        for (epoch, unclaimed) in epochs_for_claim_check.iter().zip(unclaimed_amounts.iter()) {
            unclaimed_map.insert(epoch.to::<u64>(), *unclaimed);
        }

        display.header("Staking Rewards History");

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            display.note(&format!("Data last updated: {}", formatted_time));
        }

        if self.address.is_some() {
            display.address("Address", address);
        } else {
            display.item("Address", format!("{:#x} (from config)", address));
        }
        display.item("Epoch Range", format!("{}-{}", start_epoch, end_epoch));

        // Calculate total rewards across all epochs
        let mut total_rewards = U256::ZERO;
        let mut epochs_participated = 0u64;

        for epoch in start_epoch..=end_epoch {
            if let Some(data) = epoch_data.get(&epoch) {
                // Fetch epoch staking summary to get total emissions and power
                if let Ok(epoch_summary) = client.get_epoch_staking(epoch).await {
                    let total_emissions = parse_amount(&epoch_summary.total_staking_emissions)?;
                    let total_power = parse_amount(&epoch_summary.total_staking_power)?;

                    if !total_power.is_zero() {
                        let rewards = (data.power * total_emissions) / total_power;
                        total_rewards += rewards;
                        epochs_participated += 1;
                    }
                }
            }
        }

        println!();
        display.balance(
            "Total Rewards Received",
            &format_amount(&format_ether(total_rewards)),
            "ZKC",
            "green",
        );
        display.item("Epochs Participated", epochs_participated);

        display.subsection("Epoch History");

        let epochs: Vec<u64> = if self.asc {
            (start_epoch..=end_epoch).collect()
        } else {
            (start_epoch..=end_epoch).rev().collect()
        };

        for epoch in epochs {
            println!();
            println!("  {} {}", "Epoch".dimmed(), epoch.to_string().cyan().bold());

            if let Some(data) = epoch_data.get(&epoch) {
                // Fetch epoch staking summary
                match client.get_epoch_staking(epoch).await {
                    Ok(epoch_summary) => {
                        let total_emissions = parse_amount(&epoch_summary.total_staking_emissions)?;
                        let total_power = parse_amount(&epoch_summary.total_staking_power)?;

                        // Calculate rewards: total_emissions * address_power / total_power
                        let rewards = if !total_power.is_zero() {
                            (data.power * total_emissions) / total_power
                        } else {
                            U256::ZERO
                        };

                        let power_formatted = format_amount(&format_ether(data.power));
                        let rewards_formatted = format_amount(&format_ether(rewards));

                        display.subitem(
                            "Reward Power:",
                            &format!("{} {}", power_formatted.yellow(), "ZKC".yellow()),
                        );
                        display.subitem(
                            "Rewards:",
                            &format!("{} {}", rewards_formatted.green(), "ZKC".green()),
                        );

                        // Show delegators if this is delegated power
                        if data.delegator_count > 0 {
                            display.subitem(
                                "Delegated From:",
                                &format!("{} staker(s)", data.delegator_count.to_string().cyan()),
                            );
                            if data.delegators.len() <= 3 {
                                for delegator in &data.delegators {
                                    display.subitem("  -", &delegator.dimmed().to_string());
                                }
                            } else {
                                for delegator in data.delegators.iter().take(3) {
                                    display.subitem("  -", &delegator.dimmed().to_string());
                                }
                                display.subitem(
                                    "  -",
                                    &format!("... and {} more", data.delegators.len() - 3)
                                        .dimmed()
                                        .to_string(),
                                );
                            }
                        }

                        // Determine claim status based on epoch and unclaimed amount
                        let claim_status = if epoch >= current_epoch {
                            // Current or future epoch - can't claim yet
                            format!("{} {}", "⏳".yellow(), "In Progress".yellow())
                        } else {
                            // Past epoch - check if claimed
                            let is_claimed = unclaimed_map
                                .get(&epoch)
                                .map(|unclaimed| *unclaimed == U256::ZERO)
                                .unwrap_or(true);

                            if is_claimed {
                                format!("{}", "✓".green())
                            } else {
                                format!("{}", "✗".red())
                            }
                        };

                        display.subitem("Claimed:", &claim_status);
                    }
                    Err(e) => {
                        display.subitem(
                            "Error:",
                            &format!("Failed to fetch epoch data: {}", e).red().to_string(),
                        );
                    }
                }
            } else {
                display.subitem("Reward Power:", &format!("{} {}", "0".dimmed(), "ZKC".dimmed()));
                display.subitem("Rewards:", &format!("{} {}", "0".dimmed(), "ZKC".dimmed()));
            }
        }

        if self.dry_run {
            display.subsection("Current Epoch Estimate (DRY RUN)");
            display.warning("These are estimates only and actual rewards may vary");
            // TODO: Fetch current epoch data and estimate rewards
        }

        Ok(())
    }
}
