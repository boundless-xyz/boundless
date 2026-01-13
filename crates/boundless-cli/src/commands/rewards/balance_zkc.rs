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

use alloy::primitives::Address;
use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::token::IERC20;
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

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
        // Load and validate configuration
        let rewards_config = self.rewards_config.clone().load_and_validate()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;

        // Connect to provider directly without abstraction
        let provider = ProviderBuilder::new()
            .connect(&rpc_url)
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let network_name = network_name_from_chain_id(Some(chain_id));

        // Create display manager with network context
        let display = DisplayManager::with_network(network_name);

        // Get ZKC contract address (supports mainnet and Sepolia deployments)
        let zkc_address = rewards_config.zkc_address()?;

        // Get token metadata
        let zkc_token = IERC20::new(zkc_address, &provider);
        let symbol = zkc_token.symbol().call().await.context("Failed to query ZKC symbol")?;

        // If address is provided, just check that one
        if let Some(address) = self.address {
            let zkc_token_balance = IERC20::new(zkc_address, &provider);
            let balance = zkc_token_balance
                .balanceOf(address)
                .call()
                .await
                .context("Failed to get token balance")?;

            display.header("ZKC Balance");
            display.address("Address", address);
            display.balance("Balance", &format_eth(balance), &symbol, "green");
        } else {
            // No address provided - check both staking and reward addresses from config
            use crate::config_file::{Config, Secrets};

            let config =
                Config::load().context("Failed to load config - run 'boundless rewards setup'")?;
            let network = config
                .rewards
                .as_ref()
                .context("Rewards module not configured - run 'boundless rewards setup'")?
                .network
                .clone();

            let secrets =
                Secrets::load().context("Failed to load secrets - no addresses configured")?;
            let rewards_secrets = secrets.rewards_networks.get(&network).context(
                "No rewards secrets found for current network - run 'boundless rewards setup'",
            )?;

            let mut checked_addresses = Vec::new();

            // Get staking address
            let staking_addr_opt =
                rewards_secrets.staking_address.as_ref().cloned().or_else(|| {
                    rewards_secrets.staking_private_key.as_ref().and_then(|pk| {
                        pk.parse::<alloy::signers::local::PrivateKeySigner>()
                            .ok()
                            .map(|s| format!("{:#x}", s.address()))
                    })
                });

            // Get reward address
            let reward_addr_opt = rewards_secrets.reward_address.as_ref().cloned().or_else(|| {
                rewards_secrets.reward_private_key.as_ref().and_then(|pk| {
                    pk.parse::<alloy::signers::local::PrivateKeySigner>()
                        .ok()
                        .map(|s| format!("{:#x}", s.address()))
                })
            });

            // Always show both staking and reward addresses separately
            if let Some(staking_addr_str) = staking_addr_opt {
                let staking_addr: Address =
                    staking_addr_str.parse().context("Invalid staking address")?;
                checked_addresses.push(("Staking", staking_addr));
            }

            if let Some(reward_addr_str) = reward_addr_opt {
                let reward_addr: Address =
                    reward_addr_str.parse().context("Invalid reward address")?;
                checked_addresses.push(("Reward", reward_addr));
            }

            if checked_addresses.is_empty() {
                bail!("No addresses configured in rewards module. Please run 'boundless rewards setup' or provide an address");
            }

            display.header("ZKC Balance");

            // Query and display balance for each address
            for (i, (label, address)) in checked_addresses.iter().enumerate() {
                let zkc_token_bal = IERC20::new(zkc_address, &provider);
                let balance =
                    zkc_token_bal.balanceOf(*address).call().await.with_context(|| {
                        format!("Failed to query ZKC balance for {} address", label)
                    })?;

                if i > 0 {
                    println!();
                }
                display.item(&format!("{} Address", label), format!("{:#x}", address));
                display.balance("Balance", &format_eth(balance), &symbol, "green");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestContext;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_balance_zkc_with_address() {
        let ctx = TestContext::ethereum().await;
        let account = ctx.account(0);

        ctx.cmd("rewards", "balance-zkc")
            .arg(&account.address)
            .assert()
            .success()
            .stdout(contains("ZKC Balance"));
    }

    #[tokio::test]
    async fn test_balance_zkc_zero_address() {
        let ctx = TestContext::ethereum().await;

        ctx.cmd("rewards", "balance-zkc")
            .arg("0x0000000000000000000000000000000000000000")
            .assert()
            .success()
            .stdout(contains("ZKC Balance"));
    }

    #[tokio::test]
    async fn test_balance_zkc_help() {
        crate::test_common::BoundlessCmd::new("rewards", "balance-zkc")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("balance-zkc"));
    }
}
