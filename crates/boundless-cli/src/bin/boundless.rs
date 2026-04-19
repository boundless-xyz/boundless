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

//! The Boundless CLI is a command-line interface for interacting with Boundless.

const CLI_LONG_ABOUT: &str = r#"
The Boundless CLI is a command-line interface for interacting with Boundless.
"#;

use anyhow::Result;
use boundless_cli::{
    commands::{prover::ProverCommands, requestor::RequestorCommands, rewards::RewardsCommands},
    config::GlobalConfig,
};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::aot::Shell;
use shadow_rs::shadow;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

shadow!(build);

#[derive(Subcommand, Clone, Debug)]
enum Command {
    /// Commands for requestors submitting proof requests
    #[command(subcommand)]
    Requestor(Box<RequestorCommands>),

    /// Commands for provers executing and fulfilling requests
    #[command(subcommand)]
    Prover(Box<ProverCommands>),

    /// Commands for managing rewards, staking, and PoVW
    #[command(subcommand)]
    Rewards(Box<RewardsCommands>),

    #[command(hide = true)]
    Completions { shell: Shell },
}

#[derive(Parser, Debug)]
#[clap(
    author,
    long_version = build::CLAP_LONG_VERSION,
    about = "CLI for Boundless",
    long_about = CLI_LONG_ABOUT,
    arg_required_else_help = true
)]
struct MainArgs {
    /// Subcommand to run
    #[command(subcommand)]
    command: Command,

    #[command(flatten, next_help_heading = "Global Options")]
    config: GlobalConfig,
}

