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
    primitives::Address,
    providers::{Provider, ProviderBuilder},
};
use anyhow::{bail, Context, Result};
use clap::Args;

use crate::config::GlobalConfig;

/// Get the current delegate for an address
#[derive(Args, Clone, Debug)]
pub struct RewardsGetDelegate {
    /// Address to check delegation for
    pub address: Address,
}

impl RewardsGetDelegate {
    /// Run the get-delegate command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rpc_url = global_config.require_rewards_rpc_url()?;

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
        let vezkc_address = global_config
            .vezkc_address()
            .context("VEZKC_ADDRESS environment variable is required")?;

        // Define ERC721Votes interface inline for veZKC
        alloy::sol! {
            #[sol(rpc)]
            interface IERC721Votes {
                function delegates(address account) external view returns (address);
                function getVotes(address account) external view returns (uint256);
                function balanceOf(address owner) external view returns (uint256);
            }
        }

        // Create veZKC contract instance
        let vezkc = IERC721Votes::new(vezkc_address, &provider);

        // Query current delegate
        let delegate =
            vezkc.delegates(self.address).call().await.context("Failed to query delegate")?;

        // Check if self-delegated or delegated to another address
        if delegate == self.address {
            tracing::info!("Address {:#x} is self-delegated (no delegation)", self.address);
        } else if delegate == Address::ZERO {
            tracing::info!("Address {:#x} has not delegated voting power", self.address);
        } else {
            tracing::info!(
                "Address {:#x} has delegated voting power to {:#x}",
                self.address,
                delegate
            );
        }

        // Also query voting power to provide complete info
        let voting_power =
            vezkc.getVotes(self.address).call().await.context("Failed to query voting power")?;

        let balance =
            vezkc.balanceOf(self.address).call().await.context("Failed to query staked balance")?;

        if voting_power != balance {
            if voting_power > balance {
                tracing::info!(
                    "Note: Address has {} veZKC staked balance and {} veZKC voting power (received delegation)",
                    alloy::primitives::utils::format_ether(balance),
                    alloy::primitives::utils::format_ether(voting_power)
                );
            } else {
                tracing::info!(
                    "Note: Address has {} veZKC staked balance but only {} veZKC voting power (delegated away)",
                    alloy::primitives::utils::format_ether(balance),
                    alloy::primitives::utils::format_ether(voting_power)
                );
            }
        }

        Ok(())
    }
}
