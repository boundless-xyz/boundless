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

//! Interactive setup command for the Boundless CLI.

use std::io::Write;

use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;
use inquire::{Confirm, Select, Text};
use risc0_povw::PovwLogId;

use super::custom_networks;
use super::network::{
    normalize_market_network, normalize_rewards_network, query_chain_id, PREBUILT_PROVER_NETWORKS,
    PREBUILT_REQUESTOR_NETWORKS, PREBUILT_REWARDS_NETWORKS,
};
use super::secrets::address_from_private_key;
use crate::commands::rewards::State;
use crate::config::GlobalConfig;
use crate::config_file::{
    Config, ProverConfig, ProverSecrets, RequestorConfig, RequestorSecrets, RewardsConfig,
    RewardsSecrets, Secrets,
};
use crate::display::obscure_url;
use crate::display::DisplayManager;

/// Common setup fields shared across all modules
#[derive(Args, Clone, Debug)]
pub struct CommonSetupFields {
    /// Switch network and load any previously saved configuration for that network
    #[arg(long = "change-network")]
    pub network: Option<String>,

    /// RPC URL for the network
    #[arg(long = "set-rpc-url")]
    pub rpc_url: Option<String>,

    /// Rename a custom network (format: OLD_NAME NEW_NAME)
    #[arg(long = "rename-network", num_args = 2, value_names = ["OLD_NAME", "NEW_NAME"])]
    pub rename_network: Option<Vec<String>>,

    /// Reset secrets for the currently selected network
    #[arg(long = "reset")]
    pub reset: bool,

    /// Reset all configuration and secrets for this module across all networks
    #[arg(long = "reset-all")]
    pub reset_all: bool,
}

/// Market-related setup fields (requestor/prover)
#[derive(Args, Clone, Debug)]
pub struct MarketSetupFields {
    /// Private key for transactions (will be stored in ~/.boundless/secrets.toml)
    #[arg(long = "set-private-key")]
    pub private_key: Option<String>,

    /// Address for read-only mode
    #[arg(long = "set-address")]
    pub address: Option<String>,

    /// BoundlessMarket contract address (custom networks only)
    #[arg(long = "set-boundless-market-address")]
    pub boundless_market_address: Option<String>,

    /// RiscZeroVerifierRouter contract address (custom networks only)
    #[arg(long = "set-verifier-router-address")]
    pub verifier_router_address: Option<String>,

    /// RiscZeroSetVerifier contract address (custom networks only)
    #[arg(long = "set-set-verifier-address")]
    pub set_verifier_address: Option<String>,

    /// Collateral token contract address (custom networks only)
    #[arg(long = "set-collateral-token-address")]
    pub collateral_token_address: Option<String>,

    /// Order stream URL (custom networks only)
    #[arg(long = "set-order-stream-url")]
    pub order_stream_url: Option<String>,
}

/// Rewards-specific setup fields
#[derive(Args, Clone, Debug)]
pub struct RewardsSetupFields {
    /// Staking private key
    #[arg(long = "set-staking-private-key")]
    pub staking_private_key: Option<String>,

    /// Staking address for read-only mode
    #[arg(long = "set-staking-address")]
    pub staking_address: Option<String>,

    /// Reward private key
    #[arg(long = "set-reward-private-key")]
    pub reward_private_key: Option<String>,

    /// Reward address
    #[arg(long = "set-reward-address")]
    pub reward_address: Option<String>,

    /// Beacon API URL
    #[arg(long = "set-beacon-api-url")]
    pub beacon_api_url: Option<String>,

    /// Update only the PoVW state file path
    #[arg(long = "state-file")]
    pub state_file: Option<String>,

    /// ZKC token contract address (custom networks only)
    #[arg(long = "set-zkc-address")]
    pub zkc_address: Option<String>,

    /// veZKC contract address (custom networks only)
    #[arg(long = "set-vezkc-address")]
    pub vezkc_address: Option<String>,

    /// Staking rewards contract address (custom networks only)
    #[arg(long = "set-staking-rewards-address")]
    pub staking_rewards_address: Option<String>,

    /// Mining accounting contract address (custom networks only)
    #[arg(long = "set-mining-accounting-address")]
    pub mining_accounting_address: Option<String>,

    /// Mining mint contract address (custom networks only)
    #[arg(long = "set-mining-mint-address")]
    pub mining_mint_address: Option<String>,
}

/// Requestor setup command
#[derive(Args, Clone, Debug)]
pub struct RequestorSetup {
    /// Common setup fields
    #[command(flatten)]
    pub common: CommonSetupFields,

    /// Market-related setup fields
    #[command(flatten)]
    pub market: MarketSetupFields,
}

/// Prover setup command
#[derive(Args, Clone, Debug)]
pub struct ProverSetup {
    /// Common setup fields
    #[command(flatten)]
    pub common: CommonSetupFields,

    /// Market-related setup fields
    #[command(flatten)]
    pub market: MarketSetupFields,
}

/// Rewards setup command
#[derive(Args, Clone, Debug)]
pub struct RewardsSetup {
    /// Common setup fields
    #[command(flatten)]
    pub common: CommonSetupFields,

    /// Rewards-specific setup fields
    #[command(flatten)]
    pub rewards: RewardsSetupFields,
}

/// Interactive setup command (for `boundless setup`)
#[derive(Args, Clone, Debug)]
pub struct SetupInteractive {
    /// Switch network and load any previously saved configuration for that network
    #[arg(long = "change-network")]
    pub network: Option<String>,

    /// RPC URL for the network
    #[arg(long = "set-rpc-url")]
    pub rpc_url: Option<String>,

    /// Private key for transactions (will be stored in ~/.boundless/secrets.toml)
    #[arg(long = "set-private-key")]
    pub private_key: Option<String>,

    /// Address for read-only mode (requestor/prover modules)
    #[arg(long = "set-address")]
    pub address: Option<String>,

    /// Staking private key (rewards module only)
    #[arg(long = "set-staking-private-key")]
    pub staking_private_key: Option<String>,

    /// Staking address for read-only mode (rewards module only)
    #[arg(long = "set-staking-address")]
    pub staking_address: Option<String>,

    /// Reward private key (rewards module only)
    #[arg(long = "set-reward-private-key")]
    pub reward_private_key: Option<String>,

    /// Reward address (rewards module only)
    #[arg(long = "set-reward-address")]
    pub reward_address: Option<String>,

    /// Beacon API URL (rewards module only)
    #[arg(long = "set-beacon-api-url")]
    pub beacon_api_url: Option<String>,

    /// Update only the PoVW state file path (rewards module only)
    #[arg(long = "state-file")]
    pub state_file: Option<String>,

    /// BoundlessMarket contract address (custom networks only)
    #[arg(long = "set-boundless-market-address")]
    pub boundless_market_address: Option<String>,

    /// RiscZeroVerifierRouter contract address (custom networks only)
    #[arg(long = "set-verifier-router-address")]
    pub verifier_router_address: Option<String>,

    /// RiscZeroSetVerifier contract address (custom networks only)
    #[arg(long = "set-set-verifier-address")]
    pub set_verifier_address: Option<String>,

    /// Collateral token contract address (custom networks only)
    #[arg(long = "set-collateral-token-address")]
    pub collateral_token_address: Option<String>,

    /// Order stream URL (custom networks only)
    #[arg(long = "set-order-stream-url")]
    pub order_stream_url: Option<String>,

    /// ZKC token contract address (custom networks only)
    #[arg(long = "set-zkc-address")]
    pub zkc_address: Option<String>,

    /// veZKC contract address (custom networks only)
    #[arg(long = "set-vezkc-address")]
    pub vezkc_address: Option<String>,

    /// Staking rewards contract address (custom networks only)
    #[arg(long = "set-staking-rewards-address")]
    pub staking_rewards_address: Option<String>,

    /// Mining accounting contract address (custom networks only)
    #[arg(long = "set-mining-accounting-address")]
    pub mining_accounting_address: Option<String>,

    /// Mining mint contract address (custom networks only)
    #[arg(long = "set-mining-mint-address")]
    pub mining_mint_address: Option<String>,

    /// Rename a custom network (format: OLD_NAME NEW_NAME)
    #[arg(long = "rename-network", num_args = 2, value_names = ["OLD_NAME", "NEW_NAME"])]
    pub rename_network: Option<Vec<String>>,

    /// Reset secrets for the currently selected network
    #[arg(long = "reset")]
    pub reset: bool,

    /// Reset all configuration and secrets for this module across all networks
    #[arg(long = "reset-all")]
    pub reset_all: bool,
}

impl RequestorSetup {
    /// Run requestor setup
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let setup = SetupInteractive {
            network: self.common.network.clone(),
            rpc_url: self.common.rpc_url.clone(),
            private_key: self.market.private_key.clone(),
            address: self.market.address.clone(),
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: self.market.boundless_market_address.clone(),
            verifier_router_address: self.market.verifier_router_address.clone(),
            set_verifier_address: self.market.set_verifier_address.clone(),
            collateral_token_address: self.market.collateral_token_address.clone(),
            order_stream_url: self.market.order_stream_url.clone(),
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: self.common.rename_network.clone(),
            reset: self.common.reset,
            reset_all: self.common.reset_all,
        };
        setup.run_module(global_config, "requestor").await
    }
}

impl ProverSetup {
    /// Run prover setup
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let setup = SetupInteractive {
            network: self.common.network.clone(),
            rpc_url: self.common.rpc_url.clone(),
            private_key: self.market.private_key.clone(),
            address: self.market.address.clone(),
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: self.market.boundless_market_address.clone(),
            verifier_router_address: self.market.verifier_router_address.clone(),
            set_verifier_address: self.market.set_verifier_address.clone(),
            collateral_token_address: self.market.collateral_token_address.clone(),
            order_stream_url: self.market.order_stream_url.clone(),
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: self.common.rename_network.clone(),
            reset: self.common.reset,
            reset_all: self.common.reset_all,
        };
        setup.run_module(global_config, "prover").await
    }
}

