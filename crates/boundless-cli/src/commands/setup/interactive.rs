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

use std::io::Write;

use anyhow::{Context, Result};
use clap::Args;
use inquire::{Confirm, Select, Text};

use crate::config::GlobalConfig;
use crate::config_file::{
    Config, CustomMarketDeployment, CustomRewardsDeployment, ProverConfig, ProverSecrets,
    RequestorConfig, RequestorSecrets, RewardsConfig, RewardsSecrets, Secrets,
};

/// Interactive setup command
#[derive(Args, Clone, Debug)]
pub struct SetupInteractive {
    /// Network name (e.g., base-mainnet, base-sepolia, eth-sepolia)
    #[arg(long = "set-network")]
    pub network: Option<String>,

    /// RPC URL for the network
    #[arg(long = "set-rpc-url")]
    pub rpc_url: Option<String>,

    /// Private key for transactions (will be stored in ~/.boundless/secrets.toml)
    #[arg(long = "set-private-key")]
    pub private_key: Option<String>,
}

impl SetupInteractive {
    /// Run the interactive setup
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        println!("\n🔧 Boundless CLI Interactive Setup\n");

        let module = Select::new(
            "Which module would you like to configure?",
            vec!["Requestor", "Prover", "Rewards", "All"],
        )
        .prompt()?;

        let mut config = Config::load().unwrap_or_default();
        let mut secrets = Secrets::load().unwrap_or_default();

        match module {
            "Requestor" => {
                self.setup_requestor(&mut config, &mut secrets).await?;
            }
            "Prover" => {
                self.setup_prover(&mut config, &mut secrets).await?;
            }
            "Rewards" => {
                self.setup_rewards(&mut config, &mut secrets).await?;
            }
            "All" => {
                self.setup_requestor(&mut config, &mut secrets).await?;
                self.setup_prover(&mut config, &mut secrets).await?;
                self.setup_rewards(&mut config, &mut secrets).await?;
            }
            _ => unreachable!(),
        }

        config.save()?;
        println!("\n✓ Configuration saved to {}", Config::path()?.display());

