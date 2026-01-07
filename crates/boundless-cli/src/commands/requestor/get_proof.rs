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

use alloy::primitives::U256;
use anyhow::{Context, Result};
use clap::Args;

use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::RequestorConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Get the journal and seal for a given request
#[derive(Args, Clone, Debug)]
pub struct RequestorGetProof {
    /// The proof request identifier
    pub request_id: U256,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorGetProof {
    /// Run the get-proof command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;

        let client = requestor_config.client_builder(global_config.tx_timeout)?.build().await?;
        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);

        let display = DisplayManager::with_network(network_name);

        display.header("Fetching Proof");
        display.item("Request ID", format!("{:#x}", self.request_id));

        let fulfillment = client
            .boundless_market
            .get_request_fulfillment(self.request_id)
            .await
            .context("Failed to retrieve proof")?;

        let fulfillment_data = fulfillment.data()?;
        let seal = &fulfillment.seal;

        display.success("Successfully retrieved proof");
        println!("\nFulfillment Data:\n{}", serde_json::to_string_pretty(&fulfillment_data)?);
        println!("\nSeal:\n{}", serde_json::to_string_pretty(seal)?);

        Ok(())
    }
}