impl RewardsSetup {
    /// Run rewards setup
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let setup = SetupInteractive {
            network: self.common.network.clone(),
            rpc_url: self.common.rpc_url.clone(),
            private_key: None,
            address: None,
            staking_private_key: self.rewards.staking_private_key.clone(),
            staking_address: self.rewards.staking_address.clone(),
            reward_private_key: self.rewards.reward_private_key.clone(),
            reward_address: self.rewards.reward_address.clone(),
            beacon_api_url: self.rewards.beacon_api_url.clone(),
            state_file: self.rewards.state_file.clone(),
            boundless_market_address: None,
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: self.rewards.zkc_address.clone(),
            vezkc_address: self.rewards.vezkc_address.clone(),
            staking_rewards_address: self.rewards.staking_rewards_address.clone(),
            mining_accounting_address: self.rewards.mining_accounting_address.clone(),
            mining_mint_address: self.rewards.mining_mint_address.clone(),
            rename_network: self.common.rename_network.clone(),
            reset: self.common.reset,
            reset_all: self.common.reset_all,
        };
        setup.run_module(global_config, "rewards").await
    }
}

impl SetupInteractive {
    /// Print a section header with consistent styling
    fn print_section_header(display: &DisplayManager, title: &str) {
        display.header(&format!("\n{}", title));
    }

    /// Run setup for a specific module
    pub async fn run_module(&self, _global_config: &GlobalConfig, module: &str) -> Result<()> {
        let display = DisplayManager::new();
        display.header("\n🔧 Boundless CLI Setup");

        let mut config = Config::load().unwrap_or_default();
        let mut secrets = Secrets::load().unwrap_or_default();

        let work_done = match module {
            "requestor" => self.setup_requestor(&mut config, &mut secrets).await?,
            "prover" => self.setup_prover(&mut config, &mut secrets).await?,
            "rewards" => self.setup_rewards(&mut config, &mut secrets).await?,
            _ => unreachable!(),
        };

        if work_done {
            config.save()?;
            display.info(&format!(
                "Configuration saved to {}",
                Config::path()?.display().to_string().cyan()
            ));

            if !secrets.requestor_networks.is_empty()
                || !secrets.prover_networks.is_empty()
                || !secrets.rewards_networks.is_empty()
            {
                secrets.save()?;
                display.info(&format!(
                    "Secrets saved to {} (permissions: 600)",
                    Secrets::path()?.display().to_string().cyan()
                ));
                display.warning(
                    "Secrets are stored in plaintext. Consider using environment variables instead.",
                );
            }

            display.success(&format!(
                "Setup complete! Run `boundless {} config` to see your configuration.",
                module
            ));
        }

        Ok(())
    }

