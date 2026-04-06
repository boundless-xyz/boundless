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

//! Commands for requestors interacting with the Boundless market.

mod balance;
mod config;
mod deposit;
mod get_proof;
mod status;
mod submit;
mod submit_offer;
mod verify_proof;
mod withdraw;

pub use balance::RequestorBalance;
pub use config::RequestorConfigCmd;
pub use deposit::RequestorDeposit;
pub use get_proof::RequestorGetProof;
pub use status::RequestorStatus;
pub use submit::RequestorSubmit;
pub use submit_offer::RequestorSubmitOffer;
pub use verify_proof::RequestorVerifyProof;
pub use withdraw::RequestorWithdraw;

use clap::{Args, Subcommand};

use crate::{
    commands::setup::{
        network::{display_name_for_network, normalize_market_network},
        RequestorSetup,
    },
    config::GlobalConfig,
    config_file::{Config, Secrets},
};

/// Commands for requestors
#[derive(Subcommand, Clone, Debug)]
#[command(after_help = "\x1b[1;4mModule Configuration:\x1b[0m
  Run 'boundless requestor setup' for interactive setup

  Alternatively set environment variables:
    \x1b[1mREQUESTOR_RPC_URL\x1b[0m         RPC endpoint for requestor module
    \x1b[1mREQUESTOR_PRIVATE_KEY\x1b[0m     Private key for requestor transactions
    \x1b[1mBOUNDLESS_MARKET_ADDRESS\x1b[0m  Market contract address (optional, has default)
    \x1b[1mSET_VERIFIER_ADDRESS\x1b[0m      Verifier contract address (optional, has default)

  Or configure while executing commands:
    Example: \x1b[1mboundless requestor balance --requestor-rpc-url <url> --requestor-private-key <key>\x1b[0m")]
pub enum RequestorCommands {
    /// Show requestor configuration status
    Config(RequestorConfigCmd),
    /// Deposit funds into the market
    Deposit(RequestorDeposit),
    /// Withdraw funds from the market
    Withdraw(RequestorWithdraw),
    /// Check the balance of an account in the market
    #[command(name = "deposited-balance")]
    DepositedBalance(RequestorBalance),
    /// Check the balance of an account (alias for deposited-balance)
    Balance(RequestorBalance),
    /// Submit a fully specified proof request from a YAML file
    #[command(name = "submit-file")]
    Submit(RequestorSubmit),
    /// Submit a proof request constructed with the given offer, input, and image
    #[command(name = "submit")]
    SubmitOffer(Box<RequestorSubmitOffer>),
    /// Get the status of a given request
    Status(RequestorStatus),
    /// Get the journal and seal for a given request
    #[command(name = "get-proof")]
    GetProof(RequestorGetProof),
    /// Verify the proof of the given request
    #[command(name = "verify-proof")]
    VerifyProof(RequestorVerifyProof),
    /// Interactive setup wizard for requestor configuration
    Setup(RequestorSetup),
    /// List supported networks or switch the active network
    Networks(NetworksCmd),
}

/// List or switch the active requestor network
#[derive(Args, Clone, Debug)]
pub struct NetworksCmd {
    /// Switch the active network (accepts name, kebab-case key, or chain ID)
    #[clap(long)]
    pub set: Option<String>,
}

impl RequestorCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Config(cmd) => cmd.run(global_config).await,
            Self::Deposit(cmd) => cmd.run(global_config).await,
            Self::Withdraw(cmd) => cmd.run(global_config).await,
            Self::DepositedBalance(cmd) | Self::Balance(cmd) => cmd.run(global_config).await,
            Self::Submit(cmd) => cmd.run(global_config).await,
            Self::SubmitOffer(cmd) => cmd.run(global_config).await,
            Self::Status(cmd) => cmd.run(global_config).await,
            Self::GetProof(cmd) => cmd.run(global_config).await,
            Self::VerifyProof(cmd) => cmd.run(global_config).await,
            Self::Setup(cmd) => cmd.run(global_config).await,
            Self::Networks(cmd) => {
                if let Some(ref network) = cmd.set {
                    set_requestor_network(network)
                } else {
                    show_requestor_networks();
                    Ok(())
                }
            }
        }
    }
}

fn set_requestor_network(input: &str) -> anyhow::Result<()> {
    use colored::Colorize;

    let key = normalize_market_network(input);
    let display = display_name_for_network(key);

    let mut config = Config::load().unwrap_or_default();
    config.requestor = Some(crate::config_file::RequestorConfig { network: key.to_string() });
    config.save()?;

    println!();
    println!("{} Requestor active network set to {}", "✓".green().bold(), display.bold());

    let secrets = Secrets::load().ok();
    let has_secrets = secrets.as_ref().and_then(|s| s.requestor_networks.get(key)).is_some();
    if !has_secrets {
        println!(
            "  {} {}",
            "⚠".yellow(),
            format!(
                "No credentials configured for {display}. Run 'boundless requestor setup' to add RPC URL and keys."
            )
            .yellow()
        );
    }
    println!();

    Ok(())
}

fn show_requestor_networks() {
    use colored::Colorize;

    let config = Config::load().ok();
    let secrets = Secrets::load().ok();
    let active_network =
        config.as_ref().and_then(|c| c.requestor.as_ref()).map(|r| r.network.as_str());

    println!();
    println!("{}", "Requestor Networks".bold());
    println!();

    for (chain_id, name, is_mainnet) in boundless_market::deployments::SUPPORTED_CHAINS {
        let key = normalize_market_network(name);
        let tag = if *is_mainnet { "mainnet" } else { "testnet" };
        let has_secrets = secrets.as_ref().and_then(|s| s.requestor_networks.get(key)).is_some();
        let status = if active_network == Some(key) {
            "active".green().to_string()
        } else if has_secrets {
            "configured".blue().to_string()
        } else {
            "--".dimmed().to_string()
        };

        println!(
            "  {:<30} {:<10} {}",
            format!("{} ({})", name, chain_id).bold(),
            format!("[{}]", tag).dimmed(),
            status,
        );
    }

    if let Some(ref config) = config {
        for custom in &config.custom_markets {
            let has_secrets =
                secrets.as_ref().and_then(|s| s.requestor_networks.get(&custom.name)).is_some();
            let status = if active_network == Some(custom.name.as_str()) {
                "active".green().to_string()
            } else if has_secrets {
                "configured".blue().to_string()
            } else {
                "--".dimmed().to_string()
            };

            println!(
                "  {:<30} {:<10} {}",
                format!("{} ({})", custom.name, custom.chain_id).bold(),
                "[custom]".dimmed(),
                status,
            );
        }
    }

    println!();
    println!(
        "{} {}",
        "Tip:".bold(),
        "Switch active network with: boundless requestor networks --set \"Taiko Mainnet\"".dimmed()
    );
    println!();
}
