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

use crate::config::GlobalConfig;

/// Get the journal and seal for a given request
#[derive(Args, Clone, Debug)]
pub struct RequestorGetProof {
    /// The proof request identifier
    pub request_id: U256,
}

impl RequestorGetProof {
    /// Run the get-proof command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let client = global_config.build_client().await?;
        tracing::info!("Fetching proof for request 0x{:x}", self.request_id);
        let fulfillment = client.boundless_market.get_request_fulfillment(self.request_id).await?;
        tracing::info!("Successfully retrieved proof for request 0x{:x}", self.request_id);
        tracing::info!(
            "Fulfillment Data: {} - Seal: {}",
            serde_json::to_string_pretty(&fulfillment.data()?)?,
            serde_json::to_string_pretty(&fulfillment.seal)?
        );
        Ok(())
    }
}