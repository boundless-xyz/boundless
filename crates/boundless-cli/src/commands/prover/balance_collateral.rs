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

use alloy::primitives::Address;
use anyhow::{Context, Result};
use clap::Args;

use crate::config::{GlobalConfig, ProverConfig};
use crate::config_ext::{validate_prover_address_input, ProverConfigExt};
use crate::contracts::{get_token_balance, get_token_info};
use crate::display::{format_token, network_name_from_chain_id, DisplayManager};

/// Check the collateral balance of an account in the market
#[derive(Args, Clone, Debug)]
pub struct ProverBalanceCollateral {
    /// Address to check the balance of; if not provided, defaults to the wallet address
    pub address: Option<Address>,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}

impl ProverBalanceCollateral {
    /// Run the balance collateral command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Load and validate configuration
        let prover_config = self.prover_config.clone().load_and_validate()?;

        // Validate address input with helpful error message (supports read-only mode)
        let addr = validate_prover_address_input(
            self.address,
            prover_config.prover_address,
            prover_config.private_key.as_ref(),
            "collateral balance query",
        )?;

        // Build client with standard configuration
        let client = prover_config
            .client_builder(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client")?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);

        // Create display manager with network context
        let display = DisplayManager::with_network(network_name);

        // Get collateral token information
        let collateral_token_address = client.boundless_market.collateral_token_address().await?;
        let token_info = get_token_info(client.provider(), collateral_token_address).await?;

        // Query both deposited and available balances
        let deposited = client.boundless_market.balance_of_collateral(addr).await?;
        let available =
            get_token_balance(client.provider(), collateral_token_address, addr).await?;

        // Format balances using token decimals
        let deposited_formatted = format_token(deposited, token_info.decimals)?;
        let available_formatted = format_token(available, token_info.decimals)?;

        // Display results using standardized formatting
        display.header(&format!("Collateral ({}) Balance on Boundless Market", token_info.symbol));
        display.address("Address", addr);
        display.balance("Deposited", &deposited_formatted, &token_info.symbol, "green");
        display.balance("Available", &available_formatted, &token_info.symbol, "cyan");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestContext;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_balance_collateral_with_address() {
        let ctx = TestContext::base().await;
        let account = ctx.account(0);

        ctx.cmd("prover", "balance-collateral")
            .arg(&account.address)
            .assert()
            .success()
            .stdout(contains("Collateral"))
            .stdout(contains("Balance"));
    }

    #[tokio::test]
    async fn test_balance_collateral_zero_address_fails() {
        let ctx = TestContext::base().await;

        ctx.cmd("prover", "balance-collateral")
            .arg("0x0000000000000000000000000000000000000000")
            .assert()
            .success()
            .stdout(contains("Collateral"))
            .stdout(contains("Balance"));
    }

    #[tokio::test]
    async fn test_balance_collateral_with_private_key() {
        let ctx = TestContext::base().await;
        let account = ctx.account(1);

        ctx.cmd("prover", "balance-collateral")
            .arg(&account.address)
            .with_account(&account)
            .assert()
            .success()
            .stdout(contains("Collateral"))
            .stdout(contains("Balance"));
    }

    #[tokio::test]
    async fn test_balance_collateral_help() {
        crate::test_common::BoundlessCmd::new("prover", "balance-collateral")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("balance-collateral"));
    }
}