fn address_from_private_key(private_key: &str) -> Option<String> {
    use alloy::signers::local::PrivateKeySigner;

    let key_str = private_key.strip_prefix("0x").unwrap_or(private_key);

    if let Ok(signer) = key_str.parse::<PrivateKeySigner>() {
        Some(format!("{:?}", signer.address()))
    } else {
        None
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

async fn show_welcome_screen() -> Result<()> {
    use boundless_cli::commands::rewards::State;
    use boundless_cli::config_file::{Config, Secrets};
    use colored::Colorize;

    println!();
    // Cyan gradient logo
    println!("{}", "     .MMMMMMMMMM,".bright_cyan().bold());
    println!("  {}", ",MMMMO     cMMMMMc".cyan().bold());
    println!(" {}", "WMMMMx       .MMMMMM".bright_cyan().bold());
    println!("{}", "0MMMMM'        ;MMMMM0".cyan().bold());
    println!("{}", ":MMMMM.           :MMM".bright_cyan().bold());
    println!("      {}", "0MMMMM;        .MMMMMK".cyan().bold());
    println!("       {}", "MMMMMM.       dMMMMM".bright_cyan().bold());
    println!("        {}", "lMMMMM;     dMMMMc".cyan().bold());
    println!("           {}", "lMMMMMMMMMM:".bright_cyan().bold());
    println!("               {}", "lMM;".cyan().bold());
    println!();

    println!("{}\n", "Boundless CLI - Universal ZK Protocol".bold());

    let config = Config::load().ok();
    let secrets = Secrets::load().ok();

    let requestor_configured = config.as_ref().and_then(|c| c.requestor.as_ref()).is_some();
    let prover_configured = config.as_ref().and_then(|c| c.prover.as_ref()).is_some();
    let rewards_configured = config.as_ref().and_then(|c| c.rewards.as_ref()).is_some();

    // Show Requestor Module
    if requestor_configured {
        let network = config.as_ref().unwrap().requestor.as_ref().unwrap().network.clone();
        let requestor_secrets = secrets.as_ref().and_then(|s| s.requestor_networks.get(&network));

        let display_network = match network.as_str() {
            "base-mainnet" => "Base Mainnet",
            "base-sepolia" => "Base Sepolia",
            "eth-sepolia" => "Ethereum Sepolia",
            custom => custom,
        };

        // Check env var for requestor private key
        let env_requestor_pk = std::env::var("REQUESTOR_PRIVATE_KEY").ok();
        let (requestor_pk, pk_source) = if let Some(ref pk) = env_requestor_pk {
            (Some(pk.as_str()), "env")
        } else {
            (requestor_secrets.and_then(|s| s.private_key.as_deref()), "config")
        };

        if let Some(pk) = requestor_pk {
            println!("{} Requestor Module [{}]", "✓".green().bold(), display_network.blue().bold());
            if let Some(addr) = address_from_private_key(pk) {
                println!(
                    "  Requestor Address: {} (PK: {} {})",
                    addr.green(),
                    "✓".green(),
                    format!("[{}]", pk_source).dimmed()
                );
            }
        } else if let Some(addr) = requestor_secrets.and_then(|s| s.address.as_deref()) {
            println!(
                "{} Requestor Module [{}] {}",
                "✓".yellow().bold(),
                display_network.blue().bold(),
                "(read-only)".yellow()
            );
            println!(
                "  Requestor Address: {} (PK: {} {})",
                addr.green(),
                "✗".yellow(),
                "[config file]".dimmed()
            );
        } else {
            println!(
                "{} Requestor Module [{}] {}",
                "✓".yellow().bold(),
                display_network.blue().bold(),
                "(read-only)".yellow()
            );
        }
        println!(
            "  {} {}",
            "→".cyan(),
            "Run 'boundless requestor --help' to see available commands".dimmed()
        );
    } else {
        println!("{} Requestor Module: {}", "✗".red().bold(), "Not configured".red());
        println!("  {} {}", "→".cyan(), "Run 'boundless requestor setup'".cyan());
    }

    println!();

    // Show Prover Module
    if prover_configured {
        let network = config.as_ref().unwrap().prover.as_ref().unwrap().network.clone();
        let prover_secrets = secrets.as_ref().and_then(|s| s.prover_networks.get(&network));

        let display_network = match network.as_str() {
            "base-mainnet" => "Base Mainnet",
            "base-sepolia" => "Base Sepolia",
            "eth-sepolia" => "Ethereum Sepolia",
            custom => custom,
        };

        // Check env var for prover private key
        let env_prover_pk = std::env::var("PROVER_PRIVATE_KEY").ok();
        let (prover_pk, pk_source) = if let Some(ref pk) = env_prover_pk {
            (Some(pk.as_str()), "env")
        } else {
            (prover_secrets.and_then(|s| s.private_key.as_deref()), "config")
        };

        if let Some(pk) = prover_pk {
            println!("{} Prover Module [{}]", "✓".green().bold(), display_network.blue().bold());
            if let Some(addr) = address_from_private_key(pk) {
                println!(
                    "  Prover Address:    {} (PK: {} {})",
                    addr.green(),
                    "✓".green(),
                    format!("[{}]", pk_source).dimmed()
                );
            }
        } else if let Some(addr) = prover_secrets.and_then(|s| s.address.as_deref()) {
            println!(
                "{} Prover Module [{}] {}",
                "✓".yellow().bold(),
                display_network.blue().bold(),
                "(read-only)".yellow()
            );
            println!(
                "  Prover Address:    {} (PK: {} {})",
                addr.green(),
                "✗".yellow(),
                "[config file]".dimmed()
            );
        } else {
            println!(
                "{} Prover Module [{}] {}",
                "✓".yellow().bold(),
                display_network.blue().bold(),
                "(read-only)".yellow()
            );
        }
        println!(
            "  {} {}",
            "→".cyan(),
            "Run 'boundless prover --help' to see available commands".dimmed()
        );
    } else {
        println!("{} Prover Module: {}", "✗".red().bold(), "Not configured".red());
        println!("  {} {}", "→".cyan(), "Run 'boundless prover setup'".cyan());
    }

    println!();

    if rewards_configured {
        let network = config.as_ref().unwrap().rewards.as_ref().unwrap().network.clone();
        let rewards_secrets = secrets.as_ref().and_then(|s| s.rewards_networks.get(&network));

        let display_network = match network.as_str() {
            "eth-mainnet" => "Ethereum Mainnet",
            "eth-sepolia" => "Ethereum Sepolia",
            custom => custom,
        };

        // Check env vars for staking credentials
        let env_staking_pk = std::env::var("STAKING_PRIVATE_KEY").ok();
        let env_staking_addr = std::env::var("STAKING_ADDRESS").ok();
        let env_reward_pk = std::env::var("REWARD_PRIVATE_KEY").ok();
        let env_reward_addr = std::env::var("REWARD_ADDRESS").ok();

        // Resolve staking credentials (env takes precedence)
        let (staking_pk, staking_pk_source) = if let Some(ref pk) = env_staking_pk {
            (Some(pk.as_str()), "env")
        } else {
            (rewards_secrets.and_then(|s| s.staking_private_key.as_deref()), "config")
        };

        let (staking_addr, staking_addr_source) = if let Some(ref addr) = env_staking_addr {
            (Some(addr.as_str()), "env")
        } else {
            (rewards_secrets.and_then(|s| s.staking_address.as_deref()), "config")
        };

        // Resolve reward credentials (env takes precedence)
        let (reward_pk, reward_pk_source) = if let Some(ref pk) = env_reward_pk {
            (Some(pk.as_str()), "env")
        } else {
            (rewards_secrets.and_then(|s| s.reward_private_key.as_deref()), "config")
        };

        let (reward_addr, reward_addr_source) = if let Some(ref addr) = env_reward_addr {
            (Some(addr.as_str()), "env")
        } else {
            (rewards_secrets.and_then(|s| s.reward_address.as_deref()), "config")
        };

        // Determine if module is in read-only mode (no private keys)
        let has_write_access = staking_pk.is_some() || reward_pk.is_some();

        if has_write_access {
            println!("{} Rewards Module [{}]", "✓".green().bold(), display_network.blue().bold());
        } else {
            println!(
                "{} Rewards Module [{}] {}",
                "✓".yellow().bold(),
                display_network.blue().bold(),
                "(read-only)".yellow()
            );
        }

        // Display staking address
        if let Some(pk) = staking_pk {
            if let Some(addr) = address_from_private_key(pk) {
                println!(
                    "  Staking Address: {} (PK: {} {})",
                    addr.green(),
                    "✓".green(),
                    format!("[{}]", staking_pk_source).dimmed()
                );
            }
        } else if let Some(addr) = staking_addr {
            println!(
                "  Staking Address: {} (PK: {} {})",
                addr.green(),
                "✗".yellow(),
                format!("[{}]", staking_addr_source).dimmed()
            );
        }

        // Display reward address
        if let Some(pk) = reward_pk {
            if let Some(addr) = address_from_private_key(pk) {
                println!(
                    "  Reward Address:  {} (PK: {} {})",
                    addr.green(),
                    "✓".green(),
                    format!("[{}]", reward_pk_source).dimmed()
                );
            }
        } else if let Some(addr) = reward_addr {
            // Check if reward address is same as staking address
            let staking_addr_resolved = staking_pk
                .and_then(address_from_private_key)
                .map(|a| a.to_lowercase())
                .or_else(|| staking_addr.map(|s| s.to_lowercase()));

            let same_as_staking =
                staking_addr_resolved.map(|s| s == addr.to_lowercase()).unwrap_or(false);

            // If same address and we have staking PK, show ✓ for reward too
            let has_pk = same_as_staking && staking_pk.is_some();
            let pk_indicator = if has_pk { "✓".green() } else { "✗".yellow() };

            println!(
                "  Reward Address:  {} (PK: {} {})",
                addr.green(),
                pk_indicator,
                format!("[{}]", reward_addr_source).dimmed()
            );
        }

        // Display mining state file if configured
        let env_mining_state = std::env::var("MINING_STATE_FILE").ok();
        let (mining_state, mining_source) = if let Some(ref state) = env_mining_state {
            (Some(state.as_str()), "env")
        } else {
            (rewards_secrets.and_then(|s| s.mining_state_file.as_deref()), "config")
        };

        if let Some(state_path) = mining_state {
            // Get absolute path for display
            let display_path = std::fs::canonicalize(state_path)
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| {
                    // If canonicalize fails, try to resolve relative to current dir
                    std::env::current_dir()
                        .map(|cwd| cwd.join(state_path).display().to_string())
                        .unwrap_or_else(|_| state_path.to_string())
                });

            println!(
                "  Mining State:    {} {}",
                display_path.green(),
                format!("[{}]", mining_source).dimmed()
            );

            // Try to load and display state statistics
            match State::load(state_path).await {
                Ok(state) => {
                    // Display last updated time
                    if let Ok(elapsed) = state.updated_at.elapsed() {
                        let duration_str = format_duration(elapsed);
                        println!("    Last updated:  {}", duration_str.dimmed());
                    }

                    // Display work log stats
                    if !state.work_log.is_empty() {
                        println!(
                            "    Total PoVW jobs: {}",
                            state.work_log.jobs.len().to_string().cyan()
                        );
                    } else {
                        println!("    Total PoVW jobs: {}", "0 (empty)".yellow());
                    }

                    // Display last on-chain submission from update_transactions
                    if !state.update_transactions.is_empty() {
                        // Find any transaction (they're all in the same timeframe based on updated_at)
                        if let Some((tx_hash, _)) = state.update_transactions.iter().next() {
                            let time_str = state
                                .updated_at
                                .elapsed()
                                .map(format_duration)
                                .unwrap_or_else(|_| "unknown".to_string());
                            println!(
                                "    Last submitted on-chain: {} (tx: {})",
                                time_str.dimmed(),
                                format!("{:#x}", tx_hash).cyan()
                            );
                        }
                    } else {
                        println!("    Last submitted on-chain: {}", "Never submitted".yellow());
                    }
                }
                Err(e) => {
                    // Distinguish between file not found vs invalid file
                    if e.to_string().contains("No such file or directory")
                        || e.to_string().contains("Failed to read work log state file")
                    {
                        println!("    Status:        {} File not found: verify path or run prepare-povw to initialize", "⚠".yellow());
                    } else {
                        println!(
                            "    Status:        {} Invalid file: cannot decode state",
                            "⚠".yellow()
                        );
                    }
                }
            }
        } else {
            println!("  PoVW State:      {} (PoVW disabled)", "✗".yellow());
        }

        // Backwards compatibility: show deprecated private_key field if nothing else is set
        let has_modern_config = staking_pk.is_some()
            || staking_addr.is_some()
            || reward_pk.is_some()
            || reward_addr.is_some();
        if !has_modern_config {
            if let Some(rewards_sec) = rewards_secrets {
                if let Some(ref pk) = rewards_sec.private_key {
                    if let Some(addr) = address_from_private_key(pk) {
                        println!("  Address (deprecated): {}", addr.green());
                    }
                    println!("  Status: {}", "Ready".green());
                } else {
                    println!("  Status: {} (no credentials)", "Read-only".yellow());
                }
            } else {
                println!("  Status: {} (no credentials)", "Read-only".yellow());
            }
        }
        println!(
            "  {} {}",
            "→".cyan(),
            "Run 'boundless rewards --help' to see available commands".dimmed()
        );
    } else {
        println!("{} Rewards Module: {}", "✗".red().bold(), "Not configured".red());
        println!("  {} {}", "→".cyan(), "Run 'boundless rewards setup'".cyan());
    }

    println!();
    println!("💡 {}", "Run 'boundless --help' to see all available commands".dimmed());
    println!();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let raw_args: Vec<String> = std::env::args().collect();

    let args = match MainArgs::try_parse() {
        Ok(args) => args,
        Err(err) => {
            if err.kind() == clap::error::ErrorKind::DisplayHelp {
                // If it's a help request, print the help and exit successfully
                err.print()?;
                return Ok(());
            }
            if err.kind() == clap::error::ErrorKind::DisplayVersion {
                // If it's a version request, print the version and exit successfully
                err.print()?;
                return Ok(());
            }
            // If no subcommand provided
            if err.kind() == clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand {
                // Check if a module was specified
                let has_module = raw_args.len() >= 2
                    && matches!(raw_args[1].as_str(), "requestor" | "prover" | "rewards");

                if has_module {
                    // Module specified without subcommand - show clap's error with help suggestion
                    err.print()?;
                    std::process::exit(1);
                } else {
                    // Top-level command without args - show welcome screen
                    show_welcome_screen().await?;
                    return Ok(());
                }
            }
            // Print error directly to avoid double "Error:" prefix
            err.print()?;
            std::process::exit(1);
        }
    };

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(args.config.log_level.into())
                .from_env_lossy(),
        )
        .init();

    run(&args, &args.config).await
}

pub(crate) async fn run(args: &MainArgs, config: &GlobalConfig) -> Result<()> {
    match &args.command {
        // New command structure
        Command::Requestor(cmd) => cmd.run(config).await,
        Command::Prover(cmd) => cmd.run(config).await,
        Command::Rewards(cmd) => cmd.run(config).await,

        Command::Completions { shell } => generate_shell_completions(shell),
    }
}

fn generate_shell_completions(shell: &Shell) -> Result<()> {
    clap_complete::generate(*shell, &mut MainArgs::command(), "boundless", &mut std::io::stdout());
    Ok(())
}
