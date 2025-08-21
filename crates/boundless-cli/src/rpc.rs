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

use reqwest::Client;
use url::Url;

#[derive(Debug, Clone)]
/// A provider for interacting with an Ethereum JSON-RPC endpoint.
pub struct RpcProvider {
    /// The URL of the RPC endpoint.
    url: Url,
}

impl RpcProvider {
    /// Creates a new RPC provider with the given URL.
    pub fn new(url: Url) -> Self {
        Self { url }
    }

    pub async fn get_chain_id(&self) -> Result<u64, anyhow::Error> {
        let client = Client::new();
        let response = client
            .post(self.url.clone())
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "eth_chainId",
                "params": [],
                "id": 1,
            }))
            .send()
            .await?
            .json::<serde_json::Value>()
            .await?;

        // The response is a JSON object with a "result" field containing the chain ID
        // as a hexadecimal string (e.g., "0x1").
        let chain_id_hex = response["result"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid response format"))?;

        u64::from_str_radix(chain_id_hex.trim_start_matches("0x"), 16)
            .map_err(|e| anyhow::anyhow!("Failed to parse chain ID: {}", e))
    }
}
