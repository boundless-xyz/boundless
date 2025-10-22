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

use anyhow::Result;
use clap::Args;
use colored::Colorize;

use crate::commands::rewards::State;
use crate::commands::setup::secrets::{address_from_private_key, obscure_url};
use crate::config::GlobalConfig;
use crate::config_file::{Config, Secrets};

/// Show rewards configuration status
#[derive(Args, Clone, Debug)]
pub struct RewardsConfigCmd {}

fn format_duration(d: std::time::Duration) -> String {
    let total_seconds = d.as_secs();
    if total_seconds < 60 {
        format!("{} seconds ago", total_seconds)
    } else if total_seconds < 3600 {
        format!("{} minutes ago", total_seconds / 60)
    } else if total_seconds < 86400 {
        format!("{} hours ago", total_seconds / 3600)
    } else {
        format!("{} days ago", total_seconds / 86400)
    }
}

impl RewardsConfigCmd {
    /// Run the command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        println!("\n{}\n", "Rewards Module".bold().underline());

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        if let Some(ref cfg) = config {
            if let Some(ref rewards) = cfg.rewards {
                let display_network = match rewards.network.as_str() {
                    "mainnet" => "Ethereum Mainnet",
                    "sepolia" => "Ethereum Sepolia",
                    custom => custom,
                };

                println!("{} {}", "Network:".bold(), display_network.blue());

                if let Some(ref sec) = secrets {
                    if let Some(rewards_sec) = sec.rewards_networks.get(&rewards.network) {
                        if let Some(ref rpc) = rewards_sec.rpc_url {
                            println!("{} {}", "RPC URL:".bold(), obscure_url(rpc).dimmed());
                        }

                        // Display staking address
                        if let Some(ref staking_pk) = rewards_sec.staking_private_key {
                            if let Some(addr) = address_from_private_key(staking_pk) {
                                println!("{} {}", "Staking Address:".bold(), format!("{:#x}", addr).green());
                            }
                            println!("  {} {}", "Private Key:".bold(), "Configured".green());
                        } else if let Some(ref staking_addr) = rewards_sec.staking_address {
                            println!("{} {}", "Staking Address:".bold(), staking_addr.green());
                            println!("  {} {}", "Private Key:".bold(), "Not configured".yellow());
                        }

                        // Display reward address
                        if let Some(ref reward_pk) = rewards_sec.reward_private_key {
                            if let Some(addr) = address_from_private_key(reward_pk) {
                                println!("{} {}", "Reward Address:".bold(), format!("{:#x}", addr).green());
                            }
                            println!("  {} {}", "Private Key:".bold(), "Configured".green());
                        } else if let Some(ref reward_addr) = rewards_sec.reward_address {
                            println!("{} {}", "Reward Address:".bold(), reward_addr.green());
                            println!("  {} {}", "Private Key:".bold(), "Not configured".yellow());
                        }

                        // Display PoVW state file if configured
                        let env_povw_state = std::env::var("POVW_STATE_FILE").ok();
                        let povw_state_path = if let Some(ref state) = env_povw_state {
                            Some(state.as_str())
                        } else {
                            rewards_sec.povw_state_file.as_deref()
                        };

                        if let Some(state_path) = povw_state_path {
                            // Get absolute path for display
                            let display_path = std::fs::canonicalize(state_path)
                                .map(|p| p.display().to_string())
                                .unwrap_or_else(|_| {
                                    // If canonicalize fails, try to resolve relative to current dir
                                    std::env::current_dir()
                                        .map(|cwd| cwd.join(state_path).display().to_string())
                                        .unwrap_or_else(|_| state_path.to_string())
                                });

                            println!("{} {}", "PoVW State File:".bold(), display_path.green());

                            // Try to load and display state statistics
                            match State::load(state_path).await {
                                Ok(state) => {
                                    // Display last updated time
                                    if let Ok(elapsed) = state.updated_at.elapsed() {
                                        let duration_str = format_duration(elapsed);
                                        println!("  {} {}", "Last updated:".bold(), duration_str.dimmed());
                                    }

                                    // Display work log stats
                                    if !state.work_log.is_empty() {
                                        println!("  {} {}",
                                            "Total PoVW jobs:".bold(),
                                            state.work_log.jobs.len().to_string().cyan());
                                    } else {
                                        println!("  {} {}", "Total PoVW jobs:".bold(), "0 (empty)".yellow());
                                    }

                                    // Display last on-chain submission from update_transactions
                                    if !state.update_transactions.is_empty() {
                                        if let Some((tx_hash, _)) = state.update_transactions.iter().next() {
                                            let time_str = state.updated_at.elapsed()
                                                .map(format_duration)
                                                .unwrap_or_else(|_| "unknown".to_string());
                                            println!("  {} {} (tx: {})",
                                                "Last submitted on-chain:".bold(),
                                                time_str.dimmed(),
                                                format!("{:#x}", tx_hash).cyan());
                                        }
                                    } else {
                                        println!("  {} {}", "Last submitted on-chain:".bold(), "Never submitted".yellow());
                                    }
                                }
                                Err(e) => {
                                    // Distinguish between file not found vs invalid file
                                    if e.to_string().contains("No such file or directory") ||
                                       e.to_string().contains("Failed to read work log state file") {
                                        println!("  {} {} File not found: verify path or run prepare-povw to initialize", "Status:".bold(), "⚠".yellow());
                                    } else {
                                        println!("  {} {} Invalid file: cannot decode state", "Status:".bold(), "⚠".yellow());
                                    }
                                }
                            }
                        } else {
                            println!("{} {}", "PoVW State:".bold(), "Not configured (PoVW disabled)".yellow());
                        }

                        // Display Beacon API URL if configured
                        let env_beacon_api = std::env::var("BEACON_API_URL").ok();
                        if let Some(ref url) = env_beacon_api {
                            println!("{} {} {}", "Beacon API:".bold(), obscure_url(url).dimmed(), "[env]".dimmed());
                        } else if let Some(ref url) = rewards_sec.beacon_api_url {
                            println!("{} {} {}", "Beacon API:".bold(), obscure_url(url).dimmed(), "[config file]".dimmed());
                        }
                    }
                }
            } else {
                println!("{}", "Not configured - run 'boundless rewards setup'".red());
            }
        } else {
            println!("{}", "Not configured - run 'boundless rewards setup'".red());
        }

        println!(
            "\n💡 {} {}",
            "Tip:".bold(),
            "Run 'boundless rewards --help' to see available commands".dimmed()
        );
        println!();

        Ok(())
    }
}
