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

use crate::config::GlobalConfig;

/// Check the ZKC token balance of an address
#[derive(Args, Clone, Debug)]
pub struct RewardsBalanceZkc {
    /// Address to check the balance of
    pub address: Address,
}

impl RewardsBalanceZkc {
    /// Run the balance-zkc command
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

        // Get ZKC contract address
        let zkc_address =
            global_config.zkc_address().context("ZKC_ADDRESS environment variable is required")?;

        tracing::debug!("Using ZKC address: {:#x}", zkc_address);

        // Create ERC20 instance for the ZKC token (IZKC doesn't have standard ERC20 methods)
        let zkc_token = IERC20::new(zkc_address, &provider);

        // Query balance
        let balance = zkc_token
            .balanceOf(self.address)
            .call()
            .await
            .context("Failed to query ZKC balance")?;

        // Query token metadata
        let symbol = zkc_token.symbol().call().await.context("Failed to query ZKC symbol")?;

        let decimals = zkc_token.decimals().call().await.context("Failed to query ZKC decimals")?;

        // Display balance
        tracing::info!("ZKC balance for {:#x}:", self.address);
        tracing::info!("  {} {}", format_ether(balance), symbol);

        // Also show raw value for precision
        if balance > alloy::primitives::U256::ZERO {
            tracing::debug!("  Raw: {} (decimals: {})", balance, decimals);
        }

        Ok(())
    }
}
