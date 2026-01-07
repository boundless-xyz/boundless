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

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
};
use anyhow::Context;
use anyhow::Result;
use boundless_market::contracts::token::IERC20;
use boundless_zkc::contracts::IStaking;
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};
use crate::config_ext::RewardsConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

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
        let rewards_config = self.rewards_config.load_and_validate()?;
        let rpc_url = rewards_config.require_rpc_url_with_help()?;

        // Get address - from argument or from config
        let address = if let Some(addr) = self.address {
            addr
        } else {
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

            let staking_addr_str = rewards_secrets
                .staking_address
                .as_ref()
                .cloned()
                .or_else(|| {
                    rewards_secrets.staking_private_key.as_ref().and_then(|pk| {
                        pk.parse::<alloy::signers::local::PrivateKeySigner>()
                            .ok()
                            .map(|s| format!("{:#x}", s.address()))
                    })
                })
                .context(
                    "No staking address configured. Please run 'boundless rewards setup' or provide an address",
                )?;

            staking_addr_str.parse().context("Invalid staking address")?
        };

        let provider = ProviderBuilder::new()
            .connect(&rpc_url)
            .await
            .with_context(|| format!("Failed to connect to {}", rpc_url))?;

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let network_name = network_name_from_chain_id(Some(chain_id));

        let vezkc_address = rewards_config.vezkc_address()?;
        let zkc_address = rewards_config.zkc_address()?;
        let display = DisplayManager::with_network(network_name);

        alloy::sol! {
            #[sol(rpc)]
            interface IRewards {
                function getStakingRewards(address account) external view returns (uint256);
                function rewardDelegates(address account) external view returns (address);
            }
        }

        let staking = IStaking::new(vezkc_address, &provider);
        let rewards = IRewards::new(vezkc_address, &provider);
        let zkc_token = IERC20::new(zkc_address, &provider);

        let staked_result = staking
            .getStakedAmountAndWithdrawalTime(address)
            .call()
            .await
            .context("Failed to query staked ZKC balance")?;
        let staked_balance = staked_result.amount;

        let available_balance = zkc_token
            .balanceOf(address)
            .call()
            .await
            .context("Failed to query available ZKC balance")?;

        let reward_power = rewards
            .getStakingRewards(address)
            .call()
            .await
            .context("Failed to query reward power")?;

        let reward_delegate = rewards
            .rewardDelegates(address)
            .call()
            .await
            .context("Failed to query reward delegate")?;

        let symbol = zkc_token.symbol().call().await.context("Failed to query ZKC symbol")?;

        display.header("Staking Address Balance");
        display.address("Address", address);
        display.balance("Staked", &format_eth(staked_balance), &symbol, "green");
        display.balance("Available", &format_eth(available_balance), &symbol, "cyan");

        if reward_delegate != address {
            display.item("Reward Power", format!("0 (delegated to {:#x})", reward_delegate));
        } else {
            display.balance("Reward Power", &format_eth(reward_power), "", "yellow");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestContext;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_staked_balance_zkc_with_address() {
        let ctx = TestContext::ethereum().await;
        let account = ctx.account(0);

        ctx.cmd("rewards", "staked-balance-zkc")
            .arg(&account.address)
            .assert()
            .success()
            .stdout(contains("Staking Address Balance"));
    }

    #[tokio::test]
    async fn test_staked_balance_zkc_help() {
        crate::test_common::BoundlessCmd::new("rewards", "staked-balance-zkc")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("staked-balance-zkc"));
    }
}
