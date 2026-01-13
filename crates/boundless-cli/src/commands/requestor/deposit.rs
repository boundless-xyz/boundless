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

use alloy::primitives::{utils::parse_ether, U256};
use anyhow::Result;
use clap::Args;

use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::RequestorConfigExt;
use crate::display::{format_eth, network_name_from_chain_id, DisplayManager};

/// Command to deposit funds into the market
#[derive(Args, Clone, Debug)]
pub struct RequestorDeposit {
    /// Amount in ether to deposit
    #[clap(value_parser = parse_ether)]
    pub amount: U256,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorDeposit {
    /// Run the deposit command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;
        requestor_config.require_private_key_with_help()?;

        let client =
            requestor_config.client_builder_with_signer(global_config.tx_timeout)?.build().await?;
        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);

        let display = DisplayManager::with_network(network_name);
        let formatted = format_eth(self.amount);

        display.header("Depositing funds (ETH) to Boundless Market");
        display.balance("Amount", &formatted, "ETH", "cyan");

        client.boundless_market.deposit(self.amount).await?;

        display.success(&format!("Successfully deposited {} ETH", formatted));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestContext;
    use predicates::str::contains;

    #[tokio::test]
    async fn test_deposit_help() {
        crate::test_common::BoundlessCmd::new("requestor", "deposit")
            .arg("--help")
            .assert()
            .success()
            .stdout(contains("Usage:"))
            .stdout(contains("deposit"));
    }

    #[tokio::test]
    async fn test_deposit_without_amount() {
        let ctx = TestContext::base().await;
        let account = ctx.account(0);

        ctx.cmd("requestor", "deposit").with_account(&account).assert().failure();
    }

    #[tokio::test]
    async fn test_deposit_executes() {
        let ctx = TestContext::base().await;
        let account = ctx.account(0);

        ctx.cmd("requestor", "deposit")
            .arg("0.01")
            .with_account(&account)
            .assert()
            .success()
            .stdout(contains("Depositing"))
            .stdout(contains("ETH"));
    }
}
