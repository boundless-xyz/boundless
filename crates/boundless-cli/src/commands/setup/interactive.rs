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

//! Interactive setup command for the Boundless CLI.

use anyhow::{Context, Result};
use clap::Args;
use inquire::{Confirm, Select, Text};

use crate::config::GlobalConfig;
use crate::config_file::{
    Config, CustomMarketDeployment, CustomRewardsDeployment, MarketConfig, MarketSecrets,
    RewardsConfig, RewardsSecrets, Secrets,
};

/// Interactive setup command
#[derive(Args, Clone, Debug)]
pub struct SetupInteractive;

impl SetupInteractive {
    /// Run the interactive setup
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        println!("\n🔧 Boundless CLI Interactive Setup\n");

        let module = Select::new(
            "Which module would you like to configure?",
            vec!["Requestor/Prover", "Rewards", "Both"],
        )
        .prompt()?;

        let mut config = Config::load().unwrap_or_default();
        let mut secrets = Secrets::load().unwrap_or_default();

        match module {
            "Requestor/Prover" => {
                Self::setup_market(&mut config, &mut secrets).await?;
            }
            "Rewards" => {
                Self::setup_rewards(&mut config, &mut secrets).await?;
            }
            "Both" => {
                Self::setup_market(&mut config, &mut secrets).await?;
                Self::setup_rewards(&mut config, &mut secrets).await?;
            }
            _ => unreachable!(),
        }

        config.save()?;
        println!("\n✓ Configuration saved to {}", Config::path()?.display());

        if secrets.market.is_some() || secrets.rewards.is_some() {
            secrets.save()?;
            println!("✓ Secrets saved to {} (permissions: 600)", Secrets::path()?.display());
            println!("\n⚠️  Warning: Secrets are stored in plaintext. Consider using environment variables instead.");
        }

        println!("\n✨ Setup complete! Run `boundless` to see your configuration.");

