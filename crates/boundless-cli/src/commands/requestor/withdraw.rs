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

use alloy::primitives::{utils::{format_ether, parse_ether}, U256};
use anyhow::Result;
use clap::Args;

use crate::config::GlobalConfig;

/// Command to withdraw funds from the market
#[derive(Args, Clone, Debug)]
pub struct RequestorWithdraw {
    /// Amount in ether to withdraw
    #[clap(value_parser = parse_ether)]
    pub amount: U256,
}

impl RequestorWithdraw {
    /// Run the withdraw command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let client = global_config.build_client_with_signer().await?;
        tracing::info!("Withdrawing {} ETH from the market", format_ether(self.amount));
        client.boundless_market.withdraw(self.amount).await?;
        tracing::info!("Successfully withdrew {} ETH from the market", format_ether(self.amount));
        Ok(())
    }
}