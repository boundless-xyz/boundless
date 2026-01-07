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

use std::{fs::File, io::BufReader, path::PathBuf, time::SystemTime};

use alloy::primitives::{B256, U256};
use anyhow::{bail, Context, Result};
use boundless_market::{
    contracts::{FulfillmentData, Predicate, ProofRequest},
    storage::fetch_url,
};
use clap::Args;
use risc0_zkvm::{compute_image_id, default_executor, sha::Digest, ExecutorEnv, SessionInfo};

use crate::config::{GlobalConfig, ProverConfig};
use crate::config_ext::ProverConfigExt;
use crate::display::{network_name_from_chain_id, DisplayManager};

/// Execute a proof request using the RISC Zero zkVM executor
#[derive(Args, Clone, Debug)]
pub struct ProverExecute {
    /// Path to a YAML file containing the request.
    #[arg(long, conflicts_with_all = ["request_id", "tx_hash"])]
    pub request_path: Option<PathBuf>,

    /// The proof request identifier.
    #[arg(long, conflicts_with = "request_path")]
    pub request_id: Option<U256>,

    /// The request digest
    #[arg(long)]
    pub request_digest: Option<B256>,

    /// The tx hash of the request submission.
    #[arg(long, conflicts_with = "request_path", requires = "request_id")]
    pub tx_hash: Option<B256>,

    /// Prover configuration options
    #[clap(flatten, next_help_heading = "Prover")]
    pub prover_config: ProverConfig,
}

impl ProverExecute {
    /// Run the execute command
    pub async fn run(&self, global_config: &GlobalConfig) -> Result<()> {
        let prover_config = self.prover_config.clone().load_and_validate()?;
        let client = prover_config.client_builder(global_config.tx_timeout)?.build().await?;
        let network_name = network_name_from_chain_id(client.deployment.market_chain_id);
        let display = DisplayManager::with_network(network_name);

        display.header("Executing Proof Request");

        let request: ProofRequest = if let Some(file_path) = &self.request_path {
            display.item_colored("Source", format!("file: {}", file_path.display()), "cyan");
            tracing::debug!("Loading request from file: {:?}", file_path);
            let file = File::open(file_path).context("failed to open request file")?;
            let reader = BufReader::new(file);
            serde_yaml::from_reader(reader).context("failed to parse request from YAML")?
        } else if let Some(request_id) = self.request_id {
            display.item_colored("Request ID", format!("{:#x}", request_id), "cyan");
            display.status("Status", "Fetching request from blockchain", "yellow");
            tracing::debug!("Loading request from blockchain: 0x{:x}", request_id);
            let (req, _signature) =
                client.fetch_proof_request(request_id, self.tx_hash, self.request_digest).await?;
            req
        } else {
            bail!("execute requires either a request file path or request ID")
        };

        display.status("Status", "Starting execution", "yellow");
        let (image_id, session_info) = execute(&request, &display).await?;
        let journal = session_info.journal.bytes;
        let predicate = Predicate::try_from(request.requirements.predicate.clone())?;

        let fulfillment_data =
            FulfillmentData::from_image_id_and_journal(image_id, journal.clone());

        display.status("Status", "Evaluating predicate", "yellow");
        if predicate.eval(&fulfillment_data).is_none() {
            display.error(&format!("Predicate evaluation failed for request {:#x}", request.id));
            bail!("Predicate evaluation failed");
        }

        display.success(&format!("Successfully executed request {:#x}", request.id));
        tracing::debug!("Journal: {:?}", journal);
        Ok(())
    }
}

/// Execute a proof request using the RISC Zero zkVM executor and returns the image id and session info
async fn execute(
    request: &ProofRequest,
    display: &DisplayManager,
) -> Result<(Digest, SessionInfo)> {
    display.status("Status", "Fetching program", "yellow");
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
            display.status("Status", "Fetching input", "yellow");
            let input_data = fetch_url(input_url).await?;
            boundless_market::input::GuestEnv::decode(&input_data)?.stdin
        }
        _ => anyhow::bail!("Unsupported input type"),
    };

    display.status("Status", "Executing zkVM", "yellow");
    let start = SystemTime::now();
    let env = ExecutorEnv::builder().write_slice(&input).build()?;
    let session_info = default_executor().execute(env, &program)?;
    let elapsed = SystemTime::now().duration_since(start)?.as_secs_f64();

    let total_cycles: usize = session_info.segments.iter().map(|s| s.cycles as usize).sum();

    display.item_colored("Status", "Execution completed", "green");
    display.item_colored("Time", format!("{:.2}s", elapsed), "cyan");
    display.item_colored("Cycles", total_cycles, "cyan");

    tracing::debug!("Journal: {:?}", hex::encode(&session_info.journal.bytes));

    Ok((image_id, session_info))
}
