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
use anyhow::{bail, Result};
use clap::Args;

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
        tracing::info!("Checking balance for address {}", addr);
        let balance = client.boundless_market.balance_of(addr).await?;
        tracing::info!("Balance for address {}: {} ETH", addr, format_ether(balance));
        Ok(())
    }
}