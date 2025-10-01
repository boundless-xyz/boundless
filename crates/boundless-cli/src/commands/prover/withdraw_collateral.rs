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

use crate::config::GlobalConfig;

/// Withdraw collateral funds from the market
#[derive(Args, Clone, Debug)]
pub struct ProverWithdrawCollateral {
    /// Amount to withdraw in HP or USDC based on the chain ID.
    pub amount: String,
}

impl ProverWithdrawCollateral {
    /// Run the withdraw collateral command
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

        let formatted_amount = format_units(parsed_amount, decimals)?;

        tracing::info!("Withdrawing {} {} from collateral", formatted_amount, symbol);
        client.boundless_market.withdraw_collateral(parsed_amount).await?;
        tracing::info!("Successfully withdrew {} {} from collateral", formatted_amount, symbol);

        Ok(())
    }
}