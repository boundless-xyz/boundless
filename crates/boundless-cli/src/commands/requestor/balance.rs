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

use alloy::primitives::{utils::format_ether, Address};
use alloy::providers::Provider;
use anyhow::{bail, Result};
use clap::Args;
use colored::Colorize;

use crate::config::GlobalConfig;

/// Command to check balance in the market
#[derive(Args, Clone, Debug)]
pub struct RequestorBalance {
    /// Address to check the balance of; if not provided, defaults to the wallet address
    pub address: Option<Address>,
}

impl RequestorBalance {
    /// Run the balance command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        // If address is provided, use it; otherwise try to get it from configured private key
        let addr = if let Some(addr) = self.address {
            addr
        } else if let Some(ref pk) = global_config.private_key {
            pk.address()
        } else {
            bail!(
                "No address specified for balance query.\n\n\
                To configure a default address: run 'boundless setup requestor'\n\
                Or provide an address: boundless requestor balance <ADDRESS>"
            );
        };

        let client = global_config.build_client().await?;
        let deposited = client.boundless_market.balance_of(addr).await?;
        let deposited_formatted = crate::format_amount(&format_ether(deposited));
        let network_name = crate::network_name_from_chain_id(client.deployment.chain_id);

        // Get wallet's actual ETH balance
        let available = client.provider().get_balance(addr).await?;
        let available_formatted = crate::format_amount(&format_ether(available));

        println!(
            "\n{} [{}]",
            "Balance (ETH) on Boundless Market".bold(),
            network_name.blue().bold()
        );
        println!("  Address:   {}", format!("{:#x}", addr).dimmed());
        println!("  Deposited: {} {}", deposited_formatted.green().bold(), "ETH".green());
        println!("  Available: {} {}", available_formatted.cyan().bold(), "ETH".cyan());
        Ok(())
    }
}
