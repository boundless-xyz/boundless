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

use crate::{OrderFulfilled, OrderFulfiller};
use alloy::primitives::{B256, U256};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::boundless_market::{FulfillmentTx, UnlockedRequest};
use clap::Args;

use crate::config::{GlobalConfig, ProverConfig};
use crate::config_ext::ProverConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Fulfill one or more proof requests
#[derive(Args, Clone, Debug)]
pub struct ProverFulfill {
    /// The proof requests identifiers (comma-separated list of hex values)
    #[arg(long, value_delimiter = ',')]
    pub request_ids: Vec<U256>,

    /// The request digests (comma-separated list of hex values).
    #[arg(long, value_delimiter = ',')]
    pub request_digests: Option<Vec<B256>>,

    /// The tx hash of the requests submissions (comma-separated list of hex values).
    #[arg(long, value_delimiter = ',')]
    pub tx_hashes: Option<Vec<B256>>,

    /// Withdraw the funds after fulfilling the requests
    #[arg(long, default_value = "false")]
    pub withdraw: bool,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}

impl ProverFulfill {
    /// Run the fulfill command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let prover_config = self.prover_config.clone().load_and_validate()?;
        prover_config.require_private_key_with_help()?;

        let client = prover_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client with signer")?;

        if self.request_digests.is_some()
            && self.request_ids.len() != self.request_digests.as_ref().unwrap().len()
        {
            bail!("request_ids and request_digests must have the same length");
        }
        if self.tx_hashes.is_some()
            && self.request_ids.len() != self.tx_hashes.as_ref().unwrap().len()
        {
            bail!("request_ids and tx_hashes must have the same length");
        }

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);
        let request_ids_string =
            self.request_ids.iter().map(|id| format!("{:#x}", id)).collect::<Vec<_>>().join(", ");

        display.header("Fulfilling Proof Requests");
        display.item_colored("Request IDs", &request_ids_string, "cyan");
        display.status("Status", "Initializing prover and fetching images", "yellow");

        // Initialize fulfiller with prover setup and image uploads
        let fulfiller = OrderFulfiller::initialize_from_config(&prover_config, &client).await?;

        let fetch_order_jobs = self.request_ids.iter().enumerate().map(|(i, request_id)| {
            let client = client.clone();
            let boundless_market = client.boundless_market.clone();
            async move {
                let (req, sig) = client
                    .fetch_proof_request(
                        *request_id,
                        self.tx_hashes.as_ref().map(|tx_hashes| tx_hashes[i]),
                        self.request_digests.as_ref().map(|request_digests| request_digests[i]),
                    )
                    .await?;
                tracing::debug!("Fetched order details: {req:?}");

                if !req.is_smart_contract_signed() {
                    req.verify_signature(
                        &sig,
                        client.deployment.boundless_market_address,
                        boundless_market.get_chain_id().await?,
                    )?;
                } else {
                    // TODO: Provide a way to check the EIP1271 auth.
                    tracing::debug!(
                        "Skipping authorization check on smart contract signed request 0x{:x}",
                        U256::from(req.id)
                    );
                }
                let is_locked = boundless_market.is_locked(*request_id).await?;
                Ok::<_, anyhow::Error>((req, sig, is_locked))
            }
        });

        display.status("Status", "Fetching request details", "yellow");
        let results = futures::future::join_all(fetch_order_jobs).await;
        let mut orders = Vec::new();
        let mut unlocked_requests = Vec::new();

        for result in results {
            let (req, sig, is_locked) = result?;
            if !is_locked {
                unlocked_requests.push(UnlockedRequest::new(req.clone(), sig.clone()));
            }
            orders.push((req, sig));
        }

        display.status("Status", "Generating proofs", "yellow");
        let (fills, root_receipt, assessor_receipt) = fulfiller.fulfill(&orders).await?;
        let order_fulfilled = OrderFulfilled::new(fills, root_receipt, assessor_receipt)?;
        let boundless_market = client.boundless_market.clone();

        let fulfillment_tx =
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    client.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                )
                .with_unlocked_requests(unlocked_requests)
                .with_withdraw(self.withdraw);

        display.status("Status", "Submitting fulfillment", "yellow");
        match boundless_market.fulfill(fulfillment_tx).await {
            Ok(_) => {
                display
                    .success(&format!("Successfully fulfilled requests: {}", request_ids_string));
                if self.withdraw {
                    display.item_colored("Funds", "withdrawn", "green");
                }
                Ok(())
            }
            Err(e) => {
                display.error(&format!("Failed to fulfill requests: {}", request_ids_string));
                bail!("Failed to fulfill request: {}", e)
            }
        }
    }
}
