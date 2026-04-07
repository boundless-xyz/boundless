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

//! Shared logic for the per-module `networks` subcommand.

use anyhow::Result;
use colored::Colorize;

use crate::commands::setup::network::{
    display_name_for_network, normalize_market_network, normalize_rewards_network,
};
use crate::config_file::{Config, Secrets};

/// Which CLI module is running the networks command.
#[derive(Clone, Copy)]
pub enum NetworkModule {
    /// Requestor module (market chains).
    Requestor,
    /// Prover module (market chains).
    Prover,
    /// Rewards module (Ethereum L1 chains).
    Rewards,
}

impl NetworkModule {
    fn label(self) -> &'static str {
        match self {
            Self::Requestor => "Requestor",
            Self::Prover => "Prover",
            Self::Rewards => "Rewards",
        }
    }

    fn normalize(self, input: &str) -> &str {
        match self {
            Self::Requestor | Self::Prover => normalize_market_network(input),
            Self::Rewards => normalize_rewards_network(input),
        }
    }

    fn active_network(self, config: &Config) -> Option<String> {
        match self {
            Self::Requestor => config.requestor.as_ref().map(|r| r.network.clone()),
            Self::Prover => config.prover.as_ref().map(|p| p.network.clone()),
            Self::Rewards => config.rewards.as_ref().map(|r| r.network.clone()),
        }
    }

    fn set_network(self, config: &mut Config, key: &str) {
        match self {
            Self::Requestor => {
                config.requestor =
                    Some(crate::config_file::RequestorConfig { network: key.to_string() });
            }
            Self::Prover => {
                config.prover = Some(crate::config_file::ProverConfig { network: key.to_string() });
            }
            Self::Rewards => {
                config.rewards =
                    Some(crate::config_file::RewardsConfig { network: key.to_string() });
            }
        }
    }

    fn has_secrets(self, secrets: &Secrets, key: &str) -> bool {
        match self {
            Self::Requestor => secrets.requestor_networks.contains_key(key),
            Self::Prover => secrets.prover_networks.contains_key(key),
            Self::Rewards => secrets.rewards_networks.contains_key(key),
        }
    }

    fn tip_example(self) -> &'static str {
        match self {
            Self::Requestor => {
                "Switch active network with: boundless requestor networks --set \"Taiko Mainnet\""
            }
            Self::Prover => {
                "Switch active network with: boundless prover networks --set \"Taiko Mainnet\""
            }
            Self::Rewards => {
                "Switch active network with: boundless rewards networks --set \"Ethereum Mainnet\""
            }
        }
    }
}

/// Set the active network for a module.
pub fn set_active_network(module: NetworkModule, input: &str) -> Result<()> {
    let key = module.normalize(input);
    let display = display_name_for_network(key);

    let mut config = Config::load().unwrap_or_default();
    module.set_network(&mut config, key);
    config.save()?;

    println!();
    println!("{} {} active network set to {}", "✓".green().bold(), module.label(), display.bold());

    let secrets = Secrets::load().ok();
    let has_secrets = secrets.as_ref().is_some_and(|s| module.has_secrets(s, key));
    if !has_secrets {
        println!(
            "  {} {}",
            "⚠".yellow(),
            format!(
                "No credentials configured for {display}. Run 'boundless {} setup' to add RPC URL and keys.",
                module.label().to_lowercase()
            )
            .yellow()
        );
    }
    println!();

    Ok(())
}

/// List supported networks for a module.
pub fn list_networks(module: NetworkModule) {
    let config = Config::load().ok();
    let secrets = Secrets::load().ok();
    let active_network = config.as_ref().and_then(|c| module.active_network(c));

    println!();
    println!("{}", format!("{} Networks", module.label()).bold());
    println!();

    match module {
        NetworkModule::Requestor | NetworkModule::Prover => {
            list_market_networks(module, active_network.as_deref(), &secrets, &config);
        }
        NetworkModule::Rewards => {
            list_rewards_networks(active_network.as_deref(), &secrets, &config);
        }
    }

    println!();
    println!("{} {}", "Tip:".bold(), module.tip_example().dimmed());
    println!();
}

fn network_status(
    module: NetworkModule,
    secrets: &Option<Secrets>,
    key: &str,
    active_network: Option<&str>,
) -> String {
    let has_secrets = secrets.as_ref().is_some_and(|s| module.has_secrets(s, key));
    if active_network == Some(key) {
        "active".green().to_string()
    } else if has_secrets {
        "configured".blue().to_string()
    } else {
        "--".dimmed().to_string()
    }
}

fn list_market_networks(
    module: NetworkModule,
    active_network: Option<&str>,
    secrets: &Option<Secrets>,
    config: &Option<Config>,
) {
    for (chain_id, name, is_mainnet) in boundless_market::deployments::SUPPORTED_CHAINS {
        let key = normalize_market_network(name);
        let tag = if *is_mainnet { "mainnet" } else { "testnet" };
        let status = network_status(module, secrets, key, active_network);

        println!(
            "  {:<30} {:<10} {}",
            format!("{} ({})", name, chain_id).bold(),
            format!("[{}]", tag).dimmed(),
            status,
        );
    }

    if let Some(ref config) = config {
        for custom in &config.custom_markets {
            let status = network_status(module, secrets, &custom.name, active_network);

            println!(
                "  {:<30} {:<10} {}",
                format!("{} ({})", custom.name, custom.chain_id).bold(),
                "[custom]".dimmed(),
                status,
            );
        }
    }
}

fn list_rewards_networks(
    active_network: Option<&str>,
    secrets: &Option<Secrets>,
    config: &Option<Config>,
) {
    for (key, label) in
        [("eth-mainnet", "Ethereum Mainnet (1)"), ("eth-sepolia", "Ethereum Sepolia (11155111)")]
    {
        let status = network_status(NetworkModule::Rewards, secrets, key, active_network);
        println!("  {:<35} {}", label.bold(), status);
    }

    if let Some(ref config) = config {
        for custom in &config.custom_rewards {
            let status =
                network_status(NetworkModule::Rewards, secrets, &custom.name, active_network);

            println!(
                "  {:<35} {}",
                format!("{} ({})", custom.name, custom.chain_id).bold(),
                status,
            );
        }
    }
}
