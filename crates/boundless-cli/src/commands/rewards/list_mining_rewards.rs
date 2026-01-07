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
    primitives::{utils::format_ether, Address, U256},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{Context, Result};
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use clap::Args;
use colored::Colorize;
use std::collections::HashMap;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    display::{format_amount, network_name_from_chain_id, DisplayManager},
    indexer_client::{parse_amount, IndexerClient},
};

/// List historical mining rewards for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsListMiningRewards {
    /// Work log ID (address) to query rewards for (defaults to configured reward address)
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

impl RewardsListMiningRewards {
    /// Run the list-mining-rewards command
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

        // Get deployment for PoVW contract addresses
        let deployment = Deployment::from_chain_id(chain_id)
            .context("Could not determine deployment from chain ID")?;

        // Get PoVW accounting contract to query current epoch
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, &provider);
        let current_epoch = povw_accounting
            .pendingEpoch()
            .call()
            .await
            .context("Failed to query current epoch")?
            .number
            .to::<u64>();

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch PoVW metadata for last updated timestamp
        let metadata = client.get_povw_metadata().await.ok();

        // Fetch PoVW history
        let history = client.get_povw_history(address).await?;

        let network_name = network_name_from_chain_id(Some(chain_id));
        let display = DisplayManager::with_network(network_name);

        // Set epoch range with defaults
        let end_epoch = self.end_epoch.unwrap_or(current_epoch);
        let start_epoch = self.start_epoch.unwrap_or_else(|| end_epoch.saturating_sub(5));

        // Create epoch data structures
        struct EpochMiningData {
            work_submitted: U256,
            actual_rewards: U256,
            uncapped_rewards: U256,
            reward_cap: U256,
            staked_amount: U256,
            is_capped: bool,
            percentage: f64,
        }

        let mut epoch_data: HashMap<u64, EpochMiningData> = HashMap::new();

        // Filter history entries by epoch range
        for entry in &history.entries {
            if entry.epoch >= start_epoch && entry.epoch <= end_epoch {
                let work = parse_amount(&entry.work_submitted)?;
                let actual = parse_amount(&entry.actual_rewards)?;
                let uncapped = parse_amount(&entry.uncapped_rewards)?;
                let cap = parse_amount(&entry.reward_cap)?;
                let staked = parse_amount(&entry.staked_amount)?;

                epoch_data.insert(
                    entry.epoch,
                    EpochMiningData {
                        work_submitted: work,
                        actual_rewards: actual,
                        uncapped_rewards: uncapped,
                        reward_cap: cap,
                        staked_amount: staked,
                        is_capped: entry.is_capped,
                        percentage: entry.percentage,
                    },
                );
            }
        }

        display.header("Mining Rewards History");

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

        // Calculate totals across the epoch range
        let mut total_rewards = U256::ZERO;
        let mut total_work = U256::ZERO;
        let mut epochs_participated = 0u64;

        for epoch in start_epoch..=end_epoch {
            if let Some(data) = epoch_data.get(&epoch) {
                total_rewards += data.actual_rewards;
                total_work += data.work_submitted;
                epochs_participated += 1;
            }
        }

        println!();
        display.balance(
            "Total Rewards Received",
            &format_amount(&format_ether(total_rewards)),
            "ZKC",
            "green",
        );
        display.item_colored("Total Work Submitted", format_work_cycles(&total_work), "cyan");
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
                let work_formatted = format_work_cycles(&data.work_submitted);
                let actual_formatted = format_amount(&format_ether(data.actual_rewards));
                let uncapped_formatted = format_amount(&format_ether(data.uncapped_rewards));
                let cap_formatted = format_amount(&format_ether(data.reward_cap));
                let staked_formatted = format_amount(&format_ether(data.staked_amount));

                display.subitem(
                    "Work Submitted:",
                    &format!("{} ({:.2}% of epoch)", work_formatted.cyan(), data.percentage),
                );
                display.subitem(
                    "Actual Rewards:",
                    &format!("{} {}", actual_formatted.green(), "ZKC".green()),
                );

                if data.is_capped {
                    display.subitem(
                        "Uncapped Rewards:",
                        &format!("{} {}", uncapped_formatted.yellow(), "ZKC".yellow()),
                    );
                    display.subitem(
                        "Reward Cap:",
                        &format!("{} {}", cap_formatted.yellow(), "ZKC".yellow()),
                    );
                    display.subitem(
                        "Staked Amount:",
                        &format!("{} {}", staked_formatted.cyan(), "ZKC".cyan()),
                    );
                    display
                        .subitem("Capped:", &format!("{}", "✓ (rewards limited by stake)".red()));
                } else {
                    display.subitem("Capped:", &format!("{}", "✗".green()));
                }
            } else {
                display
                    .subitem("Work Submitted:", &format!("{} {}", "0".dimmed(), "cycles".dimmed()));
                display.subitem("Actual Rewards:", &format!("{} {}", "0".dimmed(), "ZKC".dimmed()));
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

/// Helper to format work cycles with commas and " cycles" suffix
fn format_work_cycles(amount: &U256) -> String {
    let value = amount.to::<u128>();
    let formatted = format_number_with_commas(value);
    format!("{} cycles", formatted)
}

/// Helper to format numbers with comma separators
fn format_number_with_commas(n: u128) -> String {
    let s = n.to_string();
    let mut result = String::new();
    let mut count = 0;

    for c in s.chars().rev() {
        if count == 3 {
            result.push(',');
            count = 0;
        }
        result.push(c);
        count += 1;
    }

    result.chars().rev().collect()
}
