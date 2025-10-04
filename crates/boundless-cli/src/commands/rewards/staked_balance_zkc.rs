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

/// Check the staked ZKC balance (veZKC) of an address
#[derive(Args, Clone, Debug)]
pub struct RewardsStakedBalanceZkc {
    /// Address to check the staked balance of (if not provided, uses staking address from config)
    pub address: Option<Address>,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsStakedBalanceZkc {
    /// Run the staked-balance-zkc command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Get address - from argument or from config
        let address = if let Some(addr) = self.address {
            addr
        } else {
            // Get staking address from config
            use crate::config_file::{Config, Secrets};

            let config = Config::load().context("Failed to load config - run 'boundless setup rewards'")?;
            let network = config
                .rewards
                .as_ref()
                .context("Rewards module not configured - run 'boundless setup rewards'")?
                .network
                .clone();

            let secrets = Secrets::load().context("Failed to load secrets - no addresses configured")?;
            let rewards_secrets = secrets
                .rewards_networks
                .get(&network)
                .context("No rewards secrets found for current network - run 'boundless setup rewards'")?;

            let staking_addr_str = rewards_secrets.staking_address.as_ref().cloned().or_else(|| {
                rewards_secrets.staking_private_key.as_ref().and_then(|pk| {
                    pk.parse::<alloy::signers::local::PrivateKeySigner>()
                        .ok()
                        .map(|s| format!("{:#x}", s.address()))
                })
            }).context("No staking address configured. Please run 'boundless setup rewards' or provide an address")?;

            staking_addr_str.parse().context("Invalid staking address")?
        };

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        // Get chain ID to determine deployment
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = crate::network_name_from_chain_id(Some(chain_id));

        // Get veZKC (staking) contract address
        let vezkc_address = rewards_config.vezkc_address()?;

        // Get ZKC contract address
        let zkc_address = rewards_config.zkc_address()?;

        // Define interfaces for veZKC
        alloy::sol! {
            #[sol(rpc)]
            interface IERC721Votes {
                function balanceOf(address owner) external view returns (uint256);
            }

            #[sol(rpc)]
            interface IRewards {
                function getStakingRewards(address account) external view returns (uint256);
                function rewardDelegates(address account) external view returns (address);
            }
        }

        // Create contract instances
        let vezkc = IERC721Votes::new(vezkc_address, &provider);
        let rewards = IRewards::new(vezkc_address, &provider);
        let zkc_token = IERC20::new(zkc_address, &provider);

        // Query staked balance
        let staked_balance = vezkc
            .balanceOf(address)
            .call()
            .await
            .context("Failed to query staked ZKC balance")?;

        // Query available (unstaked) ZKC balance
        let available_balance = zkc_token
            .balanceOf(address)
            .call()
            .await
            .context("Failed to query available ZKC balance")?;

        // Query reward power (staking rewards for this address)
        let reward_power =
            rewards.getStakingRewards(address).call().await.context("Failed to query reward power")?;

        // Query reward delegate
        let reward_delegate =
            rewards.rewardDelegates(address).call().await.context("Failed to query reward delegate")?;

        // Query token symbol
        let symbol = zkc_token.symbol().call().await.context("Failed to query ZKC symbol")?;

        // Format balances
        let staked_formatted = crate::format_amount(&format_ether(staked_balance));
        let available_formatted = crate::format_amount(&format_ether(available_balance));

        println!("\n{} [{}]", "Staking Address Balance".bold(), network_name.blue().bold());
        println!("  Address:   {}", format!("{:#x}", address).dimmed());
        println!("  Staked:    {} {}", staked_formatted.green().bold(), symbol.green());
        println!("  Available: {} {}", available_formatted.cyan().bold(), symbol.cyan());

        // Show reward power with delegation info
        if reward_delegate != address {
            // Rewards are delegated to another address
            let reward_power_formatted = crate::format_amount(&format_ether(reward_power));
            println!(
                "  Reward Power: {} {}",
                "0".yellow().bold(),
                format!("[delegated {} to {:#x}]", reward_power_formatted, reward_delegate).dimmed()
            );
        } else {
            // Not delegated, show actual reward power
            let reward_power_formatted = crate::format_amount(&format_ether(reward_power));
            println!("  Reward Power: {}", reward_power_formatted.yellow().bold());
        }

        Ok(())
    }
}
