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

use alloy::primitives::U256;
use anyhow::Result;
use clap::Args;
use colored::Colorize;

use crate::config::{GlobalConfig, RequestorConfig};

/// Get the status of a given request
#[derive(Args, Clone, Debug)]
pub struct RequestorStatus {
    /// The proof request identifier
    pub request_id: U256,

    /// The time at which the request expires, in seconds since the UNIX epoch
    pub expires_at: Option<u64>,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorStatus {
    /// Run the status command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_from_files()?;

        let client = requestor_config.client_builder(global_config.tx_timeout)?.build().await?;
        let status = client.boundless_market.get_status(self.request_id, self.expires_at).await?;

        let network_name = crate::network_name_from_chain_id(client.deployment.chain_id);

        println!("\n{} [{}]", "Request Status".bold(), network_name.blue().bold());
        println!("  Request ID: {}", format!("{:#x}", self.request_id).dimmed());

        let (status_text, status_color): (String, fn(&str) -> colored::ColoredString) = match status
        {
            boundless_market::contracts::RequestStatus::Fulfilled => {
                ("✓ Fulfilled".to_string(), |s| s.green().bold())
            }
            boundless_market::contracts::RequestStatus::Locked => {
                ("⏳ Locked".to_string(), |s| s.yellow().bold())
            }
            boundless_market::contracts::RequestStatus::Expired => {
                ("✗ Expired".to_string(), |s| s.red().bold())
            }
            boundless_market::contracts::RequestStatus::Unknown => {
                ("? Unknown".to_string(), |s| s.dimmed())
            }
        };

        println!("  Status:     {}", status_color(&status_text));
        Ok(())
    }
}
