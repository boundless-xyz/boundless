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
    primitives::{utils::format_ether, Address},
    providers::{Provider, ProviderBuilder},
};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::token::IERC20;
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RewardsConfig};

/// Check the ZKC token balance of an address
#[derive(Args, Clone, Debug)]
pub struct RewardsBalanceZkc {
    /// Address to check the balance of (if not provided, checks staking and reward addresses from config)
    pub address: Option<Address>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsBalanceZkc {
    /// Run the balance-zkc command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Get ZKC contract address (supports mainnet and Sepolia deployments)
        let zkc_address = rewards_config.zkc_address()?;

        // Create ERC20 instance for the ZKC token
        let zkc_token = IERC20::new(zkc_address, &provider);

        // Query token metadata once
        let symbol = zkc_token.symbol().call().await.context("Failed to query ZKC symbol")?;

        // If address is provided, just check that one
        if let Some(address) = self.address {
            let balance = zkc_token
                .balanceOf(address)
                .call()
                .await
                .context("Failed to query ZKC balance")?;

            let balance_formatted = crate::format_amount(&format_ether(balance));

            println!("\n{} [{}]", "ZKC Balance".bold(), network_name.blue().bold());
            println!("  Address: {}", format!("{:#x}", address).dimmed());
            println!("  Balance: {} {}", balance_formatted.green().bold(), symbol.green());
        } else {
            // No address provided - check both staking and reward addresses from config
            use crate::config_file::Secrets;

            let secrets = Secrets::load().context("Failed to load secrets - no addresses configured")?;
            let rewards_secrets = secrets.rewards.context("Rewards module not configured")?;

            let mut checked_addresses = Vec::new();

            // Get staking address
            let staking_addr_opt = rewards_secrets.staking_address.or_else(|| {
                rewards_secrets.staking_private_key.as_ref().and_then(|pk| {
                    pk.parse::<alloy::signers::local::PrivateKeySigner>()
                        .ok()
                        .map(|s| format!("{:#x}", s.address()))
                })
            });

            // Get reward address
            let reward_addr_opt = rewards_secrets.reward_address.or_else(|| {
                rewards_secrets.reward_private_key.as_ref().and_then(|pk| {
                    pk.parse::<alloy::signers::local::PrivateKeySigner>()
                        .ok()
                        .map(|s| format!("{:#x}", s.address()))
                })
            });

            // Always show both staking and reward addresses separately
            if let Some(staking_addr_str) = staking_addr_opt {
                let staking_addr: Address = staking_addr_str.parse().context("Invalid staking address")?;
                checked_addresses.push(("Staking", staking_addr));
            }

            if let Some(reward_addr_str) = reward_addr_opt {
                let reward_addr: Address = reward_addr_str.parse().context("Invalid reward address")?;
                checked_addresses.push(("Reward", reward_addr));
            }

            if checked_addresses.is_empty() {
                bail!("No addresses configured in rewards module. Please run 'boundless setup rewards' or provide an address");
            }

            println!("\n{} [{}]", "ZKC Balance".bold(), network_name.blue().bold());

            // Query and display balance for each address
            for (i, (label, address)) in checked_addresses.iter().enumerate() {
                let balance = zkc_token
                    .balanceOf(*address)
                    .call()
                    .await
                    .with_context(|| format!("Failed to query ZKC balance for {} address", label))?;

                let balance_formatted = crate::format_amount(&format_ether(balance));

                if i > 0 {
                    println!();
                }
                println!("  {} Address: {}", label.bold(), format!("{:#x}", address).dimmed());
                println!("  Balance:        {} {}", balance_formatted.green().bold(), symbol.green());
            }
        }

        Ok(())
    }
}
