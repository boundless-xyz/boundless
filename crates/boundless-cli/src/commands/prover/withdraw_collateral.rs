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

use alloy::primitives::{
    utils::{format_units, parse_units},
    U256,
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, ProverConfig};

/// Withdraw collateral funds from the market
#[derive(Args, Clone, Debug)]
pub struct ProverWithdrawCollateral {
    /// Amount to withdraw in HP or USDC based on the chain ID.
    pub amount: String,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}

impl ProverWithdrawCollateral {
    /// Run the withdraw collateral command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let prover_config = self.prover_config.clone().load_from_files()?;
        let client = prover_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client with signer")?;

        // Parse the amount based on the collateral token's decimals
        let symbol = client.boundless_market.collateral_token_symbol().await?;
        let collateral_label = format!("collateral ({symbol})");
        let collateral_title = format!("Collateral ({symbol})");
        let decimals = client.boundless_market.collateral_token_decimals().await?;
        let parsed_amount = parse_units(&self.amount, decimals)
            .map_err(|e| anyhow!("Failed to parse amount: {}", e))?
            .into();

        if parsed_amount == U256::from(0) {
            bail!("Amount is below the denomination minimum: {}", self.amount);
        }

        let formatted_amount = crate::format_amount(&format_units(parsed_amount, decimals)?);
        let network_name = crate::network_name_from_chain_id(client.deployment.market_chain_id);

        println!(
            "\n{} [{}]",
            format!("Withdrawing {collateral_label} from Boundless Market").bold(),
            network_name.blue().bold()
        );
        println!(
            "  Amount: {} {}",
            formatted_amount.as_str().cyan().bold(),
            symbol.as_str().cyan()
        );

        client.boundless_market.withdraw_collateral(parsed_amount).await?;

        println!(
            "\n{} Successfully withdrew {}: {} {}",
            "✓".green().bold(),
            collateral_label.as_str().green().bold(),
            formatted_amount.as_str().green().bold(),
            symbol.as_str().green()
        );

        // Display updated balance
        let addr = client.boundless_market.caller();
        let deposited = client.boundless_market.balance_of_collateral(addr).await?;
        let collateral_token_address = client.boundless_market.collateral_token_address().await?;
        let token = boundless_market::contracts::token::IERC20::new(
            collateral_token_address,
            client.provider(),
        );
        let available = token.balanceOf(addr).call().await?;

        let deposited_formatted = crate::format_amount(&format_units(deposited, decimals)?);
        let available_formatted = crate::format_amount(&format_units(available, decimals)?);

        println!("  Address:   {}", format!("{:#x}", addr).dimmed());
        println!(
            "  Deposited: {} {}",
            deposited_formatted.as_str().green().bold(),
            symbol.as_str().green()
        );
        println!(
            "  Available: {} {}",
            available_formatted.as_str().cyan().bold(),
            symbol.as_str().cyan()
        );

        Ok(())
    }
}
