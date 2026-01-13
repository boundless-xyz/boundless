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
use alloy::providers::Provider;
use anyhow::Result;
use clap::Args;

use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::{validate_address_input, RequestorConfigExt};
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

/// Command to check balance in the market
#[derive(Args, Clone, Debug)]
pub struct RequestorBalance {
    /// Address to check the balance of; if not provided, defaults to the wallet address
    pub address: Option<Address>,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorBalance {
    /// Run the balance command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // Load and validate config
        let requestor_config = self.requestor_config.clone().load_and_validate()?;

        // Validate address input with helpful error message
        let addr = validate_address_input(
            self.address,
            requestor_config.private_key.as_ref(),
            "balance query",
        )?;

        // Build client with standard configuration
        let client = requestor_config.client_builder(global_config.tx_timeout)?.build().await?;
        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);

        // Create display manager with network context
        let display = DisplayManager::with_network(network_name);

        // Query balances
        let deposited = client.boundless_market.balance_of(addr).await?;
        let available = client.provider().get_balance(addr).await?;

        // Display results using standardized formatting
        display.header("Balance (ETH) on Boundless Market");
        display.address("Address", addr);
        display.balance("Deposited", &format_eth(deposited), "ETH", "green");
        display.balance("Available", &format_eth(available), "ETH", "cyan");

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestContext;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_balance_with_address() {
        let ctx = TestContext::base().await;
        let account = ctx.account(0);

        ctx.cmd("requestor", "balance")
            .arg(&account.address)
            .assert()
            .success()
            .stdout(contains("Balance"))
            .stdout(contains("ETH"));
    }

    #[tokio::test]
    async fn test_balance_zero_address_fails() {
        let ctx = TestContext::base().await;

        ctx.cmd("requestor", "balance")
            .arg("0x0000000000000000000000000000000000000000")
            .assert()
            .success()
            .stdout(contains("Balance"))
            .stdout(contains("ETH"));
    }

    #[tokio::test]
    async fn test_balance_with_private_key() {
        let ctx = TestContext::base().await;
        let account = ctx.account(1);

        ctx.cmd("requestor", "balance")
            .arg(&account.address)
            .with_account(&account)
            .assert()
            .success()
            .stdout(contains(account.address.to_lowercase()));
    }

    #[tokio::test]
    async fn test_balance_help() {
        crate::test_common::BoundlessCmd::new("requestor", "balance")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("balance"));
    }
}
