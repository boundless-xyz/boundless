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

use crate::display::network_name_from_chain_id;
use std::{
    fs::File,
    io::BufReader,
    path::PathBuf,
    time::{Duration, SystemTime},
};

use alloy::primitives::U256;
use anyhow::{ensure, Context, Result};
use boundless_market::{
    contracts::{FulfillmentData, Offer, Predicate, ProofRequest},
    storage::{fetch_url, StorageProviderConfig},
};
use clap::Args;
use risc0_zkvm::sha::{Digest, Digestible};
use risc0_zkvm::{compute_image_id, default_executor, ExecutorEnv, ReceiptClaim, SessionInfo};

use crate::{
    config::{GlobalConfig, RequestorConfig},
    config_ext::RequestorConfigExt,
    display::{convert_timestamp, DisplayManager},
};

/// Submit a fully specified proof request
#[derive(Args, Clone, Debug)]
pub struct RequestorSubmit {
    /// Path to a YAML file containing the request
    pub yaml_request: PathBuf,

    /// Wait until the request is fulfilled
    #[clap(short, long, default_value = "false")]
    pub wait: bool,

    /// Submit the request offchain via the provided order stream service url
    #[clap(short, long)]
    pub offchain: bool,

    /// Skip preflight check (not recommended)
    #[clap(long, default_value = "false")]
    pub no_preflight: bool,

    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    pub storage_config: Box<StorageProviderConfig>,

    /// Requestor configuration (RPC URL, private key, deployment)
    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

impl RequestorSubmit {
    /// Run the submit command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;
        requestor_config.require_private_key_with_help()?;

        let client = requestor_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .with_storage_provider_config(&self.storage_config)?
            .build()
            .await
            .context("Failed to build Boundless Client")?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        display.header("Submitting Proof Request from YAML");

        // Read the YAML request file
        let file = File::open(&self.yaml_request)
            .context(format!("Failed to open request file at {:?}", self.yaml_request))?;
        let reader = BufReader::new(file);
        let mut request: ProofRequest =
            serde_yaml::from_reader(reader).context("Failed to parse request from YAML")?;

        // Fill in some of the request parameters
        // If set to 0, override the offer bidding_start field with the current timestamp + 30s
        if request.offer.rampUpStart == 0 {
            // Adding a delay to bidding start lets provers see and evaluate the request
            // before the price starts to ramp up
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH)?.as_secs();
            request.offer = Offer { rampUpStart: now + 30, ..request.offer };
        }
        if request.id == U256::ZERO {
            request.id = client.boundless_market.request_id_from_rand().await?;
            display.item("Assigned Request ID", format!("{:#x}", request.id));
        };

        // Run preflight check if enabled
        if !self.no_preflight {
            display.info("Running request preflight check");
            let (image_id, session_info) = execute(&request).await?;
            let journal = &session_info.journal.bytes;

            // Verify image ID
            if let Some(claim) = &session_info.receipt_claim {
                use risc0_zkvm::sha::Digestible;
                ensure!(
                    claim.pre.digest() == image_id,
                    "Image ID mismatch: requirements ({}) do not match the given program ({})",
                    image_id,
                    claim.pre.digest(),
                );
            } else {
                tracing::debug!("Cannot check image ID; session info doesn't have receipt claim");
            }
            let predicate = Predicate::try_from(request.requirements.predicate.clone())?;

            let expected_claim_digest = ReceiptClaim::ok(image_id, journal.clone()).digest();
            ensure!(
                predicate.eval(&FulfillmentData::from_image_id_and_journal(image_id, journal.clone())).is_some(),
                "Preflight failed: Predicate evaluation failed. Journal: {}, Predicate type: {:?}, Predicate data: {}, Expected claim digest: {}, Expected journal digest: {}",
                hex::encode(journal),
                request.requirements.predicate.predicateType,
                hex::encode(&request.requirements.predicate.data),
                hex::encode(expected_claim_digest),
                hex::encode(session_info.journal.digest())
            );

            display.success("Preflight check passed");
        } else {
            display.warning("Skipping preflight check");
        }

        // Submit the request
        let (request_id, expires_at) = if self.offchain {
            display.item("Submission", "Offchain via Order Stream");
            client.submit_request_offchain(&request).await?
        } else {
            display.item("Submission", "Onchain to Boundless Market");
            client.submit_request_onchain(&request).await?
        };

        display.item("Request ID", format!("{:#x}", request_id));
        display.item("Bidding Starts", convert_timestamp(request.offer.rampUpStart));
        display.success("Request submitted successfully");

        // Wait for fulfillment if requested
        if self.wait {
            display.info("Waiting for request fulfillment...");
            let fulfillment = client
                .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
                .await?;

            display.success("Request fulfilled!");
            println!(
                "\nFulfillment Data:\n{}",
                serde_json::to_string_pretty(&fulfillment.data()?)?
            );
            println!("\nSeal:\n{}", serde_json::to_string_pretty(&fulfillment.seal)?);
        }

        Ok(())
    }
}

/// Execute a proof request using the RISC Zero zkVM executor and returns the image id and session info
async fn execute(request: &ProofRequest) -> Result<(Digest, SessionInfo)> {
    tracing::info!("Fetching program from {}", request.imageUrl);
    let program = fetch_url(&request.imageUrl).await?;
    let image_id = compute_image_id(&program)?;

    tracing::debug!("Program image id: {}", image_id);

    let input = match request.input.inputType {
        boundless_market::contracts::RequestInputType::Inline => {
            boundless_market::input::GuestEnv::decode(&request.input.data)?.stdin
        }
        boundless_market::contracts::RequestInputType::Url => {
            let input_url =
                std::str::from_utf8(&request.input.data).context("Input URL is not valid UTF-8")?;
            tracing::info!("Fetching input from {}", input_url);
            let input_data = fetch_url(input_url).await?;
            boundless_market::input::GuestEnv::decode(&input_data)?.stdin
        }
        _ => anyhow::bail!("Unsupported input type"),
    };

    tracing::info!("Starting execution");
    let start = SystemTime::now();
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    let session_info = default_executor().execute(env, &program)?;
    let elapsed = SystemTime::now().duration_since(start)?.as_secs_f64();

    tracing::info!("Execution completed in {:.2}s", elapsed);
    tracing::debug!("Journal: {:?}", hex::encode(&session_info.journal.bytes));
    tracing::info!(
        "Total cycles: {}",
        session_info.segments.iter().map(|s| s.cycles as usize).sum::<usize>()
    );

    Ok((image_id, session_info))
}
