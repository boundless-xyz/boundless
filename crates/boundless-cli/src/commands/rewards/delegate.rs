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

use crate::config::{GlobalConfig, RewardsConfig};

/// Delegate voting power to another address
#[derive(Args, Clone, Debug)]
pub struct RewardsDelegate {
    /// Address to delegate voting power to
    pub delegatee: Address,

    /// Rewards configuration (RPC URL, private key, ZKC contract address)
    #[clap(flatten)]
    pub rewards_config: RewardsConfig,
}

impl RewardsDelegate {
    /// Run the delegate command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;
        let rpc_url = rewards_config.require_rpc_url()?;

        let signer = rewards_config.require_private_key()?;

        // Connect to provider with signer
        let provider = ProviderBuilder::new().wallet(signer.clone()).on_http(rpc_url);

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
                function delegate(address delegatee) external;
                function delegates(address account) external view returns (address);
            }
        }

        // Create veZKC contract instance
        let vezkc = IERC721Votes::new(vezkc_address, &provider);

        // Check current delegation
        let current_delegate = vezkc
            .delegates(signer.address())
            .call()
            .await
            .context("Failed to query current delegate")?;

        if current_delegate == self.delegatee {
            tracing::info!("Voting power is already delegated to {:#x}", self.delegatee);
            return Ok(());
        }

        tracing::info!("Current delegate: {:#x}", current_delegate);
        tracing::info!("Delegating voting power to {:#x}...", self.delegatee);

        // Execute delegation transaction
        let tx = vezkc
            .delegate(self.delegatee)
            .send()
            .await
            .context("Failed to send delegation transaction")?;

        tracing::info!("Transaction sent: {:#x}", tx.tx_hash());
        tracing::info!("Waiting for confirmation...");

        // Wait for transaction confirmation
        let tx_hash = tx.watch().await.context("Failed to wait for transaction confirmation")?;

        tracing::info!("Successfully delegated voting power to {:#x}!", self.delegatee);
        tracing::info!("Transaction: {:#x}", tx_hash);

        Ok(())
    }
}
