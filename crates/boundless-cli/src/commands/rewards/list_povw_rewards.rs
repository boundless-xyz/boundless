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
use clap::Args;
use colored::Colorize;

use crate::{
    config::{GlobalConfig, RewardsConfig},
    indexer_client::{parse_amount, IndexerClient},
};

/// List historical PoVW rewards for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsListPovwRewards {
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

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsListPovwRewards {
    /// Run the list-povw-rewards command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Use provided address or default to reward address from config
        let address = self.address.or(rewards_config.reward_address)
            .context("No address provided.\n\nTo configure: run 'boundless setup rewards'\nOr provide --address <ADDRESS>")?;

        let rpc_url = rewards_config.require_rpc_url()?;
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
        let chain_id = provider.get_chain_id().await?;

        // Create indexer client based on chain ID
        let client = IndexerClient::new_from_chain_id(chain_id)?;

        // Fetch PoVW metadata for last updated timestamp
        let metadata = client.get_povw_metadata().await.ok();

        // Fetch PoVW history
        let history = client.get_povw_history(address).await?;

        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        println!("\n{} [{}]", "PoVW Rewards History".bold(), network_name.blue().bold());

        if let Some(ref meta) = metadata {
            let formatted_time = crate::indexer_client::format_timestamp(&meta.last_updated_at);
            println!("  Data last updated: {}", formatted_time.dimmed());
        }

        if self.address.is_some() {
            println!("  Address: {}", format!("{:#x}", address).cyan());
        } else {
            println!("  Address: {} {}", format!("{:#x}", address).cyan(), "(from config)".dimmed());
        }

        if history.entries.is_empty() {
            println!("  Status:  {} {}", "No history".yellow(), "(no PoVW work submitted yet)".dimmed());
            return Ok(());
        }

        if let Some(summary) = &history.summary {
            let total_work = parse_amount(&summary.total_work_submitted)?;
            let total_actual = parse_amount(&summary.total_actual_rewards)?;
            let total_uncapped = parse_amount(&summary.total_uncapped_rewards)?;

            let total_work_formatted = format_work_cycles(&total_work);
            let total_actual_formatted = crate::format_amount(&format_ether(total_actual));
            let total_uncapped_formatted = crate::format_amount(&format_ether(total_uncapped));

            println!("\n{}", "Lifetime Summary".bold().green());
            println!("  Total Work Submitted:    {}", total_work_formatted.cyan());
            println!("  Total Actual Rewards:    {} {}", total_actual_formatted.green().bold(), "ZKC".green());
            println!("  Total Uncapped Rewards:  {} {}", total_uncapped_formatted.cyan(), "ZKC".cyan());
            println!("  Epochs Participated:     {}", summary.epochs_participated.to_string().cyan());

            // Check if rewards were capped
            if total_actual < total_uncapped {
                let capped_amount = total_uncapped - total_actual;
                let capped_formatted = crate::format_amount(&format_ether(capped_amount));
                println!("  {} {}", "⚠".yellow(), format!("Rewards capped by {} ZKC due to staking limits", capped_formatted).yellow());
            }
        }

        println!("\n{}", "Epoch History".bold().green());
        for entry in &history.entries {
            let work = parse_amount(&entry.work_submitted)?;
            let actual = parse_amount(&entry.actual_rewards)?;
            let uncapped = parse_amount(&entry.uncapped_rewards)?;

            let work_formatted = format_work_cycles(&work);
            let actual_formatted = crate::format_amount(&format_ether(actual));

            let capped_indicator = if entry.is_capped {
                format!(" {}", "[CAPPED]".red().bold())
            } else {
                String::new()
            };

            println!(
                "  Epoch {}: Work={}, Rewards={} {} ({:.1}% of work){}",
                entry.epoch.to_string().cyan(),
                work_formatted.cyan(),
                actual_formatted.green().bold(),
                "ZKC".green(),
                entry.percentage,
                capped_indicator
            );

            if entry.is_capped {
                let cap = parse_amount(&entry.reward_cap)?;
                let staked = parse_amount(&entry.staked_amount)?;
                let cap_formatted = crate::format_amount(&format_ether(cap));
                let staked_formatted = crate::format_amount(&format_ether(staked));
                let uncapped_formatted = crate::format_amount(&format_ether(uncapped));

                println!(
                    "    {} Capped at {} {} (staked: {} {})",
                    "→".dimmed(),
                    cap_formatted.yellow(),
                    "ZKC".yellow(),
                    staked_formatted.cyan(),
                    "ZKC".cyan()
                );
                println!("    {} Uncapped would have been: {} {}", "→".dimmed(), uncapped_formatted.cyan(), "ZKC".cyan());
            }
        }

        if self.dry_run {
            println!("\n{}", "Current Epoch Estimate (DRY RUN)".bold().yellow());
            println!("  {} {}", "⚠".yellow(), "These are estimates only and actual rewards may vary".yellow());

            // Get current epoch data
            // TODO: Query actual current epoch from contract
            let current_epoch = 5; // Placeholder

            match client.get_epoch_povw(current_epoch).await {
                Ok(epoch_summary) => {
                    let total_work = parse_amount(&epoch_summary.total_work)?;
                    let total_emissions = parse_amount(&epoch_summary.total_emissions)?;

                    let total_work_formatted = crate::format_amount(&format_ether(total_work));
                    let total_emissions_formatted = crate::format_amount(&format_ether(total_emissions));

                    println!("\n  Current Epoch {}", current_epoch.to_string().cyan());
                    println!("    Total Work Submitted:  {}", total_work_formatted.cyan());
                    println!("    Total Emissions:       {} {}", total_emissions_formatted.green(), "ZKC".green());
                    println!("    Participants:          {}", epoch_summary.num_participants.to_string().cyan());

                    println!("\n  {} {}", "💡".cyan(), "To maximize rewards:".bold());
                    println!("    - Submit work early in the epoch");
                    println!("    - Ensure adequate ZKC staking to avoid caps");
                    println!("    - Monitor your reward cap: 2.5x your staked amount");
                }
                Err(e) => {
                    println!("  {} Unable to fetch current epoch data: {}", "⚠".yellow(), e.to_string().dimmed());
                }
            }
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
