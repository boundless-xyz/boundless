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

use anyhow::Result;
use clap::Args;

use crate::commands::config_display::{
    address_from_pk, display_not_configured, display_rpc_url, display_tip,
    get_private_key_with_source, get_rpc_url_with_source, normalize_network_name, ModuleType,
};
use crate::commands::rewards::State;
use crate::config::GlobalConfig;
use crate::config_file::{Config, Secrets};
use crate::display::obscure_url;
use crate::display::DisplayManager;

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
        let module = ModuleType::Rewards;
        let display = DisplayManager::new();

        display.header(module.display_name());

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        let Some(ref cfg) = config else {
            display_not_configured(&display, module);
            display_tip(&display, module);
            return Ok(());
        };

        let Some(ref rewards) = cfg.rewards else {
            display_not_configured(&display, module);
            display_tip(&display, module);
            return Ok(());
        };

        let network = normalize_network_name(&rewards.network);
        display.item_colored("Network", network, "cyan");

        if let Some(ref sec) = secrets {
            if let Some(rewards_sec) = sec.rewards_networks.get(&rewards.network) {
                let (rpc_url, rpc_source) = get_rpc_url_with_source(
                    module.rpc_url_env_var(),
                    rewards_sec.rpc_url.as_deref(),
                );
                display_rpc_url(&display, rpc_url, rpc_source);

                // Display staking address
                let (staking_pk, staking_pk_source) = get_private_key_with_source(
                    "STAKING_PRIVATE_KEY",
                    rewards_sec.staking_private_key.as_deref(),
                );

                if let Some(pk) = staking_pk {
                    if let Some(addr) = address_from_pk(pk) {
                        display.item_colored("Staking Address", format!("{:#x}", addr), "green");
                    }
                    display
                        .subitem("  ", &format!("Private Key: Configured [{}]", staking_pk_source));
                } else if let Some(ref addr) = rewards_sec.staking_address {
                    display.item_colored("Staking Address", addr, "green");
                    display.item_colored("  Private Key", "Not configured", "yellow");
                }

                // Display reward address
                let (reward_pk, reward_pk_source) = get_private_key_with_source(
                    "REWARD_PRIVATE_KEY",
                    rewards_sec.reward_private_key.as_deref(),
                );

                if let Some(pk) = reward_pk {
                    if let Some(addr) = address_from_pk(pk) {
                        display.item_colored("Reward Address", format!("{:#x}", addr), "green");
                    }
                    display
                        .subitem("  ", &format!("Private Key: Configured [{}]", reward_pk_source));
                } else if let Some(ref addr) = rewards_sec.reward_address {
                    display.item_colored("Reward Address", addr, "green");
                    display.item_colored("  Private Key", "Not configured", "yellow");
                }

                // Display Mining state file if configured
                let env_mining_state = std::env::var("MINING_STATE_FILE").ok();
                let mining_state_path =
                    env_mining_state.as_deref().or(rewards_sec.mining_state_file.as_deref());

                if let Some(state_path) = mining_state_path {
                    let display_path = std::fs::canonicalize(state_path)
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| {
                            std::env::current_dir()
                                .map(|cwd| cwd.join(state_path).display().to_string())
                                .unwrap_or_else(|_| state_path.to_string())
                        });

                    display.item_colored("PoVW State File", display_path, "green");

                    match State::load(state_path).await {
                        Ok(state) => {
                            if let Ok(elapsed) = state.updated_at.elapsed() {
                                let duration_str = format_duration(elapsed);
                                display.subitem("  ", &format!("Last updated: {}", duration_str));
                            }

                            if !state.work_log.is_empty() {
                                display.subitem(
                                    "  ",
                                    &format!("Total PoVW jobs: {}", state.work_log.jobs.len()),
                                );
                            } else {
                                display.item_colored("  Total PoVW jobs", "0 (empty)", "yellow");
                            }

                            if !state.update_transactions.is_empty() {
                                if let Some((tx_hash, _)) = state.update_transactions.iter().next()
                                {
                                    let time_str = state
                                        .updated_at
                                        .elapsed()
                                        .map(format_duration)
                                        .unwrap_or_else(|_| "unknown".to_string());
                                    display.subitem(
                                        "  ",
                                        &format!(
                                            "Last submitted on-chain: {} (tx: {:#x})",
                                            time_str, tx_hash
                                        ),
                                    );
                                }
                            } else {
                                display.item_colored(
                                    "  Last submitted",
                                    "Never submitted",
                                    "yellow",
                                );
                            }
                        }
                        Err(e) => {
                            if e.to_string().contains("No such file or directory")
                                || e.to_string().contains("Failed to read work log state file")
                            {
                                display.item_colored("  Status", "⚠ File not found: verify path or run prepare-povw to initialize", "yellow");
                            } else {
                                display.item_colored(
                                    "  Status",
                                    "⚠ Invalid file: cannot decode state",
                                    "yellow",
                                );
                            }
                        }
                    }
                } else {
                    display.item_colored("PoVW State", "Not configured (PoVW disabled)", "yellow");
                }

                // Display Beacon API URL if configured
                let env_beacon_api = std::env::var("BEACON_API_URL").ok();
                if let Some(ref url) = env_beacon_api {
                    display.item_colored(
                        "Beacon API",
                        format!("{} [env]", obscure_url(url)),
                        "dimmed",
                    );
                } else if let Some(ref url) = rewards_sec.beacon_api_url {
                    display.item_colored(
                        "Beacon API",
                        format!("{} [config file]", obscure_url(url)),
                        "dimmed",
                    );
                }
            }
        }

        display_tip(&display, module);

        Ok(())
    }
}
