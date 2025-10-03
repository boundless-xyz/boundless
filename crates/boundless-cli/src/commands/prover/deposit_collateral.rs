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
    primitives::{utils::{format_units, parse_units}, U256},
};
use anyhow::{anyhow, bail, Context, Result};
use clap::Args;
use colored::Colorize;

use crate::config::GlobalConfig;

/// Deposit collateral funds into the market
#[derive(Args, Clone, Debug)]
pub struct ProverDepositCollateral {
    /// Amount to deposit in HP or USDC based on the chain ID.
    pub amount: String,
}

impl ProverDepositCollateral {
    /// Run the deposit collateral command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let client = global_config.client_builder_with_signer()?.build().await
            .context("Failed to build Boundless Client with signer")?;

        // Parse the amount based on the collateral token's decimals
        let symbol = client.boundless_market.collateral_token_symbol().await?;
        let decimals = client.boundless_market.collateral_token_decimals().await?;
        let parsed_amount = parse_units(&self.amount, decimals)
            .map_err(|e| anyhow!("Failed to parse amount: {}", e))?.into();

        if parsed_amount == U256::from(0) {
            bail!("Amount is below the denomination minimum: {}", self.amount);
        }

        let formatted_amount = crate::format_amount(&format_units(parsed_amount, decimals)?);
        let network_name = crate::network_name_from_chain_id(client.deployment.chain_id);

        println!("\n{} [{}]", "Depositing collateral to Boundless Market".bold(), network_name.blue().bold());
        println!("  Amount: {} {}", formatted_amount.cyan().bold(), symbol.cyan());

        // Check if collateral token supports permit (EIP-2612)
        if !client.deployment.collateral_token_supports_permit() {
            println!("  {} Approving token...", "→".dimmed());
            client.boundless_market.approve_deposit_collateral(parsed_amount).await?;

            println!("  {} Depositing...", "→".dimmed());
            match client.boundless_market.deposit_collateral(parsed_amount).await {
                Ok(_) => {
                    println!("\n{} Successfully deposited {} {}", "✓".green().bold(), formatted_amount.green().bold(), symbol.green());
                    Ok(())
                }
                Err(e) => {
                    if e.to_string().contains("TRANSFER_FROM_FAILED") {
                        let addr = client.boundless_market.caller();
                        Err(anyhow!(
                            "Failed to deposit collateral: Ensure your address ({}) has funds on the {} contract",
                            addr, symbol
                        ))
                    } else {
                        Err(anyhow!("Failed to deposit collateral: {}", e))
                    }
                }
            }
        } else {
            println!("  {} Depositing with permit...", "→".dimmed());
            match client.boundless_market
                .deposit_collateral_with_permit(parsed_amount, &client.signer.unwrap())
                .await
            {
                Ok(_) => {
                    println!("\n{} Successfully deposited {} {}", "✓".green().bold(), formatted_amount.green().bold(), symbol.green());
                    Ok(())
                }
                Err(e) => {
                    if e.to_string().contains("TRANSFER_FROM_FAILED") {
                        let addr = client.boundless_market.caller();
                        Err(anyhow!(
                            "Failed to deposit collateral: Ensure your address ({}) has funds on the {} contract",
                            addr, symbol
                        ))
                    } else {
                        Err(anyhow!("Failed to deposit collateral: {}", e))
                    }
                }
            }
        }
    }
}