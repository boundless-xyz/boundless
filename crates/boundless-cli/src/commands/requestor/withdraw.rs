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
    utils::{format_ether, parse_ether},
    U256,
};
use anyhow::Result;
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RequestorConfig};

/// Command to withdraw funds from the market
#[derive(Args, Clone, Debug)]
pub struct RequestorWithdraw {
    /// Amount in ether to withdraw
    #[clap(value_parser = parse_ether)]
    pub amount: U256,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorWithdraw {
    /// Run the withdraw command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_from_files()?;

        let client = requestor_config.client_builder_with_signer(global_config.tx_timeout)?.build().await?;
        let formatted = crate::format_amount(&format_ether(self.amount));
        let network_name = crate::network_name_from_chain_id(client.deployment.market_chain_id);

        println!(
            "\n{} [{}]",
            "Withdrawing funds (ETH) from Boundless Market".bold(),
            network_name.blue().bold()
        );
        println!("  Amount: {} {}", formatted.cyan().bold(), "ETH".cyan());

        client.boundless_market.withdraw(self.amount).await?;

        println!(
            "\n{} Successfully withdrew {} {}",
            "✓".green().bold(),
            formatted.green().bold(),
            "ETH".green()
        );
        Ok(())
    }
}
