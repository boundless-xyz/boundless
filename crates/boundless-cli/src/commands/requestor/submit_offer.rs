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

use alloy::primitives::{Address, U256};
use anyhow::{anyhow, bail, Context, Result};
use boundless_market::{
    input::GuestEnvBuilder,
    request_builder::{OfferParams as OfferParamsStruct, RequirementParams},
    selector::{ProofType, SelectorExt},
    storage::StorageProviderConfig,
};
use clap::Args;
use serde_json;
use std::borrow::Cow;
use std::path::PathBuf;
use std::time::Duration;
use url::Url;

use crate::config::{GlobalConfig, RequestorConfig};
use crate::config_ext::RequestorConfigExt;
use crate::display::{convert_timestamp, network_name_from_chain_id, DisplayManager};

/// Submit a proof request constructed with the given offer, input, and image
#[derive(Args, Clone, Debug)]
#[allow(missing_docs)]
pub struct RequestorSubmitOffer {
    /// Optional identifier for the request
    pub id: Option<u32>,

    #[clap(flatten)]
    pub program: SubmitOfferProgram,

    /// Wait until the request is fulfilled
    #[clap(short, long, default_value = "false")]
    pub wait: bool,

    /// Submit the request offchain via the provided order stream service url
    #[clap(short, long)]
    pub offchain: bool,

    /// Use risc0_zkvm::serde to encode the input as a `Vec<u8>`
    #[clap(long)]
    pub encode_input: bool,

    #[clap(flatten)]
    pub input: SubmitOfferInput,

    #[clap(flatten)]
    pub requirements: SubmitOfferRequirements,

    #[clap(flatten, next_help_heading = "Offer")]
    /// Offer parameters for the request
    pub offer_params: OfferParamsStruct,

    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    pub storage_config: StorageProviderConfig,

    #[clap(flatten)]
    pub requestor_config: RequestorConfig,
}

#[derive(Args, Clone, Debug)]
#[group(required = true, multiple = false)]
#[allow(missing_docs)]
pub struct SubmitOfferInput {
    /// Input for the guest, given as a string.
    #[clap(long)]
    pub input: Option<String>,
    /// Input for the guest, given as a path to a file.
    #[clap(long)]
    pub input_file: Option<PathBuf>,
}

#[derive(Args, Clone, Debug)]
#[group(required = true, multiple = false)]
#[allow(missing_docs)]
pub struct SubmitOfferProgram {
    /// Program binary to use as the guest image, given as a path.
    #[clap(short = 'p', long = "program")]
    pub path: Option<PathBuf>,
    /// Program binary to use as a guest image, given as a public URL.
    #[clap(long = "program-url")]
    pub url: Option<Url>,
}

#[derive(Args, Clone, Debug)]
#[allow(missing_docs)]
pub struct SubmitOfferRequirements {
    /// Address of the callback to use in the requirements.
    #[clap(long, requires = "callback_gas_limit")]
    pub callback_address: Option<Address>,
    /// Gas limit of the callback to use in the requirements.
    #[clap(long, requires = "callback_address")]
    pub callback_gas_limit: Option<U256>,
    /// Request a groth16 proof (i.e., a Groth16).
    #[clap(long, default_value = "any")]
    pub proof_type: ProofType,
}

impl RequestorSubmitOffer {
    /// Run the submit-offer command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let requestor_config = self.requestor_config.clone().load_and_validate()?;
        requestor_config.require_private_key_with_help()?;

        let client = requestor_config
            .client_builder_with_signer(global_config.tx_timeout)?
            .build()
            .await
            .context("Failed to build Boundless Client with signer")?;

        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        let request = client.new_request();

        // Resolve the program from command line arguments
        let request = match (&self.program.path, &self.program.url) {
            (Some(path), None) => {
                if client.storage_provider.is_none() {
                    bail!("A storage provider is required to upload programs.\nPlease provide a storage provider (see --help for options) or upload your program and set --program-url.")
                }
                let program: Cow<'static, [u8]> = std::fs::read(path)
                    .context(format!("Failed to read program file at {:?}", path))?
                    .into();
                request.with_program(program)
            }
            (None, Some(url)) => {
                request.with_program_url(url.clone()).map_err(|e| anyhow!("{}", e))?
            }
            _ => bail!("Exactly one of program path and program-url args must be provided"),
        };

        // Process input based on provided arguments
        let stdin: Vec<u8> = match (&self.input.input, &self.input.input_file) {
            (Some(input), None) => input.as_bytes().to_vec(),
            (None, Some(input_file)) => std::fs::read(input_file)
                .context(format!("Failed to read input file at {input_file:?}"))?,
            _ => bail!("Exactly one of input or input-file args must be provided"),
        };

        // Prepare the input environment
        let mut env_builder = GuestEnvBuilder::default();
        if self.encode_input {
            env_builder = env_builder.write(&stdin)?;
        } else {
            env_builder = env_builder.write_slice(&stdin);
        }
        let request = request.with_env(env_builder);

        // Configure callback if provided
        let mut requirements = RequirementParams::builder();
        if let Some(address) = self.requirements.callback_address {
            requirements.callback_address(address);
            if let Some(gas_limit) = self.requirements.callback_gas_limit {
                requirements.callback_gas_limit(gas_limit.to::<u64>());
            }
        }
        match self.requirements.proof_type {
            ProofType::Inclusion => {
                requirements.selector(SelectorExt::set_inclusion_latest() as u32)
            }
            ProofType::Groth16 => requirements.selector(SelectorExt::groth16_latest() as u32),
            ProofType::Blake3Groth16 => {
                requirements.selector(SelectorExt::blake3_groth16_latest() as u32)
            }
            ProofType::Any => &mut requirements,
            ty => bail!("unsupported proof type provided in proof-type flag: {:?}", ty),
        };
        let request = request.with_requirements(requirements);

        // Apply offer parameters
        let request = request.with_offer(self.offer_params.clone());

        let request =
            client.build_request(request).await.context("failed to build proof request")?;
        tracing::debug!("Request details: {}", serde_yaml::to_string(&request)?);

        display.header("Submitting Proof Request");

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
                .boundless_market
                .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
                .await?;
            let fulfillment_data = fulfillment.data()?;
            let seal = fulfillment.seal;

            display.success("Request fulfilled!");
            println!("\nFulfillment Data:\n{}", serde_json::to_string_pretty(&fulfillment_data)?);
            println!("\nSeal:\n{}", serde_json::to_string_pretty(&seal)?);
        }

        Ok(())
    }
}
