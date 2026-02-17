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
use std::{fs::File, io::BufReader, path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use boundless_market::{
    contracts::ProofRequest,
    request_builder::{OfferParams, RequirementParams},
    StorageUploaderConfig,
};
use clap::Args;
use url::Url;

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

    /// Skip preflight and pricing checks (not recommended)
    #[clap(long, default_value = "false")]
    pub no_preflight: bool,

    /// Configuration for the uploader used for programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Uploader")]
    pub storage_config: Box<StorageUploaderConfig>,

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
            .with_uploader_config(&self.storage_config)
            .await?
            .with_skip_preflight(self.no_preflight)
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
        let yaml_request: ProofRequest =
            serde_yaml::from_reader(reader).context("Failed to parse request from YAML")?;

        // Build request using the request builder pattern
        // This runs preflight execution and pricing checks automatically
        display.info("Building request (running preflight and pricing checks)");

        let program_url: Url = yaml_request.imageUrl.parse().context("Invalid imageUrl")?;
        let request_builder = client
            .new_request()
            .with_program_url(program_url)?
            .with_request_input(yaml_request.input.clone())
            .with_offer(OfferParams::from_offer(yaml_request.offer.clone()))
            .with_requirements(RequirementParams::try_from(yaml_request.requirements.clone())?);

        let request =
            client.build_request(request_builder).await.context("Failed to build proof request")?;

        display.success("Request built successfully");

        tracing::debug!("Request details: {}", serde_yaml::to_string(&request)?);

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