    async fn setup_requestor(&self, config: &mut Config, secrets: &mut Secrets) -> Result<bool> {
        let display = DisplayManager::new();
        Self::print_section_header(&display, "Requestor Module Setup");

        // Handle reset-all mode
        if self.reset_all {
            let network_count = secrets.requestor_networks.len();
            let networks: Vec<String> = secrets.requestor_networks.keys().cloned().collect();

            config.requestor = None;
            secrets.requestor_networks.clear();

            display.success("Cleared all requestor configuration");
            if network_count > 0 {
                display.success(&format!("Removed secrets for {} network(s):", network_count));
                for network in networks {
                    display.note(&format!("  - {}", network));
                }
            }
            return Ok(true);
        }

        // Handle reset mode (current network only)
        if self.reset {
            let work_done = if let Some(ref requestor) = config.requestor {
                let network_name = &requestor.network;
                if let Some(removed) = secrets.requestor_networks.remove(network_name) {
                    display.success(&format!(
                        "Cleared secrets for network: {}",
                        network_name.cyan().bold()
                    ));
                    if removed.rpc_url.is_some() {
                        display.note("  - RPC URL");
                    }
                    if removed.private_key.is_some() {
                        display.note("  - Private key");
                    }
                    if removed.address.is_some() {
                        display.note("  - Address");
                    }
                    true
                } else {
                    display.warning(&format!("No secrets found for network: {}", network_name));
                    false
                }
            } else {
                display.warning("No network configured to reset");
                display.note("Run 'boundless requestor setup' to configure a network first");
                false
            };
            return Ok(work_done);
        }

        // Step 1: Handle --change-network first (determine target network)
        let target_network =
            self.network.as_ref().map(|network| normalize_market_network(network).to_string());

        // Step 2: If --change-network was provided, switch to that network
        if let Some(ref network_name) = target_network {
            config.requestor = Some(RequestorConfig { network: network_name.clone() });
            display.success(&format!("Switched to network: {}", network_name.cyan().bold()));
        }

        // Step 3: Check if we're in non-interactive mode (has flags to apply)
        let non_interactive = self.rpc_url.is_some()
            || self.private_key.is_some()
            || self.address.is_some()
            || self.boundless_market_address.is_some()
            || self.verifier_router_address.is_some()
            || self.set_verifier_address.is_some()
            || self.collateral_token_address.is_some()
            || self.order_stream_url.is_some();

        if non_interactive {
            // Non-interactive mode: update existing network or create new one
            if let Some(ref existing_config) = config.requestor {
                // Network already selected - update secrets for that network
                let mut network_name = existing_config.network.clone();

                // Check if user is trying to modify a pre-built network
                let has_contract_address_flags = self.boundless_market_address.is_some()
                    || self.verifier_router_address.is_some()
                    || self.set_verifier_address.is_some()
                    || self.collateral_token_address.is_some()
                    || self.order_stream_url.is_some();

                if PREBUILT_REQUESTOR_NETWORKS.contains(&network_name.as_str())
                    && has_contract_address_flags
                {
                    // Clone pre-built network as custom with user's overrides
                    let custom_network = custom_networks::clone_prebuilt_market_as_custom(
                        config,
                        &network_name,
                        self,
                    )?;
                    let custom_name = custom_network.name.clone();
                    config.custom_markets.push(custom_network);

                    // Switch to the new custom network
                    config.requestor = Some(RequestorConfig { network: custom_name.clone() });

                    display.success(&format!(
                        "Created new custom network '{}' based on '{}'",
                        custom_name.cyan().bold(),
                        network_name
                    ));
                    display.note("Pre-built networks cannot be modified. A new custom network was created instead.");

                    // Update network_name to point to the new custom network
                    network_name = custom_name;
                }

                // Get existing secrets or create new entry
                let existing_secrets =
                    secrets.requestor_networks.get(&network_name).cloned().unwrap_or(
                        RequestorSecrets { rpc_url: None, private_key: None, address: None },
                    );

                // Merge with provided values (only overwrite if provided)
                let rpc_url = if let Some(ref url) = self.rpc_url {
                    display.success("Updated RPC URL");
                    Some(url.clone())
                } else {
                    existing_secrets.rpc_url
                };

                let (private_key, address) = if let Some(ref pk) = self.private_key {
                    let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                    let addr = address_from_private_key(&pk);
                    display.success("Updated private key");
                    (Some(pk), addr.map(|a| format!("{:#x}", a)))
                } else if let Some(ref addr) = self.address {
                    display.success("Updated address");
                    (existing_secrets.private_key, Some(addr.clone()))
                } else {
                    (existing_secrets.private_key, existing_secrets.address)
                };

                secrets.requestor_networks.insert(
                    network_name.clone(),
                    RequestorSecrets { rpc_url, private_key, address },
                );

                // Update contract addresses if this is a custom network and flags are provided
                if network_name.starts_with("custom-") {
                    custom_networks::update_custom_market_addresses(config, &network_name, self)?;
                }

                display.success(&format!(
                    "Updated requestor configuration for network: {}",
                    network_name.cyan().bold()
                ));
                return Ok(true);
            } else {
                // No network selected - need to create custom network
                if let Some(ref rpc_url) = self.rpc_url {
                    // Query chain ID from RPC
                    display.info("Querying chain ID from RPC...");
                    let chain_id = query_chain_id(rpc_url).await?;
                    display.success(&format!("Chain ID: {}", chain_id.to_string().bright_white()));

                    let network_name = format!("custom-{}", chain_id);

                    // Check if this custom network already exists
                    if !config.custom_markets.iter().any(|m| m.name == network_name) {
                        // Create minimal custom network
                        let custom_network =
                            custom_networks::create_minimal_custom_network(chain_id);
                        config.custom_markets.push(custom_network);
                    }

                    // Set as selected network
                    config.requestor = Some(RequestorConfig { network: network_name.clone() });

                    // Extract address from private key if provided, or use explicit address
                    let (private_key, address) = if let Some(ref pk) = self.private_key {
                        let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                        let addr = address_from_private_key(&pk);
                        (Some(pk), addr.map(|a| format!("{:#x}", a)))
                    } else if let Some(ref addr) = self.address {
                        (None, Some(addr.clone()))
                    } else {
                        (None, None)
                    };

                    // Save secrets
                    secrets.requestor_networks.insert(
                        network_name.clone(),
                        RequestorSecrets { rpc_url: Some(rpc_url.clone()), private_key, address },
                    );

                    // Update contract addresses if flags are provided
                    let has_contract_addresses = custom_networks::update_custom_market_addresses(
                        config,
                        &network_name,
                        self,
                    )?;

                    display.success(&format!(
                        "Created custom network '{}' (chain ID: {})",
                        network_name.cyan().bold(),
                        chain_id.to_string().bright_white()
                    ));

                    if !has_contract_addresses {
                        display.warning("Contract addresses not configured. Configure them by:");
                        display.note("• Running 'boundless requestor setup' interactively");
                        display.note(
                            "• Using command-line flags (--set-boundless-market-address, etc.)",
                        );
                        display.note("• Editing ~/.boundless/config.toml directly");
                    }

                    return Ok(true);
                } else {
                    bail!("No network configured. Please provide --set-rpc-url to create a custom network, or run 'boundless requestor setup' interactively to select a network");
                }
            }
        }

        // Step 4: If --change-network was provided but no other flags, check if we should load existing config
        if let Some(network_name) = &target_network {
            // Check if this network has existing secrets
            if let Some(existing) = secrets.requestor_networks.get(network_name) {
                // Network already configured - load it and display config
                display.success(&format!(
                    "Loaded existing configuration for network: {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing.address {
                    display.note(&format!("Address: {}", addr.bright_yellow()));
                }
                return Ok(true);
            } else {
                // Network not configured - continue to interactive setup
                display.info(&format!(
                    "Setting up new configuration for network: {}",
                    network_name.cyan().bold()
                ));
                // Fall through to interactive setup below
            }
        }

        // Step 5: Fully interactive mode - prompt for network selection
        let network_name = if let Some(network) = target_network {
            // Network was specified via --change-network but doesn't exist yet
            // We already set it in Step 2, just use it
            network
        } else {
            // No --change-network provided - prompt user to select network
            let mut network_options = vec!["Base Mainnet", "Base Sepolia", "Ethereum Sepolia"];

            for custom_market in &config.custom_markets {
                network_options.push(&custom_market.name);
            }
            network_options.push("Custom (add new)");

            let network =
                Select::new("Select Boundless Market network:", network_options).prompt()?;

            let selected_network = match network {
                "Base Mainnet" => "base-mainnet".to_string(),
                "Base Sepolia" => "base-sepolia".to_string(),
                "Ethereum Sepolia" => "eth-sepolia".to_string(),
                "Custom (add new)" => {
                    let custom = custom_networks::setup_custom_market()?;
                    let name = custom.name.clone();
                    config.custom_markets.push(custom);
                    name
                }
                custom => custom.to_string(),
            };

            config.requestor = Some(RequestorConfig { network: selected_network.clone() });
            selected_network
        };

        let existing = secrets.requestor_networks.get(&network_name).cloned();

        if let Some(ref existing_config) = existing {
            if self.rpc_url.is_none() && self.private_key.is_none() {
                display.info(&format!(
                    "Previous configuration found for {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing_config.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing_config.address {
                    display.note(&format!("Address: {}", addr.bright_yellow()));
                }
            }
        }

        let rpc_url = if let Some(ref url) = self.rpc_url {
            display.success(&format!("Using RPC URL: {}", url.dimmed()));
            url.clone()
        } else {
            let mut prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(prev) = existing.as_ref().and_then(|e| e.rpc_url.as_ref()) {
                prompt = prompt.with_default(prev);
            }

            prompt.prompt()?
        };

        let (private_key, address) = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            display.success("Using provided private key");
            (Some(pk), None)
        } else {
            let has_previous_key = existing.as_ref().and_then(|e| e.private_key.as_ref()).is_some();
            let private_key_prompt = Confirm::new("Do you want to store a private key?")
                .with_default(has_previous_key)
                .with_help_message(
                    "Required for write operations. If no, you can set REQUESTOR_PRIVATE_KEY env variable instead.",
                )
                .prompt()?;

            if private_key_prompt {
                let help_msg = if has_previous_key {
                    "Leave empty to keep existing key or enter new key"
                } else {
                    "Will be stored in plaintext in ~/.boundless/secrets.toml"
                };

                let pk = Text::new("Enter private key:").with_help_message(help_msg).prompt()?;

                if pk.is_empty() && has_previous_key {
                    let prev_key = existing.as_ref().and_then(|e| e.private_key.clone()).unwrap();
                    (Some(prev_key), None)
                } else {
                    let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                    (Some(pk), None)
                }
            } else {
                display.success("You can set REQUESTOR_PRIVATE_KEY environment variable later for write operations");

                let has_previous_address =
                    existing.as_ref().and_then(|e| e.address.as_ref()).is_some();
                let address_prompt =
                    Confirm::new("Do you want to store an address for read-only mode?")
                        .with_default(has_previous_address)
                        .with_help_message(
                            "Allows you to monitor a requestor address without the private key",
                        )
                        .prompt()?;

                if address_prompt {
                    let mut addr_prompt =
                        Text::new("Enter requestor address:").with_help_message("e.g., 0x1234...");

                    if let Some(prev_addr) = existing.as_ref().and_then(|e| e.address.as_ref()) {
                        addr_prompt = addr_prompt.with_default(prev_addr);
                    }

                    let addr = addr_prompt.prompt()?;
                    (None, Some(addr))
                } else {
                    (None, None)
                }
            }
        };

        secrets.requestor_networks.insert(
            network_name,
            RequestorSecrets { rpc_url: Some(rpc_url), private_key, address },
        );

        Ok(true)
    }

    async fn setup_prover(&self, config: &mut Config, secrets: &mut Secrets) -> Result<bool> {
        let display = DisplayManager::new();
        Self::print_section_header(&display, "Prover Module Setup");

        // Handle reset-all mode
        if self.reset_all {
            let network_count = secrets.prover_networks.len();
            let networks: Vec<String> = secrets.prover_networks.keys().cloned().collect();

            config.prover = None;
            secrets.prover_networks.clear();

            display.success("Cleared all prover configuration");
            if network_count > 0 {
                display.success(&format!("Removed secrets for {} network(s):", network_count));
                for network in networks {
                    display.note(&format!("  - {}", network));
                }
            }
            return Ok(true);
        }

        // Handle reset mode (current network only)
        if self.reset {
            let work_done = if let Some(ref prover) = config.prover {
                let network_name = &prover.network;
                if let Some(removed) = secrets.prover_networks.remove(network_name) {
                    display.success(&format!(
                        "Cleared secrets for network: {}",
                        network_name.cyan().bold()
                    ));
                    if removed.rpc_url.is_some() {
                        display.note("  - RPC URL");
                    }
                    if removed.private_key.is_some() {
                        display.note("  - Private key");
                    }
                    if removed.address.is_some() {
                        display.note("  - Address");
                    }
                    true
                } else {
                    display.warning(&format!("No secrets found for network: {}", network_name));
                    false
                }
            } else {
                display.warning("No network configured to reset");
                display.note("Run 'boundless prover setup' to configure a network first");
                false
            };
            return Ok(work_done);
        }

        // Step 1: Handle --change-network first (determine target network)
        let target_network =
            self.network.as_ref().map(|network| normalize_market_network(network).to_string());

        // Step 2: If --change-network was provided, switch to that network
        if let Some(ref network_name) = target_network {
            config.prover = Some(ProverConfig { network: network_name.clone() });
            display.success(&format!("Switched to network: {}", network_name.cyan().bold()));
        }

        // Step 3: Check if we're in non-interactive mode (has flags to apply)
        let non_interactive = self.rpc_url.is_some()
            || self.private_key.is_some()
            || self.address.is_some()
            || self.boundless_market_address.is_some()
            || self.verifier_router_address.is_some()
            || self.set_verifier_address.is_some()
            || self.collateral_token_address.is_some()
            || self.order_stream_url.is_some();

        if non_interactive {
            // Non-interactive mode: update existing network or create new one
            if let Some(ref existing_config) = config.prover {
                // Network already selected - update secrets for that network
                let mut network_name = existing_config.network.clone();

                // Check if user is trying to modify a pre-built network
                let has_contract_address_flags = self.boundless_market_address.is_some()
                    || self.verifier_router_address.is_some()
                    || self.set_verifier_address.is_some()
                    || self.collateral_token_address.is_some()
                    || self.order_stream_url.is_some();

                if PREBUILT_PROVER_NETWORKS.contains(&network_name.as_str())
                    && has_contract_address_flags
                {
                    // Clone pre-built network as custom with user's overrides
                    let custom_network = custom_networks::clone_prebuilt_market_as_custom(
                        config,
                        &network_name,
                        self,
                    )?;
                    let custom_name = custom_network.name.clone();
                    config.custom_markets.push(custom_network);

                    // Switch to the new custom network
                    config.prover = Some(ProverConfig { network: custom_name.clone() });

                    display.success(&format!(
                        "Created new custom network '{}' based on '{}'",
                        custom_name.cyan().bold(),
                        network_name
                    ));
                    display.note("Pre-built networks cannot be modified. A new custom network was created instead.");

                    // Update network_name to point to the new custom network
                    network_name = custom_name;
                }

                // Get existing secrets or create new entry
                let existing_secrets = secrets
                    .prover_networks
                    .get(&network_name)
                    .cloned()
                    .unwrap_or(ProverSecrets { rpc_url: None, private_key: None, address: None });

                // Merge with provided values (only overwrite if provided)
                let rpc_url = if let Some(ref url) = self.rpc_url {
                    display.success("Updated RPC URL");
                    Some(url.clone())
                } else {
                    existing_secrets.rpc_url
                };

                let (private_key, address) = if let Some(ref pk) = self.private_key {
                    let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                    let addr = address_from_private_key(&pk);
                    display.success("Updated private key");
                    (Some(pk), addr.map(|a| format!("{:#x}", a)))
                } else if let Some(ref addr) = self.address {
                    display.success("Updated address");
                    (existing_secrets.private_key, Some(addr.clone()))
                } else {
                    (existing_secrets.private_key, existing_secrets.address)
                };

                secrets
                    .prover_networks
                    .insert(network_name.clone(), ProverSecrets { rpc_url, private_key, address });

                // Update contract addresses if this is a custom network and flags are provided
                if network_name.starts_with("custom-") {
                    custom_networks::update_custom_market_addresses(config, &network_name, self)?;
                }

                display.success(&format!(
                    "Updated prover configuration for network: {}",
                    network_name.cyan().bold()
                ));
                return Ok(true);
            } else {
                // No network selected - need to create custom network
                if let Some(ref rpc_url) = self.rpc_url {
                    // Query chain ID from RPC
                    display.info("Querying chain ID from RPC...");
                    let chain_id = query_chain_id(rpc_url).await?;
                    display.success(&format!("Chain ID: {}", chain_id.to_string().bright_white()));

                    let network_name = format!("custom-{}", chain_id);

                    // Check if this custom network already exists
                    if !config.custom_markets.iter().any(|m| m.name == network_name) {
                        // Create minimal custom network
                        let custom_network =
                            custom_networks::create_minimal_custom_network(chain_id);
                        config.custom_markets.push(custom_network);
                    }

                    // Set as selected network
                    config.prover = Some(ProverConfig { network: network_name.clone() });

                    // Extract address from private key if provided, or use explicit address
                    let (private_key, address) = if let Some(ref pk) = self.private_key {
                        let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                        let addr = address_from_private_key(&pk);
                        (Some(pk), addr.map(|a| format!("{:#x}", a)))
                    } else if let Some(ref addr) = self.address {
                        (None, Some(addr.clone()))
                    } else {
                        (None, None)
                    };

                    // Save secrets
                    secrets.prover_networks.insert(
                        network_name.clone(),
                        ProverSecrets { rpc_url: Some(rpc_url.clone()), private_key, address },
                    );

                    // Update contract addresses if flags are provided
                    let has_contract_addresses = custom_networks::update_custom_market_addresses(
                        config,
                        &network_name,
                        self,
                    )?;

                    display.success(&format!(
                        "Created custom network '{}' (chain ID: {})",
                        network_name.cyan().bold(),
                        chain_id.to_string().bright_white()
                    ));

                    if !has_contract_addresses {
                        display.warning("Contract addresses not configured. Configure them by:");
                        display.note("• Running 'boundless prover setup' interactively");
                        display.note(
                            "• Using command-line flags (--set-boundless-market-address, etc.)",
                        );
                        display.note("• Editing ~/.boundless/config.toml directly");
                    }

                    return Ok(true);
                } else {
                    bail!("No network configured. Please provide --prover-rpc-url to create a custom network, or run 'boundless prover setup' interactively to select a network");
                }
            }
        }

        // Step 4: If --change-network was provided but no other flags, check if we should load existing config
        if let Some(network_name) = &target_network {
            // Check if this network has existing secrets
            if let Some(existing) = secrets.prover_networks.get(network_name) {
                // Network already configured - load it and display config
                display.success(&format!(
                    "Loaded existing configuration for network: {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing.address {
                    display.note(&format!("Address: {}", addr.bright_yellow()));
                }
                return Ok(true);
            } else {
                // Network not configured - continue to interactive setup
                display.info(&format!(
                    "Setting up new configuration for network: {}",
                    network_name.cyan().bold()
                ));
                // Fall through to interactive setup below
            }
        }

        // Step 5: Fully interactive mode - prompt for network selection
        let network_name = if let Some(network) = target_network {
            // Network was specified via --change-network but doesn't exist yet
            // We already set it in Step 2, just use it
            network
        } else {
            let mut network_options = vec!["Base Mainnet", "Base Sepolia", "Ethereum Sepolia"];

            for custom_market in &config.custom_markets {
                network_options.push(&custom_market.name);
            }
            network_options.push("Custom (add new)");

            // No --change-network provided - prompt user to select network
            let network =
                Select::new("Select Boundless Market network:", network_options).prompt()?;

            let selected_network = match network {
                "Base Mainnet" => "base-mainnet".to_string(),
                "Base Sepolia" => "base-sepolia".to_string(),
                "Ethereum Sepolia" => "eth-sepolia".to_string(),
                "Custom (add new)" => {
                    let custom = custom_networks::setup_custom_market()?;
                    let name = custom.name.clone();
                    config.custom_markets.push(custom);
                    name
                }
                custom => custom.to_string(),
            };

            config.prover = Some(ProverConfig { network: selected_network.clone() });
            selected_network
        };

        let existing = secrets.prover_networks.get(&network_name).cloned();

        if let Some(ref existing_config) = existing {
            if self.rpc_url.is_none() && self.private_key.is_none() {
                display.info(&format!(
                    "Previous configuration found for {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing_config.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing_config.address {
                    display.note(&format!("Address: {}", addr.bright_yellow()));
                }
            }
        }

        let rpc_url = if let Some(ref url) = self.rpc_url {
            display.success(&format!("Using RPC URL: {}", url.dimmed()));
            url.clone()
        } else {
            let mut prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(prev) = existing.as_ref().and_then(|e| e.rpc_url.as_ref()) {
                prompt = prompt.with_default(prev);
            }

            prompt.prompt()?
        };

        let (private_key, address) = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            display.success("Using provided private key");
            (Some(pk), None)
        } else {
            let has_previous_key = existing.as_ref().and_then(|e| e.private_key.as_ref()).is_some();
            let private_key_prompt = Confirm::new("Do you want to store a private key?")
                .with_default(has_previous_key)
                .with_help_message(
                    "Required for write operations. If no, you can set PROVER_PRIVATE_KEY env variable instead.",
                )
                .prompt()?;

            if private_key_prompt {
                let help_msg = if has_previous_key {
                    "Leave empty to keep existing key or enter new key"
                } else {
                    "Will be stored in plaintext in ~/.boundless/secrets.toml"
                };

                let pk = Text::new("Enter private key:").with_help_message(help_msg).prompt()?;

                if pk.is_empty() && has_previous_key {
                    let prev_key = existing.as_ref().and_then(|e| e.private_key.clone()).unwrap();
                    (Some(prev_key), None)
                } else {
                    let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                    (Some(pk), None)
                }
            } else {
                display.success("You can set PROVER_PRIVATE_KEY environment variable later for write operations");

                let has_previous_address =
                    existing.as_ref().and_then(|e| e.address.as_ref()).is_some();
                let address_prompt =
                    Confirm::new("Do you want to store an address for read-only mode?")
                        .with_default(has_previous_address)
                        .with_help_message(
                            "Allows you to monitor a prover address without the private key",
                        )
                        .prompt()?;

                if address_prompt {
                    let mut addr_prompt =
                        Text::new("Enter prover address:").with_help_message("e.g., 0x1234...");

                    if let Some(prev_addr) = existing.as_ref().and_then(|e| e.address.as_ref()) {
                        addr_prompt = addr_prompt.with_default(prev_addr);
                    }

                    let addr = addr_prompt.prompt()?;
                    (None, Some(addr))
                } else {
                    (None, None)
                }
            }
        };

        secrets
            .prover_networks
            .insert(network_name, ProverSecrets { rpc_url: Some(rpc_url), private_key, address });

        Ok(true)
    }

    async fn update_state_file_only(
        &self,
        secrets: &mut Secrets,
        network_name: &str,
        new_state_file_path: &str,
    ) -> Result<bool> {
        // Get existing rewards config for this network
        let existing = secrets.rewards_networks.get_mut(network_name).with_context(|| {
            format!(
                "No existing rewards configuration found for network: {}\n\n\
                Please run 'boundless rewards setup' first to configure the rewards module.",
                network_name
            )
        })?;

        // Expand ~ to home directory
        let expanded_path = if new_state_file_path.starts_with("~/") {
            let home = dirs::home_dir().context("Failed to get home directory")?;
            home.join(new_state_file_path.strip_prefix("~/").unwrap()).display().to_string()
        } else {
            new_state_file_path.to_string()
        };

        // Get absolute path for display
        let abs_path = std::fs::canonicalize(&expanded_path).unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded_path))
                .unwrap_or_else(|_| std::path::PathBuf::from(&expanded_path))
        });

        // Check if state file exists
        let file_exists = std::path::Path::new(&expanded_path).exists();

        if !file_exists {
            // Create new state file
            let reward_addr = existing.reward_address.as_ref()
                .context("Cannot create state file: no reward address configured.\n\nPlease run 'boundless rewards setup' first.")?;

            let log_id = reward_addr
                .parse::<PovwLogId>()
                .context("Failed to parse reward address as PoVW log ID")?;

            // Create parent directory if it doesn't exist
            if let Some(parent) = std::path::Path::new(&expanded_path).parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
            }

            // Create and save empty state file
            let empty_state = State::new(log_id);
            empty_state
                .save(&expanded_path)
                .with_context(|| format!("Failed to create state file: {}", abs_path.display()))?;

            println!(
                "\n✓ Created new PoVW state file at: {}",
                abs_path.display().to_string().cyan()
            );
            println!("  Log ID: {:x}", log_id);
            println!("  Remember to back this up regularly!");
        } else {
            // Validate existing state file's log_id matches the reward address
            if let Some(ref reward_addr) = existing.reward_address {
                print!("Validating state file matches reward address... ");
                std::io::stdout().flush()?;

                match State::load(&expanded_path).await {
                    Ok(state) => {
                        let reward_log_id = reward_addr
                            .parse::<PovwLogId>()
                            .context("Failed to parse reward address as PoVW log ID")?;

                        if state.log_id != reward_log_id {
                            println!("{}", "✗".red());
                            bail!(
                                "PoVW state file log ID mismatch!\n\n\
                                State file log ID: {:x}\n\
                                Reward address:    {}\n\n\
                                Each reward address must have its own unique state file.",
                                state.log_id,
                                reward_addr
                            );
                        }
                        println!("{}", "✓".green());
                    }
                    Err(e) => {
                        println!("{}", "✗".red());
                        bail!("Failed to load state file: {}\n\nPlease check the file is a valid PoVW state file.", e);
                    }
                }
            }

            println!("\n✓ State file validated successfully");
        }

        // Update the state file path
        existing.mining_state_file = Some(expanded_path.clone());

        if !file_exists {
            // Already printed message about creating new file
        } else {
            println!("  Updated PoVW state file path to: {}", expanded_path.cyan());
        }

        Ok(true)
    }

    async fn setup_rewards(&self, config: &mut Config, secrets: &mut Secrets) -> Result<bool> {
        let display = DisplayManager::new();
        Self::print_section_header(&display, "Rewards Module Setup");

        // Handle reset-all mode
        if self.reset_all {
            let network_count = secrets.rewards_networks.len();
            let networks: Vec<String> = secrets.rewards_networks.keys().cloned().collect();

            config.rewards = None;
            secrets.rewards_networks.clear();

            display.success("Cleared all rewards configuration");
            if network_count > 0 {
                display.success(&format!("Removed secrets for {} network(s):", network_count));
                for network in networks {
                    display.note(&format!("  - {}", network));
                }
            }
            return Ok(true);
        }

        // Handle reset mode (current network only)
        if self.reset {
            let work_done = if let Some(ref rewards) = config.rewards {
                let network_name = &rewards.network;
                if let Some(removed) = secrets.rewards_networks.remove(network_name) {
                    display.success(&format!(
                        "Cleared secrets for network: {}",
                        network_name.cyan().bold()
                    ));
                    if removed.rpc_url.is_some() {
                        display.note("  - RPC URL");
                    }
                    if removed.staking_private_key.is_some() {
                        display.note("  - Staking private key");
                    }
                    if removed.staking_address.is_some() {
                        display.note("  - Staking address");
                    }
                    if removed.reward_private_key.is_some() {
                        display.note("  - Reward private key");
                    }
                    if removed.reward_address.is_some() {
                        display.note("  - Reward address");
                    }
                    if removed.mining_state_file.is_some() {
                        display.note("  - PoVW state file path");
                    }
                    if removed.beacon_api_url.is_some() {
                        display.note("  - Beacon API URL");
                    }
                    true
                } else {
                    display.warning(&format!("No secrets found for network: {}", network_name));
                    false
                }
            } else {
                display.warning("No network configured to reset");
                display.note("Run 'boundless rewards setup' to configure a network first");
                false
            };
            return Ok(work_done);
        }

        // Step 1: Handle --change-network first (determine target network)
        let target_network =
            self.network.as_ref().map(|network| normalize_rewards_network(network).to_string());

        // Step 2: If --change-network was provided, switch to that network
        if let Some(ref network_name) = target_network {
            config.rewards = Some(RewardsConfig { network: network_name.clone() });
            display.success(&format!("Switched to network: {}", network_name.cyan().bold()));
        }

        // Step 3: Check if we're in non-interactive mode (has flags to apply)
        let non_interactive = self.rpc_url.is_some()
            || self.staking_private_key.is_some()
            || self.staking_address.is_some()
            || self.reward_private_key.is_some()
            || self.reward_address.is_some()
            || self.beacon_api_url.is_some()
            || self.state_file.is_some()
            || self.zkc_address.is_some()
            || self.vezkc_address.is_some()
            || self.staking_rewards_address.is_some()
            || self.mining_accounting_address.is_some()
            || self.mining_mint_address.is_some();

        if non_interactive {
            // Non-interactive mode: update existing network or create new one
            if let Some(ref existing_config) = config.rewards {
                // Network already selected - update secrets for that network
                let mut network_name = existing_config.network.clone();
                display.success(&format!(
                    "Updating configuration for network: {}",
                    network_name.cyan().bold()
                ));

                // If --state-file is provided, do a quick update and return
                if let Some(ref new_state_file) = self.state_file {
                    return self
                        .update_state_file_only(secrets, &network_name, new_state_file)
                        .await;
                }

                // Check if user is trying to modify a pre-built network
                let has_contract_address_flags = self.zkc_address.is_some()
                    || self.vezkc_address.is_some()
                    || self.staking_rewards_address.is_some()
                    || self.mining_accounting_address.is_some()
                    || self.mining_mint_address.is_some();

                if PREBUILT_REWARDS_NETWORKS.contains(&network_name.as_str())
                    && has_contract_address_flags
                {
                    // Clone pre-built network as custom with user's overrides
                    let custom_network = custom_networks::clone_prebuilt_rewards_as_custom(
                        config,
                        &network_name,
                        self,
                    )?;
                    let custom_name = custom_network.name.clone();
                    config.custom_rewards.push(custom_network);

                    // Switch to the new custom network
                    config.rewards = Some(RewardsConfig { network: custom_name.clone() });

                    display.success(&format!(
                        "Created new custom network '{}' based on '{}'",
                        custom_name.cyan().bold(),
                        network_name
                    ));
                    display.note("Pre-built networks cannot be modified. A new custom network was created instead.");

                    // Update network_name to point to the new custom network
                    network_name = custom_name;
                }

                // Get existing secrets or create new entry
                let existing_secrets =
                    secrets.rewards_networks.get(&network_name).cloned().unwrap_or(
                        RewardsSecrets {
                            rpc_url: None,
                            staking_private_key: None,
                            staking_address: None,
                            reward_private_key: None,
                            reward_address: None,
                            mining_state_file: None,
                            beacon_api_url: None,
                            private_key: None,
                        },
                    );

                // Merge with provided values (only overwrite if provided)
                let rpc_url = if let Some(ref url) = self.rpc_url {
                    display.success("Updated RPC URL");

                    // Query chain ID and warn if not Ethereum mainnet
                    display.info("Querying chain ID from RPC...");
                    let chain_id = query_chain_id(url).await?;
                    display.success(&format!("Chain ID: {}", chain_id.to_string().bright_white()));

                    if chain_id != 1 {
                        display.warning(
                            "The RPC URL you provided is NOT for Ethereum mainnet (chain ID 1)",
                        );
                        display.note("The ZKC token contract is deployed on Ethereum mainnet");
                        display.note("Using a different chain may result in errors when interacting with rewards");

                        let proceed =
                            Confirm::new("Are you sure you want to continue with this RPC URL?")
                                .with_default(false)
                                .prompt()?;

                        if !proceed {
                            bail!("Setup cancelled. Please provide an Ethereum mainnet RPC URL");
                        }
                    }

                    Some(url.clone())
                } else {
                    existing_secrets.rpc_url
                };

                let (staking_private_key, staking_address) =
                    if let Some(ref pk) = self.staking_private_key {
                        let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                        let addr = address_from_private_key(&pk);
                        display.success("Updated staking private key");
                        (Some(pk), addr.map(|a| format!("{:#x}", a)))
                    } else if let Some(ref addr) = self.staking_address {
                        display.success("Updated staking address");
                        (existing_secrets.staking_private_key, Some(addr.clone()))
                    } else {
                        (existing_secrets.staking_private_key, existing_secrets.staking_address)
                    };

                let (reward_private_key, reward_address) =
                    if let Some(ref pk) = self.reward_private_key {
                        let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                        let addr = address_from_private_key(&pk);
                        display.success("Updated reward private key");
                        (Some(pk), addr.map(|a| format!("{:#x}", a)))
                    } else if let Some(ref addr) = self.reward_address {
                        display.success("Updated reward address");
                        (existing_secrets.reward_private_key, Some(addr.clone()))
                    } else {
                        (existing_secrets.reward_private_key, existing_secrets.reward_address)
                    };

                let beacon_api_url = if let Some(ref url) = self.beacon_api_url {
                    display.success("Updated Beacon API URL");
                    Some(url.clone())
                } else {
                    existing_secrets.beacon_api_url
                };

                let mining_state_file = if let Some(ref path) = self.state_file {
                    display.success("Updated PoVW state file path");
                    Some(path.clone())
                } else {
                    existing_secrets.mining_state_file
                };

                secrets.rewards_networks.insert(
                    network_name.clone(),
                    RewardsSecrets {
                        rpc_url,
                        staking_private_key,
                        staking_address,
                        reward_private_key,
                        reward_address,
                        mining_state_file,
                        beacon_api_url,
                        private_key: None,
                    },
                );

                // Update contract addresses if this is a custom network and flags are provided
                if network_name.starts_with("custom-") {
                    custom_networks::update_custom_rewards_addresses(config, &network_name, self)?;
                }

                display.success(&format!(
                    "Updated rewards configuration for network: {}",
                    network_name.cyan().bold()
                ));
                return Ok(true);
            } else {
                // No network selected - need to create custom network
                if let Some(ref rpc_url) = self.rpc_url {
                    // Query chain ID from RPC
                    display.info("Querying chain ID from RPC...");
                    let chain_id = query_chain_id(rpc_url).await?;
                    display.success(&format!("Chain ID: {}", chain_id.to_string().bright_white()));

                    // Warn if not Ethereum mainnet
                    if chain_id != 1 {
                        display.warning(
                            "The RPC URL you provided is NOT for Ethereum mainnet (chain ID 1)",
                        );
                        display.note("The ZKC token contract is deployed on Ethereum mainnet");
                        display.note("Using a different chain may result in errors when interacting with rewards");

                        let proceed =
                            Confirm::new("Are you sure you want to continue with this RPC URL?")
                                .with_default(false)
                                .prompt()?;

                        if !proceed {
                            bail!("Setup cancelled. Please provide an Ethereum mainnet RPC URL");
                        }
                    }

                    let network_name = format!("custom-{}", chain_id);

                    // Check if this custom network already exists
                    if !config.custom_rewards.iter().any(|r| r.name == network_name) {
                        // Create minimal custom rewards deployment
                        let custom_rewards =
                            custom_networks::create_minimal_custom_rewards(chain_id);
                        config.custom_rewards.push(custom_rewards);
                    }

                    // Set as selected network
                    config.rewards = Some(RewardsConfig { network: network_name.clone() });

                    // Extract addresses from private keys if provided, or use explicit addresses
                    let (staking_private_key, staking_address) =
                        if let Some(ref pk) = self.staking_private_key {
                            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                            let addr = address_from_private_key(&pk);
                            (Some(pk), addr.map(|a| format!("{:#x}", a)))
                        } else if let Some(ref addr) = self.staking_address {
                            (None, Some(addr.clone()))
                        } else {
                            (None, None)
                        };

                    let (reward_private_key, reward_address) =
                        if let Some(ref pk) = self.reward_private_key {
                            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
                            let addr = address_from_private_key(&pk);
                            (Some(pk), addr.map(|a| format!("{:#x}", a)))
                        } else if let Some(ref addr) = self.reward_address {
                            (None, Some(addr.clone()))
                        } else {
                            (None, None)
                        };

                    // Save secrets
                    secrets.rewards_networks.insert(
                        network_name.clone(),
                        RewardsSecrets {
                            rpc_url: Some(rpc_url.clone()),
                            staking_private_key,
                            staking_address,
                            reward_private_key,
                            reward_address,
                            mining_state_file: self.state_file.clone(),
                            beacon_api_url: self.beacon_api_url.clone(),
                            private_key: None,
                        },
                    );

                    // Update contract addresses if flags are provided
                    let has_contract_addresses = custom_networks::update_custom_rewards_addresses(
                        config,
                        &network_name,
                        self,
                    )?;

                    display.success(&format!(
                        "Created custom rewards network '{}' (chain ID: {})",
                        network_name.cyan().bold(),
                        chain_id.to_string().bright_white()
                    ));

                    if !has_contract_addresses {
                        display.warning("Contract addresses not configured. Configure them by:");
                        display.note("• Running 'boundless rewards setup' interactively");
                        display.note("• Using command-line flags (--set-zkc-address, etc.)");
                        display.note("• Editing ~/.boundless/config.toml directly");
                    }

                    return Ok(true);
                } else {
                    bail!("No network configured. Please provide --set-rpc-url to create a custom network, or run 'boundless rewards setup' interactively to select a network");
                }
            }
        }

        // Step 4: If --change-network was provided but no other flags, check if we should load existing config
        if let Some(network_name) = &target_network {
            // Check if this network has existing secrets
            if let Some(existing) = secrets.rewards_networks.get(network_name) {
                // Network already configured - load it and display config
                display.success(&format!(
                    "Loaded existing configuration for network: {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing.staking_address {
                    display.note(&format!("Staking Address: {}", addr.bright_yellow()));
                }
                if let Some(ref addr) = existing.reward_address {
                    display.note(&format!("Reward Address: {}", addr.bright_yellow()));
                }
                if let Some(ref path) = existing.mining_state_file {
                    display.note(&format!("PoVW State File: {}", path.cyan()));
                }
                return Ok(true);
            } else {
                // Network not configured - continue to interactive setup
                display.info(&format!(
                    "Setting up new configuration for network: {}",
                    network_name.cyan().bold()
                ));
                // Fall through to interactive setup below
            }
        }

        // Step 5: Fully interactive mode - prompt for network selection
        let network_name = if let Some(network) = target_network {
            // Network was specified via --change-network but doesn't exist yet
            // We already set it in Step 2, just use it
            network
        } else {
            let mut network_options = vec!["Eth Mainnet", "Eth Testnet (Sepolia)"];

            for custom_rewards in &config.custom_rewards {
                network_options.push(&custom_rewards.name);
            }
            network_options.push("Custom (add new)");

            // No --change-network provided - prompt user to select network
            let network = Select::new("Select rewards network:", network_options).prompt()?;

            let selected_network = match network {
                "Eth Mainnet" => "eth-mainnet".to_string(),
                "Eth Testnet (Sepolia)" => "eth-sepolia".to_string(),
                "Custom (add new)" => {
                    let custom = custom_networks::setup_custom_rewards()?;
                    let name = custom.name.clone();
                    config.custom_rewards.push(custom);
                    name
                }
                custom => custom.to_string(),
            };

            config.rewards = Some(RewardsConfig { network: selected_network.clone() });
            selected_network
        };

        // If --state-file is provided, do a quick update and return
        if let Some(ref new_state_file) = self.state_file {
            return self.update_state_file_only(secrets, &network_name, new_state_file).await;
        }

        let existing = secrets.rewards_networks.get(&network_name).cloned();

        if let Some(ref existing_config) = existing {
            if self.rpc_url.is_none() && self.private_key.is_none() {
                display.info(&format!(
                    "Previous configuration found for {}",
                    network_name.cyan().bold()
                ));
                if let Some(ref rpc) = existing_config.rpc_url {
                    display.note(&format!("RPC URL: {}", obscure_url(rpc).dimmed()));
                }
                if let Some(ref addr) = existing_config.staking_address {
                    display.note(&format!("Staking Address: {}", addr.bright_yellow()));
                }
                if let Some(ref addr) = existing_config.reward_address {
                    display.note(&format!("Reward Address: {}", addr.bright_yellow()));
                }
                if let Some(ref path) = existing_config.mining_state_file {
                    display.note(&format!("PoVW State File: {}", path.cyan()));
                }
                if let Some(ref beacon) = existing_config.beacon_api_url {
                    display.note(&format!("Beacon API URL: {}", obscure_url(beacon).dimmed()));
                }
            }
        }

        let rpc_url = if let Some(ref url) = self.rpc_url {
            display.success(&format!("Using RPC URL: {}", url.dimmed()));
            url.clone()
        } else {
            let mut prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(prev) = existing.as_ref().and_then(|e| e.rpc_url.as_ref()) {
                prompt = prompt.with_default(prev);
            }

            prompt.prompt()?
        };

        // Query chain ID and warn if not Ethereum mainnet
        display.info("Querying chain ID from RPC...");
        let chain_id = query_chain_id(&rpc_url).await?;
        display.success(&format!("Chain ID: {}", chain_id.to_string().bright_white()));

        if chain_id != 1 {
            display.warning("The RPC URL you provided is NOT for Ethereum mainnet (chain ID 1)");
            display.note("The ZKC token contract is deployed on Ethereum mainnet");
            display
                .note("Using a different chain may result in errors when interacting with rewards");

            let proceed = Confirm::new("Are you sure you want to continue with this RPC URL?")
                .with_default(false)
                .prompt()?;

            if !proceed {
                bail!("Setup cancelled. Please provide an Ethereum mainnet RPC URL");
            }
        }

        Self::print_section_header(&display, "Staking Address Configuration");
        display.note("The staking address is the wallet used to stake ZKC tokens");

        let (staking_private_key, staking_address) = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            display.success("Using provided private key for staking address");
            let addr = address_from_private_key(&pk);
            (Some(pk), addr.map(|a| format!("{:#x}", a)))
        } else {
            let has_previous_staking_key =
                existing.as_ref().and_then(|e| e.staking_private_key.as_ref()).is_some();
            let has_previous_staking_addr =
                existing.as_ref().and_then(|e| e.staking_address.as_ref()).is_some();

            let default_wants_key = has_previous_staking_key || !has_previous_staking_addr;
            let staking_key_prompt = Confirm::new("Do you want to store a private key for the staking address?")
                .with_default(default_wants_key)
                .with_help_message("Skip if using a hardware wallet, smart contract wallet, or don't need to stake ZKC or delegate rewards")
                .prompt()?;

            if staking_key_prompt {
                let help_msg = if has_previous_staking_key {
                    "Leave empty to keep existing key or enter new key"
                } else {
                    "Will be stored in plaintext in ~/.boundless/secrets.toml"
                };

                let pk =
                    Text::new("Enter staking private key:").with_help_message(help_msg).prompt()?;

                if pk.is_empty() && has_previous_staking_key {
                    let prev_key =
                        existing.as_ref().and_then(|e| e.staking_private_key.clone()).unwrap();
                    let addr = address_from_private_key(&prev_key);
                    (Some(prev_key), addr.map(|a| format!("{:#x}", a)))
                } else {
                    let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                    let addr = address_from_private_key(&pk);
                    (Some(pk), addr.map(|a| format!("{:#x}", a)))
                }
            } else {
                let mut addr_prompt = Text::new("Enter staking address:")
                    .with_help_message("Public Ethereum address that holds your staked ZKC");

                if let Some(prev_addr) = existing.as_ref().and_then(|e| e.staking_address.as_ref())
                {
                    addr_prompt = addr_prompt.with_default(prev_addr);
                }

                let addr = addr_prompt.prompt()?;
                (None, Some(addr))
            }
        };

        // Query on-chain for reward delegation if we have a staking address
        let (reward_private_key, reward_address) = if let Some(ref staking_addr) = staking_address {
            Self::print_section_header(&display, "Reward Address Configuration");
            display.note("The reward address is the address with reward power (can receive delegated rewards)");

            // Try to query delegation on-chain
            print!("{} ", "Checking if you've delegated rewards...".cyan());
            std::io::stdout().flush()?;
            let delegated_address =
                Self::query_reward_delegation(&rpc_url, &network_name, staking_addr)
                    .await
                    .ok()
                    .flatten();

            let prev_reward_key = existing.as_ref().and_then(|e| e.reward_private_key.as_ref());
            let prev_reward_addr = existing.as_ref().and_then(|e| e.reward_address.as_ref());

            if let Some(ref delegated) = delegated_address {
                if delegated.to_lowercase() != staking_addr.to_lowercase() {
                    println!("{}", delegated.bright_yellow());
                    let use_delegated = Confirm::new(
                        "Would you like to use this delegated address as your reward address?",
                    )
                    .with_default(true)
                    .prompt()?;

                    if use_delegated {
                        display.success("Using delegated address as reward address");

                        let has_previous_reward_key = prev_reward_key.is_some();
                        let has_reward_key = Confirm::new("Do you want to store a private key for the reward address?")
                            .with_default(has_previous_reward_key)
                            .with_help_message("Skip if using a hardware wallet, smart contract wallet, or don't need to submit PoVW")
                            .prompt()?;

                        if has_reward_key {
                            let help_msg = if has_previous_reward_key {
                                "Leave empty to keep existing key or enter new key"
                            } else {
                                "Will be stored in plaintext in ~/.boundless/secrets.toml"
                            };

                            let pk = Text::new("Enter reward private key:")
                                .with_help_message(help_msg)
                                .prompt()?;

                            if pk.is_empty() && has_previous_reward_key {
                                let prev_key = prev_reward_key.cloned().unwrap();
                                (Some(prev_key), Some(delegated.clone()))
                            } else {
                                let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                                (Some(pk), Some(delegated.clone()))
                            }
                        } else {
                            (None, Some(delegated.clone()))
                        }
                    } else {
                        Self::ask_for_reward_address(
                            staking_addr,
                            &staking_private_key,
                            &prev_reward_key.cloned(),
                            &prev_reward_addr.cloned(),
                        )
                        .await?
                    }
                } else {
                    println!("{}", "No delegatee".dimmed());
                    Self::ask_for_reward_address(
                        staking_addr,
                        &staking_private_key,
                        &prev_reward_key.cloned(),
                        &prev_reward_addr.cloned(),
                    )
                    .await?
                }
            } else {
                println!("{}", "No delegatee".dimmed());
                Self::ask_for_reward_address(
                    staking_addr,
                    &staking_private_key,
                    &prev_reward_key.cloned(),
                    &prev_reward_addr.cloned(),
                )
                .await?
            }
        } else {
            display.warning("No staking address provided - reward address configuration skipped");
            (None, None)
        };

        // If reward address is the same as staking address, copy the staking private key
        let (final_reward_pk, final_reward_addr) =
            if let (Some(ref reward_addr), Some(ref staking_pk)) =
                (&reward_address, &staking_private_key)
            {
                let staking_addr_derived = address_from_private_key(staking_pk)
                    .map(|a| format!("{:#x}", a).to_lowercase());

                if staking_addr_derived
                    .as_ref()
                    .map(|s| s == &reward_addr.to_lowercase())
                    .unwrap_or(false)
                {
                    // Same address - copy staking PK to reward PK
                    (staking_private_key.clone(), reward_address)
                } else {
                    (reward_private_key, reward_address)
                }
            } else {
                (reward_private_key, reward_address)
            };

        // PoVW State File Configuration
        let mining_state_file = if let Some(ref reward_addr) = final_reward_addr {
            Self::print_section_header(&display, "PoVW (Proof of Verifiable Work) Configuration");

            let has_previous_povw_state =
                existing.as_ref().and_then(|e| e.mining_state_file.as_ref()).is_some();
            let generate_povw = Confirm::new("Do you plan to generate PoVW?")
                .with_default(has_previous_povw_state)
                .with_help_message("PoVW allows you to earn ZKC rewards for proving work")
                .prompt()?;

            if generate_povw {
                use colored::Colorize;

                // Show critical warnings
                println!("\n{}", "⚠️  CRITICAL: PoVW State File Information".yellow().bold());
                println!("  • Loss of this state file WILL result in loss of all unsubmitted work");
                println!("  • Each reward address should have ONLY ONE state file");
                println!("  • Keep this file backed up in a durable location");
                println!("  • See: https://docs.boundless.network/zkc/mining/walkthrough\n");

                let has_existing = Confirm::new(&format!(
                    "Do you have an existing PoVW state file for {}?",
                    reward_addr
                ))
                .with_default(has_previous_povw_state)
                .prompt()?;

                if has_existing {
                    let mut path_prompt = Text::new("Enter path to existing state file:")
                        .with_help_message("Enter full path (supports ~)");

                    if let Some(prev_path) =
                        existing.as_ref().and_then(|e| e.mining_state_file.as_ref())
                    {
                        path_prompt = path_prompt.with_default(prev_path);
                    }

                    let path = path_prompt.prompt()?;

                    // Expand ~ to home directory
                    let expanded_path = if path.starts_with("~/") {
                        let home = dirs::home_dir().context("Failed to get home directory")?;
                        home.join(path.strip_prefix("~/").unwrap()).display().to_string()
                    } else {
                        path
                    };

                    // Validate that the state file's log_id matches the reward address
                    print!("{} ", "Validating state file...".cyan());
                    std::io::stdout().flush()?;

                    match State::load(&expanded_path).await {
                        Ok(state) => {
                            // Parse reward address as PovwLogId for comparison
                            let reward_log_id = reward_addr
                                .parse::<PovwLogId>()
                                .context("Failed to parse reward address as PoVW log ID")?;

                            if state.log_id != reward_log_id {
                                println!("{}", "✗".red());
                                bail!(
                                    "PoVW state file log ID mismatch!\n\n\
                                    State file log ID: {:x}\n\
                                    Reward address:    {}\n\n\
                                    Each reward address must have its own unique state file.\n\
                                    Either use the correct state file for this reward address,\n\
                                    or create a new state file.",
                                    state.log_id,
                                    reward_addr
                                );
                            }
                            println!("{}", "✓".green());
                            display.note(&format!(
                                "Log ID matches reward address: {:x}",
                                state.log_id
                            ));
                        }
                        Err(e) => {
                            println!("{}", "✗".red());
                            bail!("Failed to load state file: {}\n\nPlease check the path and try again.", e);
                        }
                    }

                    Some(expanded_path)
                } else {
                    let default_filename = format!(
                        "povw_state_{}.bin",
                        reward_addr.strip_prefix("0x").unwrap_or(reward_addr)
                    );

                    let path = Text::new("Enter path for new state file:")
                        .with_default(&default_filename)
                        .with_help_message("Enter filename (current dir) or full path (supports ~)")
                        .prompt()?;

                    // Expand ~ to home directory
                    let expanded_path = if path.starts_with("~/") {
                        let home = dirs::home_dir().context("Failed to get home directory")?;
                        home.join(path.strip_prefix("~/").unwrap()).display().to_string()
                    } else {
                        path
                    };

                    // Get absolute path for display
                    let abs_path = std::fs::canonicalize(&expanded_path).unwrap_or_else(|_| {
                        // If file doesn't exist yet, resolve relative to current dir
                        std::env::current_dir()
                            .map(|cwd| cwd.join(&expanded_path))
                            .unwrap_or_else(|_| std::path::PathBuf::from(&expanded_path))
                    });

                    // Safety check: NEVER overwrite an existing file
                    if std::path::Path::new(&expanded_path).exists() {
                        bail!(
                            "File already exists at: {}\n\n\
                            If this is an existing PoVW state file, please choose the 'existing file' option instead.\n\
                            If you want to create a new state file, please specify a different path.",
                            abs_path.display()
                        );
                    }

                    // Create empty state file
                    use risc0_povw::PovwLogId;
                    let log_id = reward_addr
                        .parse::<PovwLogId>()
                        .context("Failed to parse reward address as PoVW log ID")?;

                    let empty_state = State::new(log_id);

                    // Create parent directory if it doesn't exist
                    if let Some(parent) = std::path::Path::new(&expanded_path).parent() {
                        std::fs::create_dir_all(parent).with_context(|| {
                            format!("Failed to create directory: {}", parent.display())
                        })?;
                    }

                    // Save the empty state file
                    empty_state.save(&expanded_path).with_context(|| {
                        format!("Failed to create state file: {}", abs_path.display())
                    })?;

                    display.success(&format!(
                        "Created empty state file at: {}",
                        abs_path.display().to_string().cyan()
                    ));
                    display.note("Remember to back this up regularly!");
                    Some(expanded_path)
                }
            } else {
                None
            }
        } else {
            None
        };

        // Beacon API URL Configuration (needed for claiming PoVW rewards)
        let beacon_api_url = if mining_state_file.is_some() {
            Self::print_section_header(&display, "Beacon API Configuration");
            display.note("A Beacon API URL is required to claim PoVW rewards on Ethereum");
            display.note("Providers like Quicknode offer Beacon API access");

            let has_previous_beacon =
                existing.as_ref().and_then(|e| e.beacon_api_url.as_ref()).is_some();
            let configure_beacon = Confirm::new("Configure Beacon API URL now?")
                .with_default(has_previous_beacon)
                .with_help_message("You can set this later via BEACON_API_URL env var")
                .prompt()?;

            if configure_beacon {
                let mut beacon_prompt = Text::new("Enter Beacon API URL:")
                    .with_help_message("e.g., https://YOUR_PROVIDER/beacon");

                if let Some(prev_beacon) = existing.as_ref().and_then(|e| e.beacon_api_url.as_ref())
                {
                    beacon_prompt = beacon_prompt.with_default(prev_beacon);
                }

                Some(beacon_prompt.prompt()?)
            } else {
                None
            }
        } else {
            None
        };

        secrets.rewards_networks.insert(
            network_name,
            RewardsSecrets {
                rpc_url: Some(rpc_url),
                staking_private_key,
                staking_address,
                reward_private_key: final_reward_pk,
                reward_address: final_reward_addr,
                mining_state_file,
                beacon_api_url,
                private_key: None,
            },
        );

        display.success("Rewards module configured successfully");

        Ok(true)
    }

    async fn ask_for_reward_address(
        staking_addr: &str,
        staking_private_key: &Option<String>,
        previous_reward_key: &Option<String>,
        previous_reward_addr: &Option<String>,
    ) -> Result<(Option<String>, Option<String>)> {
        let display = DisplayManager::new();

        let default_same_as_staking = if let Some(prev_addr) = previous_reward_addr {
            prev_addr.to_lowercase() == staking_addr.to_lowercase()
        } else {
            true
        };

        let same_as_staking = Confirm::new("Is the reward address the same as the staking address?")
            .with_default(default_same_as_staking)
            .with_help_message("The reward address is the address with reward power (e.g., if staking address has delegated to another address)")
            .prompt()?;

        if same_as_staking {
            display.warning(
                "Using the same address for reward and staking address is not recommended",
            );
            display.note("See: https://docs.boundless.network/zkc/mining/wallet-setup");
            Ok((staking_private_key.clone(), Some(staking_addr.to_string())))
        } else {
            let has_previous_reward_key = previous_reward_key.is_some();
            let has_reward_key = Confirm::new("Do you want to store a private key for the reward address?")
                .with_default(has_previous_reward_key)
                .with_help_message("Skip if using a hardware wallet, smart contract wallet, or don't need to stake ZKC or delegate rewards")
                .prompt()?;

            if has_reward_key {
                let help_msg = if has_previous_reward_key {
                    "Leave empty to keep existing key or enter new key"
                } else {
                    "Will be stored in plaintext in ~/.boundless/secrets.toml"
                };

                let pk =
                    Text::new("Enter reward private key:").with_help_message(help_msg).prompt()?;

                if pk.is_empty() && has_previous_reward_key {
                    let prev_key = previous_reward_key.clone().unwrap();
                    let addr = address_from_private_key(&prev_key);
                    Ok((Some(prev_key), addr.map(|a| format!("{:#x}", a))))
                } else {
                    let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                    let addr = address_from_private_key(&pk);
                    Ok((Some(pk), addr.map(|a| format!("{:#x}", a))))
                }
            } else {
                let mut addr_prompt = Text::new("Enter reward address:")
                    .with_help_message("Public Ethereum address with reward power");

                if let Some(prev_addr) = previous_reward_addr {
                    addr_prompt = addr_prompt.with_default(prev_addr);
                }

                let addr = addr_prompt.prompt()?;
                Ok((None, Some(addr)))
            }
        }
    }

    async fn query_reward_delegation(
        rpc_url: &str,
        network: &str,
        address: &str,
    ) -> Result<Option<String>> {
        use alloy::primitives::Address;
        use alloy::providers::ProviderBuilder;

        // Determine veZKC address based on network
        let vezkc_address = match network {
            "eth-mainnet" => "0xe8ae8ee8ffa57f6a79b6cbe06bafc0b05f3ffbf4",
            "eth-sepolia" => "0xc23340732038ca6C5765763180E81B395d2e9cCA",
            _ => return Ok(None), // Can't query custom networks without deployment info
        };

        let addr: Address = address.parse().context("Invalid staking address")?;
        let vezkc_addr: Address = vezkc_address.parse()?;

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url)
            .await
            .context("Failed to connect to provider")?;

        // Define IRewards interface
        alloy::sol! {
            #[sol(rpc)]
            interface IRewards {
                function rewardDelegates(address account) external view returns (address);
            }
        }

        let rewards_contract = IRewards::new(vezkc_addr, &provider);
        let delegate = rewards_contract
            .rewardDelegates(addr)
            .call()
            .await
            .context("Failed to query reward delegation")?;

        Ok(Some(format!("{:#x}", delegate)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config_file::{Config, RequestorConfig, RequestorSecrets, Secrets};
    use alloy::primitives::Address;

    #[test]
    fn test_non_interactive_custom_network_creation() {
        let boundless_market_addr = "0x5FbDB2315678afecb367f032d93F642f64180aa3";
        let set_verifier_addr = "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512";
        let collateral_token_addr = "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0";
        let order_stream_url = "wss://localhost:3030";

        let setup = SetupInteractive {
            network: None,
            rpc_url: Some("http://localhost:8545".to_string()),
            private_key: Some(
                "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80".to_string(),
            ),
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: Some(boundless_market_addr.to_string()),
            verifier_router_address: None,
            set_verifier_address: Some(set_verifier_addr.to_string()),
            collateral_token_address: Some(collateral_token_addr.to_string()),
            order_stream_url: Some(order_stream_url.to_string()),
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let chain_id = 31337u64;

        let mut config = Config::default();
        let network_name = format!("custom-{}", chain_id);

        let custom_network = custom_networks::create_minimal_custom_network(chain_id);
        config.custom_markets.push(custom_network);

        assert_eq!(config.custom_markets.len(), 1);
        assert_eq!(config.custom_markets[0].name, "custom-31337");
        assert_eq!(config.custom_markets[0].chain_id, 31337);
        assert_eq!(config.custom_markets[0].boundless_market_address, Address::ZERO);

        let updated =
            custom_networks::update_custom_market_addresses(&mut config, &network_name, &setup);
        assert!(updated.is_ok());
        assert!(updated.unwrap());

        let network = &config.custom_markets[0];
        assert_eq!(
            format!("{:#x}", network.boundless_market_address),
            boundless_market_addr.to_lowercase()
        );
        assert_eq!(
            format!("{:#x}", network.set_verifier_address),
            set_verifier_addr.to_lowercase()
        );
        assert_eq!(
            format!("{:#x}", network.collateral_token_address.unwrap()),
            collateral_token_addr.to_lowercase()
        );
        assert_eq!(network.order_stream_url.as_ref().unwrap(), order_stream_url);

        let mut secrets = Secrets::default();

        let pk_clean = setup
            .private_key
            .as_ref()
            .unwrap()
            .strip_prefix("0x")
            .unwrap_or(setup.private_key.as_ref().unwrap());

        secrets.requestor_networks.insert(
            network_name.clone(),
            RequestorSecrets {
                rpc_url: setup.rpc_url.clone(),
                private_key: Some(pk_clean.to_string()),
                address: Some("0xf39fd6e51aad88f6f4ce6ab8827279cfffb92266".to_string()),
            },
        );

        assert_eq!(secrets.requestor_networks.len(), 1);
        assert!(secrets.requestor_networks.contains_key("custom-31337"));

        let network_secrets = secrets.requestor_networks.get("custom-31337").unwrap();
        assert_eq!(network_secrets.rpc_url.as_ref().unwrap(), "http://localhost:8545");
        assert!(network_secrets.private_key.is_some());
        assert!(network_secrets.address.is_some());

        config.requestor = Some(RequestorConfig { network: network_name.clone() });

        assert!(config.requestor.is_some());
        assert_eq!(config.requestor.unwrap().network, "custom-31337");
    }

    #[test]
    fn test_clone_prebuilt_when_modifying() {
        let config = Config::default();

        let setup = SetupInteractive {
            network: Some("base-mainnet".to_string()),
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: Some(
                "0x1234567890123456789012345678901234567890".to_string(),
            ),
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let custom_network =
            custom_networks::clone_prebuilt_market_as_custom(&config, "base-mainnet", &setup);
        assert!(custom_network.is_ok());

        let network = custom_network.unwrap();

        assert!(network.name.starts_with("custom-base-mainnet"));
        assert_eq!(network.chain_id, 8453);

        assert_eq!(
            format!("{:#x}", network.boundless_market_address),
            "0x1234567890123456789012345678901234567890"
        );

        assert_ne!(network.set_verifier_address, Address::ZERO);
    }

    #[test]
    fn test_update_existing_custom_network() {
        let mut config = Config::default();

        let mut network = custom_networks::create_minimal_custom_network(1234);
        network.boundless_market_address =
            "0x5FbDB2315678afecb367f032d93F642f64180aa3".parse().unwrap();
        network.set_verifier_address =
            "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512".parse().unwrap();
        config.custom_markets.push(network);

        let setup = SetupInteractive {
            network: Some("custom-1234".to_string()),
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: None,
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: Some("wss://new-url.com".to_string()),
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let result =
            custom_networks::update_custom_market_addresses(&mut config, "custom-1234", &setup);
        assert!(result.is_ok());
        assert!(result.unwrap());

        let network = &config.custom_markets[0];
        assert_eq!(network.order_stream_url.as_ref().unwrap(), "wss://new-url.com");

        assert_eq!(
            format!("{:#x}", network.boundless_market_address),
            "0x5fbdb2315678afecb367f032d93f642f64180aa3"
        );
        assert_eq!(
            format!("{:#x}", network.set_verifier_address),
            "0xe7f1725e7734ce288f8367e1bb143e90bb3f0512"
        );
    }

    #[test]
    fn test_unique_name_generation_on_clone() {
        let mut config = Config::default();

        let mut existing = custom_networks::create_minimal_custom_network(8453);
        existing.name = "custom-base-mainnet".to_string();
        config.custom_markets.push(existing);

        let setup = SetupInteractive {
            network: None,
            rpc_url: None,
            private_key: None,
            address: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            beacon_api_url: None,
            state_file: None,
            boundless_market_address: Some(
                "0x1234567890123456789012345678901234567890".to_string(),
            ),
            verifier_router_address: None,
            set_verifier_address: None,
            collateral_token_address: None,
            order_stream_url: None,
            zkc_address: None,
            vezkc_address: None,
            staking_rewards_address: None,
            mining_accounting_address: None,
            mining_mint_address: None,
            rename_network: None,
            reset: false,
            reset_all: false,
        };

        let network =
            custom_networks::clone_prebuilt_market_as_custom(&config, "base-mainnet", &setup)
                .unwrap();

        assert_eq!(network.name, "custom-base-mainnet-1");
    }
}
