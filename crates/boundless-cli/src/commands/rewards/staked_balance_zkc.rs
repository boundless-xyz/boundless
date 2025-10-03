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
use clap::Args;

use crate::config::{GlobalConfig, RewardsConfig};

/// Check the staked ZKC balance (veZKC) of an address
#[derive(Args, Clone, Debug)]
pub struct RewardsStakedBalanceZkc {
    /// Address to check the staked balance of
    pub address: Address,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsStakedBalanceZkc {
    /// Run the staked-balance-zkc command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        // Connect to provider
        let provider = ProviderBuilder::new()
            .connect(rpc_url.as_str())
            .await
            .context("Failed to connect to Ethereum provider")?;

        // Verify we're on mainnet (chain ID 1)
        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        if chain_id != 1 {
            bail!("Rewards commands require connection to Ethereum mainnet (chain ID 1), got chain ID {}", chain_id);
        }

        // Get veZKC (staking) contract address
        let vezkc_address = rewards_config.vezkc_address()?;

        // Define ERC721Votes interface inline for veZKC
        alloy::sol! {
            #[sol(rpc)]
            interface IERC721Votes {
                function balanceOf(address owner) external view returns (uint256);
                function getVotes(address account) external view returns (uint256);
                function delegates(address account) external view returns (address);
            }
        }

        // Create veZKC contract instance with ERC721Votes interface
        let vezkc = IERC721Votes::new(vezkc_address, &provider);

        // Query staked balance
        let staked_balance = vezkc
            .balanceOf(self.address)
            .call()
            .await
            .context("Failed to query staked ZKC balance")?;

        // Query voting power (which may differ from balance due to delegation)
        let voting_power =
            vezkc.getVotes(self.address).call().await.context("Failed to query voting power")?;

        // Display staked balance
        tracing::info!("Staked ZKC (veZKC) for {:#x}:", self.address);
        tracing::info!("  Staked balance: {} veZKC", format_ether(staked_balance));
        tracing::info!("  Voting power: {} veZKC", format_ether(voting_power));

        // Show if there's a delegation difference
        if staked_balance != voting_power {
            if voting_power > staked_balance {
                tracing::info!(
                    "  Note: Address has received {} veZKC in delegated voting power",
                    format_ether(voting_power - staked_balance)
                );
            } else {
                tracing::info!(
                    "  Note: Address has delegated {} veZKC voting power to another address",
                    format_ether(staked_balance - voting_power)
                );
            }
        }

        Ok(())
    }
}
