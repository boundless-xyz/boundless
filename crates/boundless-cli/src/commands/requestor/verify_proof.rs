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

use alloy::primitives::{B256, U256};
use anyhow::{anyhow, bail, ensure, Context, Result};
use boundless_market::contracts::{FulfillmentData, Predicate};
use clap::Args;
use risc0_ethereum_contracts::IRiscZeroVerifier;
use risc0_zkvm::sha::Digest;

use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::RequestorConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Verify the proof of the given request against the SetVerifier contract
#[derive(Args, Clone, Debug)]
pub struct RequestorVerifyProof {
    /// The proof request identifier
    pub request_id: U256,

    /// The image id of the original request
    pub image_id: B256,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorVerifyProof {
    /// Run the verify-proof command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;

        let client = requestor_config
            .client_builder(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client")?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        display.header("Verifying Proof");
        display.item("Request ID", format!("{:#x}", self.request_id));

        // Get verifier contract address
        let verifier_address = client.deployment.verifier_router_address
            .context("no address provided for the verifier router; specify a verifier address with --verifier-address")?;
        let verifier = IRiscZeroVerifier::new(verifier_address, client.provider());

        // Fetch fulfillment data
        let fulfillment = client.boundless_market.get_request_fulfillment(self.request_id).await?;
        let fulfillment_data = fulfillment.data()?;
        let seal = fulfillment.seal;

        // Fetch original request
        let (req, _) = client.boundless_market.get_submitted_request(self.request_id, None).await?;
        let predicate = Predicate::try_from(req.requirements.predicate)?;

        // Verify the proof based on fulfillment type
        match (&predicate, fulfillment_data.clone()) {
            (_, FulfillmentData::ImageIdAndJournal(image_id_from_data, journal)) => {
                // Verify image ID matches
                ensure!(
                    image_id_from_data == Digest::from(<[u8; 32]>::from(self.image_id)),
                    "Image ID mismatch: expected {:?}, got {:?}",
                    image_id_from_data,
                    self.image_id
                );

                // Compute journal digest
                let journal_digest = {
                    use risc0_zkvm::sha::Digestible;
                    let digest = journal.digest();
                    B256::from(<[u8; 32]>::from(digest))
                };

                // Verify the proof on-chain
                verifier
                    .verify(seal, self.image_id, journal_digest)
                    .call()
                    .await
                    .map_err(|_| anyhow!("Verification failed"))?;
            }
            (_, _) => {
                bail!(
                    "Verification failed due to invalid predicate {:?} or fulfillment data {:?}",
                    predicate,
                    fulfillment_data
                )
            }
        }

        display.success(&format!("Successfully verified proof for request {:#x}", self.request_id));
        Ok(())
    }
}
