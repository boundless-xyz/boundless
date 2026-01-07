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

use alloy::primitives::{utils::parse_units, U256};
use anyhow::{anyhow, bail, Context, Result};
use clap::Args;

use crate::config::{GlobalConfig, ProverConfig};
use crate::config_ext::ProverConfigExt;
use crate::contracts::{get_token_balance, get_token_info};
use crate::display::{format_token, network_name_from_chain_id, DisplayManager};

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
        let prover_config = self.prover_config.clone().load_and_validate()?;
        prover_config.require_private_key_with_help()?;

        let client = prover_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client with signer")?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        // Get collateral token information
        let collateral_token_address = client.boundless_market.collateral_token_address().await?;
        let token_info = get_token_info(client.provider(), collateral_token_address).await?;
        let collateral_label = format!("collateral ({})", token_info.symbol);

        // Parse and validate amount
        let parsed_amount = parse_units(&self.amount, token_info.decimals)
            .map_err(|e| anyhow!("Failed to parse amount: {}", e))?
            .into();

        if parsed_amount == U256::from(0) {
            bail!("Amount is below the denomination minimum: {}", self.amount);
        }

        let formatted_amount = format_token(parsed_amount, token_info.decimals)?;

        display.header(&format!("Withdrawing {} from Boundless Market", collateral_label));
        display.balance("Amount", &formatted_amount, &token_info.symbol, "cyan");

        client.boundless_market.withdraw_collateral(parsed_amount).await?;

        display.success(&format!(
            "Successfully withdrew {}: {} {}",
            collateral_label, formatted_amount, token_info.symbol
        ));

        // Display updated balance
        let addr = client.boundless_market.caller();
        let deposited = client.boundless_market.balance_of_collateral(addr).await?;
        let available =
            get_token_balance(client.provider(), collateral_token_address, addr).await?;

        let deposited_formatted = format_token(deposited, token_info.decimals)?;
        let available_formatted = format_token(available, token_info.decimals)?;

        display.address("Address", addr);
        display.balance("Deposited", &deposited_formatted, &token_info.symbol, "green");
        display.balance("Available", &available_formatted, &token_info.symbol, "cyan");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_withdraw_collateral_help() {
        test_common::BoundlessCmd::new("prover", "withdraw-collateral")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("withdraw-collateral"));
    }
}
