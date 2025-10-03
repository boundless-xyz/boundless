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
    primitives::{utils::format_units, Address},
};
use anyhow::{bail, Context, Result};
use clap::Args;
use colored::Colorize;

use crate::config::GlobalConfig;

/// Check the collateral balance of an account in the market
#[derive(Args, Clone, Debug)]
pub struct ProverBalanceCollateral {
    /// Address to check the balance of; if not provided, defaults to the wallet address
    pub address: Option<Address>,
}

impl ProverBalanceCollateral {
    /// Run the balance collateral command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // If address is provided, use it; otherwise try to get it from configured private key
        let addr = if let Some(addr) = self.address {
            addr
        } else if let Some(ref pk) = global_config.private_key {
            pk.address()
        } else {
            bail!(
                "No address specified for collateral balance query.\n\n\
                To configure a default address: run 'boundless setup prover'\n\
                Or provide an address: boundless prover balance-collateral <ADDRESS>"
            );
        };

        let client = global_config.client_builder()?.build().await
            .context("Failed to build Boundless Client")?;

        let symbol = client.boundless_market.collateral_token_symbol().await?;
        let decimals = client.boundless_market.collateral_token_decimals().await?;

        // Query both deposited and available balances
        let deposited = client.boundless_market.balance_of_collateral(addr).await?;

        // Get available balance by querying the ERC20 token directly
        let collateral_token_address = client.boundless_market.collateral_token_address().await?;
        let token = boundless_market::contracts::token::IERC20::new(collateral_token_address, client.provider());
        let available = token.balanceOf(addr).call().await
            .context("Failed to query collateral token balance")?;

        let deposited_formatted = crate::format_amount(&format_units(deposited, decimals)?);
        let available_formatted = crate::format_amount(&format_units(available, decimals)?);
        let network_name = crate::network_name_from_chain_id(client.deployment.chain_id);

        println!("\n{} [{}]", "Collateral Balance".bold(), network_name.blue().bold());
        println!("  Address:   {}", format!("{:#x}", addr).dimmed());
        println!("  Deposited: {} {}", deposited_formatted.green().bold(), symbol.green());
        println!("  Available: {} {}", available_formatted.cyan().bold(), symbol.cyan());

        Ok(())
    }
}