        Ok(())
    }

    /// Run setup for a specific module
    pub async fn run_module(&self, _global_config: &GlobalConfig, module: &str) -> Result<()> {
        println!("\n🔧 Boundless CLI Setup\n");

        let mut config = Config::load().unwrap_or_default();
        let mut secrets = Secrets::load().unwrap_or_default();

        match module {
            "requestor" => {
                Self::setup_market(&mut config, &mut secrets).await?;
            }
            "rewards" => {
                Self::setup_rewards(&mut config, &mut secrets).await?;
            }
            _ => unreachable!(),
        }

        config.save()?;
        println!("\n✓ Configuration saved to {}", Config::path()?.display());

        if secrets.market.is_some() || secrets.rewards.is_some() {
            secrets.save()?;
            println!("✓ Secrets saved to {} (permissions: 600)", Secrets::path()?.display());
            println!("\n⚠️  Warning: Secrets are stored in plaintext. Consider using environment variables instead.");
        }

        println!("\n✨ Setup complete! Run `boundless {}` to see your configuration.", module);

        Ok(())
    }

    async fn setup_market(config: &mut Config, secrets: &mut Secrets) -> Result<()> {
        println!("\n--- Requestor/Prover Module Setup ---\n");

        let existing_market = secrets.market.as_ref();
        if existing_market.is_some() {
            println!("📝 Existing configuration detected - current values will be shown\n");
        }

        let mut network_options = vec!["Base Mainnet", "Base Sepolia", "Ethereum Sepolia"];

        for custom_market in &config.custom_markets {
            network_options.push(&custom_market.name);
        }
        network_options.push("Custom (add new)");

        let network = Select::new("Select Boundless Market network:", network_options).prompt()?;

        let network_name = match network {
            "Base Mainnet" => "base-mainnet".to_string(),
            "Base Sepolia" => "base-sepolia".to_string(),
            "Ethereum Sepolia" => "eth-sepolia".to_string(),
            "Custom (add new)" => {
                let custom = Self::setup_custom_market()?;
                let name = custom.name.clone();
                config.custom_markets.push(custom);
                name
            }
            custom => custom.to_string(),
        };

        config.market = Some(MarketConfig { network: network_name });

        let mut rpc_url_prompt = Text::new("Enter RPC URL for this network:")
            .with_help_message("e.g., https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

        if let Some(existing_rpc) = existing_market.and_then(|m| m.rpc_url.as_ref()) {
            rpc_url_prompt = rpc_url_prompt.with_default(existing_rpc);
        }

        let rpc_url = rpc_url_prompt.prompt()?;

        let has_existing_key = existing_market.and_then(|m| m.private_key.as_ref()).is_some();
        let private_key_prompt = if has_existing_key {
            let help_msg = format!("Current: {}", Self::obscure_secret("existing_key"));
            Confirm::new("Do you want to update the stored private key?")
                .with_default(false)
                .with_help_message(&help_msg)
                .prompt()?
        } else {
            Confirm::new("Do you want to store a private key?")
                .with_default(false)
                .with_help_message("Required for write operations (submitting requests, depositing funds, etc.)")
                .prompt()?
        };

        let private_key = if private_key_prompt {
            let storage_method = Select::new(
                "How would you like to store the private key?",
                vec!["Config file (less secure)", "Environment variable (recommended)"],
            )
            .prompt()?;

            match storage_method {
                "Config file (less secure)" => {
                    let pk = Text::new("Enter private key (without 0x prefix):")
                        .with_help_message("This will be stored in ~/.boundless/secrets.toml")
                        .prompt()?;
                    Some(pk)
                }
                "Environment variable (recommended)" => {
                    println!("\n✓ Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):");
                    println!("  export PRIVATE_KEY=your_private_key_here\n");
                    None
                }
                _ => unreachable!(),
            }
        } else if has_existing_key {
            existing_market.and_then(|m| m.private_key.clone())
        } else {
            println!("\n✓ You can set PRIVATE_KEY environment variable later for write operations");
            None
        };

        secrets.market = Some(MarketSecrets {
            rpc_url: Some(rpc_url),
            private_key,
        });

        Ok(())
    }

    fn obscure_secret(secret: &str) -> String {
        if secret.len() <= 8 {
            "****".to_string()
        } else {
            format!("{}...{}", &secret[..4], &secret[secret.len()-4..])
        }
    }

    fn setup_custom_market() -> Result<CustomMarketDeployment> {
        println!("\n--- Custom Market Deployment ---\n");

        let name = Text::new("Deployment name:")
            .with_help_message("A friendly name for this deployment (e.g., 'my-testnet')")
            .prompt()?;

        let chain_id: u64 = Text::new("Chain ID:")
            .with_help_message("EIP-155 chain ID (e.g., 84532 for Base Sepolia)")
            .prompt()?
            .parse()
            .context("Invalid chain ID")?;

        let boundless_market_address = Text::new("BoundlessMarket contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let verifier_router_address_str = Text::new("RiscZeroVerifierRouter address (optional, press Enter to skip):")
            .with_default("")
            .prompt()?;

        let verifier_router_address = if verifier_router_address_str.is_empty() {
            None
        } else {
            Some(verifier_router_address_str.parse().context("Invalid address")?)
        };

        let set_verifier_address = Text::new("RiscZeroSetVerifier address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let collateral_token_address_str = Text::new("Collateral token address (optional, press Enter to skip):")
            .with_default("")
            .prompt()?;

        let collateral_token_address = if collateral_token_address_str.is_empty() {
            None
        } else {
            Some(collateral_token_address_str.parse().context("Invalid address")?)
        };

        let order_stream_url_str = Text::new("Order stream URL (optional, press Enter to skip):")
            .with_default("")
            .prompt()?;

        let order_stream_url = if order_stream_url_str.is_empty() {
            None
        } else {
            Some(order_stream_url_str)
        };

        Ok(CustomMarketDeployment {
            name,
            chain_id,
            boundless_market_address,
            verifier_router_address,
            set_verifier_address,
            collateral_token_address,
            order_stream_url,
        })
    }

    async fn setup_rewards(config: &mut Config, secrets: &mut Secrets) -> Result<()> {
        println!("\n--- Rewards Module Setup ---\n");

        let existing_rewards = secrets.rewards.as_ref();
        if existing_rewards.is_some() {
            println!("📝 Existing configuration detected - current values will be shown\n");
        }

        let mut network_options = vec!["Mainnet", "Testnet (Sepolia)"];

        for custom_rewards in &config.custom_rewards {
            network_options.push(&custom_rewards.name);
        }
        network_options.push("Custom (add new)");

        let network = Select::new("Select rewards network:", network_options).prompt()?;

        let network_name = match network {
            "Mainnet" => "mainnet".to_string(),
            "Testnet (Sepolia)" => "sepolia".to_string(),
            "Custom (add new)" => {
                let custom = Self::setup_custom_rewards()?;
                let name = custom.name.clone();
                config.custom_rewards.push(custom);
                name
            }
            custom => custom.to_string(),
        };

        config.rewards = Some(RewardsConfig { network: network_name });

        let mut rpc_url_prompt = Text::new("Enter RPC URL for this network:")
            .with_help_message("e.g., https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

        if let Some(existing_rpc) = existing_rewards.and_then(|r| r.rpc_url.as_ref()) {
            rpc_url_prompt = rpc_url_prompt.with_default(existing_rpc);
        }

        let rpc_url = rpc_url_prompt.prompt()?;

        let has_existing_key = existing_rewards.and_then(|r| r.private_key.as_ref()).is_some();
        let private_key_prompt = if has_existing_key {
            let help_msg = format!("Current: {}", Self::obscure_secret("existing_key"));
            Confirm::new("Do you want to update the stored private key?")
                .with_default(false)
                .with_help_message(&help_msg)
                .prompt()?
        } else {
            Confirm::new("Do you want to store a private key?")
                .with_default(false)
                .with_help_message("Required for staking, claiming rewards, etc.")
                .prompt()?
        };

        let private_key = if private_key_prompt {
            let storage_method = Select::new(
                "How would you like to store the private key?",
                vec!["Config file (less secure)", "Environment variable (recommended)"],
            )
            .prompt()?;

            match storage_method {
                "Config file (less secure)" => {
                    let pk = Text::new("Enter private key (without 0x prefix):")
                        .with_help_message("This will be stored in ~/.boundless/secrets.toml")
                        .prompt()?;
                    Some(pk)
                }
                "Environment variable (recommended)" => {
                    println!("\n✓ Add this to your shell profile (~/.bashrc, ~/.zshrc, etc.):");
                    println!("  export PRIVATE_KEY=your_private_key_here\n");
                    None
                }
                _ => unreachable!(),
            }
        } else if has_existing_key {
            existing_rewards.and_then(|r| r.private_key.clone())
        } else {
            println!("\n✓ You can set PRIVATE_KEY environment variable later for write operations");
            None
        };

        secrets.rewards = Some(RewardsSecrets {
            rpc_url: Some(rpc_url),
            private_key,
        });

        Ok(())
    }

    fn setup_custom_rewards() -> Result<CustomRewardsDeployment> {
        println!("\n--- Custom Rewards Deployment ---\n");

        let name = Text::new("Deployment name:")
            .with_help_message("A friendly name for this deployment (e.g., 'my-testnet')")
            .prompt()?;

        let chain_id: u64 = Text::new("Chain ID:")
            .with_help_message("EIP-155 chain ID")
            .prompt()?
            .parse()
            .context("Invalid chain ID")?;

        let zkc_address = Text::new("ZKC token contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let vezkc_address = Text::new("veZKC contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let staking_rewards_address = Text::new("Staking rewards contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let povw_accounting_address = Text::new("PoVW accounting contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        let povw_mint_address = Text::new("PoVW mint contract address:")
            .prompt()?
            .parse()
            .context("Invalid address")?;

        Ok(CustomRewardsDeployment {
            name,
            chain_id,
            zkc_address,
            vezkc_address,
            staking_rewards_address,
            povw_accounting_address,
            povw_mint_address,
        })
    }
}