        if secrets.requestor.is_some() || secrets.prover.is_some() || secrets.rewards.is_some() {
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
                self.setup_requestor(&mut config, &mut secrets).await?;
            }
            "prover" => {
                self.setup_prover(&mut config, &mut secrets).await?;
            }
            "rewards" => {
                self.setup_rewards(&mut config, &mut secrets).await?;
            }
            _ => unreachable!(),
        }

        config.save()?;
        println!("\n✓ Configuration saved to {}", Config::path()?.display());

        if secrets.requestor.is_some() || secrets.prover.is_some() || secrets.rewards.is_some() {
            secrets.save()?;
            println!("✓ Secrets saved to {} (permissions: 600)", Secrets::path()?.display());
            println!("\n⚠️  Warning: Secrets are stored in plaintext. Consider using environment variables instead.");
        }

        println!("\n✨ Setup complete! Run `boundless {}` to see your configuration.", module);

        Ok(())
    }

    async fn setup_requestor(&self, config: &mut Config, secrets: &mut Secrets) -> Result<()> {
        println!("\n--- Requestor Module Setup ---\n");

        let existing_requestor = secrets.requestor.as_ref();
        if existing_requestor.is_some() && self.network.is_none() && self.rpc_url.is_none() && self.private_key.is_none() {
            println!("📝 Existing configuration detected - current values will be shown\n");
        }

        let network_name = if let Some(ref network) = self.network {
            println!("✓ Using network: {}", network);
            network.clone()
        } else {
            let mut network_options = vec!["Base Mainnet", "Base Sepolia", "Ethereum Sepolia"];

            for custom_market in &config.custom_markets {
                network_options.push(&custom_market.name);
            }
            network_options.push("Custom (add new)");

            let network = Select::new("Select Boundless Market network:", network_options).prompt()?;

            match network {
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
            }
        };

        config.requestor = Some(RequestorConfig { network: network_name });

        let rpc_url = if let Some(ref url) = self.rpc_url {
            println!("✓ Using RPC URL: {}", url);
            url.clone()
        } else {
            let mut rpc_url_prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(existing_rpc) = existing_requestor.and_then(|m| m.rpc_url.as_ref()) {
                rpc_url_prompt = rpc_url_prompt.with_default(existing_rpc);
            }

            rpc_url_prompt.prompt()?
        };

        let private_key = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            println!("✓ Using provided private key");
            Some(pk)
        } else {
            let has_existing_key = existing_requestor.and_then(|m| m.private_key.as_ref()).is_some();
            let private_key_prompt = if has_existing_key {
                let help_msg = format!("Current: {}", Self::obscure_secret("existing_key"));
                Confirm::new("Do you want to update the stored private key?")
                    .with_default(false)
                    .with_help_message(&help_msg)
                    .prompt()?
            } else {
                Confirm::new("Do you want to store a private key?")
                    .with_default(false)
                    .with_help_message(
                        "Required for write operations. If no, you can set REQUESTOR_PRIVATE_KEY env variable instead.",
                    )
                    .prompt()?
            };

            if private_key_prompt {
                let pk = Text::new("Enter private key:")
                    .with_help_message("Will be stored in plaintext in ~/.boundless/secrets.toml")
                    .prompt()?;
                let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                Some(pk)
            } else if has_existing_key {
                existing_requestor.and_then(|m| m.private_key.clone())
            } else {
                println!("\n✓ You can set REQUESTOR_PRIVATE_KEY environment variable later for write operations");
                None
            }
        };

        secrets.requestor = Some(RequestorSecrets { rpc_url: Some(rpc_url), private_key });

        Ok(())
    }

    async fn setup_prover(&self, config: &mut Config, secrets: &mut Secrets) -> Result<()> {
        println!("\n--- Prover Module Setup ---\n");

        let existing_prover = secrets.prover.as_ref();
        if existing_prover.is_some() && self.network.is_none() && self.rpc_url.is_none() && self.private_key.is_none() {
            println!("📝 Existing configuration detected - current values will be shown\n");
        }

        let network_name = if let Some(ref network) = self.network {
            println!("✓ Using network: {}", network);
            network.clone()
        } else {
            let mut network_options = vec!["Base Mainnet", "Base Sepolia", "Ethereum Sepolia"];

            for custom_market in &config.custom_markets {
                network_options.push(&custom_market.name);
            }
            network_options.push("Custom (add new)");

            let network = Select::new("Select Boundless Market network:", network_options).prompt()?;

            match network {
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
            }
        };

        config.prover = Some(ProverConfig { network: network_name });

        let rpc_url = if let Some(ref url) = self.rpc_url {
            println!("✓ Using RPC URL: {}", url);
            url.clone()
        } else {
            let mut rpc_url_prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://base-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(existing_rpc) = existing_prover.and_then(|m| m.rpc_url.as_ref()) {
                rpc_url_prompt = rpc_url_prompt.with_default(existing_rpc);
            }

            rpc_url_prompt.prompt()?
        };

        let private_key = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            println!("✓ Using provided private key");
            Some(pk)
        } else {
            let has_existing_key = existing_prover.and_then(|m| m.private_key.as_ref()).is_some();
            let private_key_prompt = if has_existing_key {
                let help_msg = format!("Current: {}", Self::obscure_secret("existing_key"));
                Confirm::new("Do you want to update the stored private key?")
                    .with_default(false)
                    .with_help_message(&help_msg)
                    .prompt()?
            } else {
                Confirm::new("Do you want to store a private key?")
                    .with_default(false)
                    .with_help_message(
                        "Required for write operations. If no, you can set PROVER_PRIVATE_KEY env variable instead.",
                    )
                    .prompt()?
            };

            if private_key_prompt {
                let pk = Text::new("Enter private key:")
                    .with_help_message("Will be stored in plaintext in ~/.boundless/secrets.toml")
                    .prompt()?;
                let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                Some(pk)
            } else if has_existing_key {
                existing_prover.and_then(|m| m.private_key.clone())
            } else {
                println!("\n✓ You can set PROVER_PRIVATE_KEY environment variable later for write operations");
                None
            }
        };

        secrets.prover = Some(ProverSecrets { rpc_url: Some(rpc_url), private_key });

        Ok(())
    }

    fn obscure_secret(secret: &str) -> String {
        if secret.len() <= 8 {
            "****".to_string()
        } else {
            format!("{}...{}", &secret[..4], &secret[secret.len() - 4..])
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

        let verifier_router_address_str =
            Text::new("RiscZeroVerifierRouter address (optional, press Enter to skip):")
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

        let collateral_token_address_str =
            Text::new("Collateral token address (optional, press Enter to skip):")
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

        let order_stream_url =
            if order_stream_url_str.is_empty() { None } else { Some(order_stream_url_str) };

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

    async fn setup_rewards(&self, config: &mut Config, secrets: &mut Secrets) -> Result<()> {
        println!("\n--- Rewards Module Setup ---\n");

        let existing_rewards = secrets.rewards.as_ref();
        if existing_rewards.is_some() && self.network.is_none() && self.rpc_url.is_none() && self.private_key.is_none() {
            println!("📝 Existing configuration detected - current values will be shown\n");
        }

        let network_name = if let Some(ref network) = self.network {
            println!("✓ Using network: {}", network);
            network.clone()
        } else {
            let mut network_options = vec!["Eth Mainnet", "Eth Testnet (Sepolia)"];

            for custom_rewards in &config.custom_rewards {
                network_options.push(&custom_rewards.name);
            }
            network_options.push("Custom (add new)");

            let network = Select::new("Select rewards network:", network_options).prompt()?;

            match network {
                "Eth Mainnet" => "mainnet".to_string(),
                "Eth Testnet (Sepolia)" => "sepolia".to_string(),
                "Custom (add new)" => {
                    let custom = Self::setup_custom_rewards()?;
                    let name = custom.name.clone();
                    config.custom_rewards.push(custom);
                    name
                }
                custom => custom.to_string(),
            }
        };

        config.rewards = Some(RewardsConfig { network: network_name.clone() });

        let rpc_url = if let Some(ref url) = self.rpc_url {
            println!("✓ Using RPC URL: {}", url);
            url.clone()
        } else {
            let mut rpc_url_prompt = Text::new("Enter RPC URL for this network:")
                .with_help_message("e.g., https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY");

            if let Some(existing_rpc) = existing_rewards.and_then(|r| r.rpc_url.as_ref()) {
                rpc_url_prompt = rpc_url_prompt.with_default(existing_rpc);
            }

            rpc_url_prompt.prompt()?
        };

        // Ask for staking address
        println!("\n--- Staking Address Configuration ---");
        println!("The staking address is the wallet used to stake ZKC tokens\n");

        let (staking_private_key, staking_address) = if let Some(ref pk) = self.private_key {
            let pk = pk.strip_prefix("0x").unwrap_or(pk).to_string();
            println!("✓ Using provided private key for staking address");
            let addr = Self::address_from_private_key(&pk);
            (Some(pk), addr.map(|a| format!("{:#x}", a)))
        } else {
            let has_existing_staking_key = existing_rewards.and_then(|r| r.staking_private_key.as_ref()).is_some();
            let staking_key_prompt = if has_existing_staking_key {
                Confirm::new("Do you want to update the stored staking private key?")
                    .with_default(false)
                    .prompt()?
            } else {
                Confirm::new("Do you want to store a private key for the staking address?")
                    .with_default(true)
                    .with_help_message("Skip if using a hardware wallet, smart contract wallet, or don't need to stake ZKC or delegate rewards")
                    .prompt()?
            };

            if staking_key_prompt {
                let pk = Text::new("Enter staking private key:")
                    .with_help_message("Will be stored in plaintext in ~/.boundless/secrets.toml")
                    .prompt()?;
                let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                let addr = Self::address_from_private_key(&pk);
                (Some(pk), addr.map(|a| format!("{:#x}", a)))
            } else {
                // Ask for public staking address
                let addr = Text::new("Enter staking address:")
                    .with_help_message("Public Ethereum address that holds your staked ZKC")
                    .prompt()?;
                (None, Some(addr))
            }
        };

        // Query on-chain for reward delegation if we have a staking address
        let (reward_private_key, reward_address) = if let Some(ref staking_addr) = staking_address {
            println!("\n--- Reward Address Configuration ---");
            println!("The reward address is the address with reward power (can receive delegated rewards)\n");

            // Try to query delegation on-chain
            print!("Checking if you've delegated rewards... ");
            std::io::stdout().flush()?;
            let delegated_address = Self::query_reward_delegation(&rpc_url, &network_name, staking_addr).await.ok().flatten();

            if let Some(ref delegated) = delegated_address {
                if delegated.to_lowercase() != staking_addr.to_lowercase() {
                    println!("{}", delegated);
                    let use_delegated = Confirm::new("Would you like to use this delegated address as your reward address?")
                        .with_default(true)
                        .prompt()?;

                    if use_delegated {
                        println!("✓ Using delegated address as reward address");
                        (None, Some(delegated.clone()))
                    } else {
                        Self::ask_for_reward_address(staking_addr, &staking_private_key).await?
                    }
                } else {
                    println!("No delegatee");
                    Self::ask_for_reward_address(staking_addr, &staking_private_key).await?
                }
            } else {
                println!("No delegatee");
                Self::ask_for_reward_address(staking_addr, &staking_private_key).await?
            }
        } else {
            println!("\n⚠️  No staking address provided - reward address configuration skipped");
            (None, None)
        };

        secrets.rewards = Some(RewardsSecrets {
            rpc_url: Some(rpc_url),
            staking_private_key,
            staking_address,
            reward_private_key,
            reward_address,
            private_key: None, // deprecated field
        });

        println!("\n✓ Rewards module configured successfully");

        Ok(())
    }

    async fn ask_for_reward_address(staking_addr: &str, staking_private_key: &Option<String>) -> Result<(Option<String>, Option<String>)> {
        let same_as_staking = Confirm::new("Is the reward address the same as the staking address?")
            .with_default(true)
            .with_help_message("The reward address is the address with reward power (e.g., if staking address has delegated to another address)")
            .prompt()?;

        if same_as_staking {
            println!("\n⚠️  WARN: Using the same address for reward and staking address is not recommended");
            println!("   See: https://docs.boundless.network/zkc/mining/wallet-setup\n");
            // If we have a staking private key, use it for reward address too
            Ok((staking_private_key.clone(), Some(staking_addr.to_string())))
        } else {
            let has_reward_key = Confirm::new("Do you want to store a private key for the reward address?")
                .with_default(false)
                .with_help_message("Skip if using a hardware wallet, smart contract wallet, or don't need to stake ZKC or delegate rewards")
                .prompt()?;

            if has_reward_key {
                let pk = Text::new("Enter reward private key:")
                    .with_help_message("Will be stored in plaintext in ~/.boundless/secrets.toml")
                    .prompt()?;
                let pk = pk.strip_prefix("0x").unwrap_or(&pk).to_string();
                let addr = Self::address_from_private_key(&pk);
                Ok((Some(pk), addr.map(|a| format!("{:#x}", a))))
            } else {
                let addr = Text::new("Enter reward address:")
                    .with_help_message("Public Ethereum address with reward power")
                    .prompt()?;
                Ok((None, Some(addr)))
            }
        }
    }

    async fn query_reward_delegation(rpc_url: &str, network: &str, address: &str) -> Result<Option<String>> {
        use alloy::primitives::Address;
        use alloy::providers::ProviderBuilder;

        // Determine veZKC address based on network
        let vezkc_address = match network {
            "mainnet" => "0xe8ae8ee8ffa57f6a79b6cbe06bafc0b05f3ffbf4",
            "sepolia" => "0xc23340732038ca6C5765763180E81B395d2e9cCA",
            _ => return Ok(None), // Can't query custom networks without deployment info
        };

        let addr: Address = address.parse().context("Invalid staking address")?;
        let vezkc_addr: Address = vezkc_address.parse()?;

        // Connect to provider
        let provider = ProviderBuilder::new().connect(rpc_url).await.context("Failed to connect to provider")?;

        // Define IRewards interface
        alloy::sol! {
            #[sol(rpc)]
            interface IRewards {
                function rewardDelegates(address account) external view returns (address);
            }
        }

        let rewards_contract = IRewards::new(vezkc_addr, &provider);
        let delegate = rewards_contract.rewardDelegates(addr).call().await.context("Failed to query reward delegation")?;

        Ok(Some(format!("{:#x}", delegate)))
    }

    fn address_from_private_key(pk: &str) -> Option<alloy::primitives::Address> {
        use alloy::signers::local::PrivateKeySigner;
        pk.parse::<PrivateKeySigner>().ok().map(|signer| signer.address())
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

        let vezkc_address =
            Text::new("veZKC contract address:").prompt()?.parse().context("Invalid address")?;

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
