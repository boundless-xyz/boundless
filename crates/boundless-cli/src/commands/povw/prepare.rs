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

use std::{borrow::Borrow, collections::HashSet, path::PathBuf, str::FromStr};

use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId, WorkLog};
use risc0_zkvm::{default_prover, ProverOpts};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::ProverConfig;

use super::{State, WorkReceipt};

/// Compress a directory of work receipts into a work log update.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwPrepare {
    /// Create a new work log with the given work log identifier.
    ///
    /// The work log identifier is a 160-bit public key hash (i.e. an Ethereum address) which is
    /// used to identify the work log. A work log is a collection of work claims, including their
    /// value and nonces. A single work log can only include a nonce (and so a receipt) once.
    ///
    /// A prover may have one or more work logs, and may set the work log ID equal to their onchain
    /// prover address, or to a new address just used as the work log ID.
    /// If this not set, then the state file must exist.
    #[arg(short, long = "new")]
    new_log_id: Option<PovwLogId>,

    /// Path of the work log state file to load the work log and store the prepared update.
    #[arg(short, long, env = "POVW_STATE_PATH")]
    state: PathBuf,

    /// Work receipt files to add to the work log.
    #[arg(id = "work_receipts", group = "source")]
    work_receipts_files: Vec<PathBuf>,

    /// Pull work receipts from your Bento cluster.
    ///
    /// If specified, this command will connect to your Bento cluster, and list all work receipts.
    /// It will then download those that are not already in the work log and add them to the work
    /// log.
    #[arg(long, group = "source")]
    from_bento: bool,

    /// Use a specific URL for fetching receipts from Bento, which may be different from the one
    /// used for proving. If not specified, the value of --bento-api-url will be used.
    #[arg(long, requires = "from_bento")]
    from_bento_url: Option<Url>,

    /// If set and there is an error loading a receipt, process all receipts that were loaded correctly.
    #[arg(long)]
    allow_partial_update: bool,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl PovwPrepare {
    /// Run the [PovwPrepare] command.
    pub async fn run(&self) -> Result<()> {
        // Load the existing state, if provided.
        let mut state = if let Some(log_id) = self.new_log_id {
            if self.state.exists() {
                bail!("File already exists at the state path; refusing to overwrite");
            }
            tracing::info!("Initializing a new work log with ID {log_id:x}");
            State::new(log_id)
        } else {
            let state = State::load(&self.state).await.context("Failed to load state file")?;
            tracing::info!("Loaded work log state from {}", self.state.display(),);
            tracing::debug!(commit = %state.work_log.commit(), "Loaded work log commit");
            tracing::info!("Preparing work log update for log ID: {:x}", state.log_id);
            state
        };

        let work_receipt_results = if self.from_bento {
            // Load the work receipts from Bento.
            let bento_url = match self.from_bento_url.clone() {
                Some(bento_url) => bento_url,
                None => Url::parse(&self.prover_config.bento_api_url)
                    .context("Failed to parse Bento API URL")?,
            };
            fetch_work_receipts(state.log_id, &state.work_log, &bento_url)
                .await
                .context("Failed to fetch work receipts from Bento")?
        } else {
            // Load work receipt files, filtering out receipt files that we cannot add to the log.
            load_work_receipts(state.log_id, &state.work_log, &self.work_receipts_files).await
        };

        // Check to see if there were errors in loading the receipts and decide whether to continue.
        let mut warning = false;
        let mut work_receipts = Vec::new();
        for result in work_receipt_results {
            match result {
                Err(err) => {
                    tracing::warn!("{:?}", err.context("Skipping receipt"));
                    warning = true;
                }
                Ok(receipt) => work_receipts.push(receipt),
            }
        }
        if warning && !self.allow_partial_update {
            bail!("Encountered errors in loading receipts");
        }

        if work_receipts.is_empty() {
            tracing::info!("No work receipts to process");
            // Save the state file anyway, to create an empty one if it does not yet exist.
            state.save(&self.state).context("Failed to save state")?;
            return Ok(());
        }
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        // Set up the work log update prover
        self.prover_config.configure_proving_backend_with_health_check().await?;
        let prover_builder = WorkLogUpdateProver::builder()
            .prover(default_prover())
            .prover_opts(ProverOpts::succinct())
            .log_id(state.log_id)
            .log_builder_program(risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ELF)
            .context("Failed to build WorkLogUpdateProver")?;

        // Add the initial state to the prover.
        let prover_builder = if !state.work_log.is_empty() {
            let Some(receipt) = state.log_builder_receipts.last() else {
                bail!("State contains non-empty work log and no log builder receipts")
            };
            prover_builder
                .work_log(state.work_log.clone(), receipt.clone())
                .context("Failed to build prover with given state")?
        } else {
            prover_builder
        };

        let mut prover = prover_builder.build().context("Failed to build WorkLogUpdateProver")?;

        // Prove the work log update
        // NOTE: We use tokio block_in_place here to mitigate two issues. One is that when using
        // the Bonsai version of the default prover, tokio may panic with an error about the
        // runtime being dropped. And spawn_blocking cannot be used because VerifierContext,
        // default_prover(), and ExecutorEnv do not implement Send.
        let prove_info = tokio::task::block_in_place(|| {
            prover.prove_update(work_receipts).context("Failed to prove work log update")
        })?;

        // Update and save the output state.
        state
            .update_work_log(prover.work_log, prove_info.receipt)
            .context("Failed to update state")?
            .save(&self.state)
            .context("Failed to save state")?;

        tracing::info!("Updated work log and prepared an update proof");
        Ok(())
    }
}

/// Load work receipts from the specified directory
async fn load_work_receipts(
    log_id: PovwLogId,
    work_log: &WorkLog,
    files: &[PathBuf],
) -> Vec<anyhow::Result<WorkReceipt>> {
    let mut work_receipts = Vec::new();
    for path in files {
        // Load the receipts, propogating an error on failure or if the receipt isn't for this log.
        let work_receipt = load_work_receipt_file(path)
            .await
            .with_context(|| format!("Failed to load receipt from {}", path.display()))
            .and_then(|receipt| {
                check_work_receipt(log_id, work_log, receipt)
                    .with_context(|| format!("Receipt from path {}", path.display()))
            });

        if work_receipt.is_ok() {
            tracing::debug!("Loaded receipt from: {}", path.display());
        }

        work_receipts.push(work_receipt);
    }
    work_receipts
}

/// Load a single receipt file
async fn load_work_receipt_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<WorkReceipt> {
    let path = path.as_ref();
    if !path.is_file() {
        bail!("Work receipt path is not a file: {}", path.display())
    }

    let data = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    // Deserialize as WorkReceipt
    // TODO: Provide a common library implementation of encoding that can be used by Bento,
    // r0vm, and this crate. bincode works, but is fragile to any changes so e.g. adding a
    // version number would help.
    let receipt: WorkReceipt = bincode::deserialize(&data)
        .with_context(|| format!("Failed to deserialize receipt from: {}", path.display()))?;

    Ok(receipt)
}

fn check_work_receipt<T: Borrow<WorkReceipt>>(
    log_id: PovwLogId,
    work_log: &WorkLog,
    work_receipt: T,
) -> anyhow::Result<T> {
    let work_claim = work_receipt
        .borrow()
        .claim()
        .as_value()
        .context("Loaded receipt has a pruned claim")?
        .work
        .as_value()
        .context("Loaded receipt has a pruned work claim")?
        .clone();

    // NOTE: If nonce_max does not have the same log ID as nonce_min, the exec will fail.
    ensure!(
        work_claim.nonce_min.log == log_id,
        "Receipt has a log ID that does not match the work log: receipt: {:x}, work log: {:x}",
        work_claim.nonce_min.log,
        log_id
    );

    ensure!(
        !work_log.jobs.contains_key(&work_claim.nonce_min.job),
        "Receipt has job ID that is already in the work log: {}",
        work_claim.nonce_min.job,
    );
    Ok(work_receipt)
}

// TODO: Create a common crate that Bento, test-utils and the CLI can all use.
/// Work receipt info matching Bento API format
/// Copied from bento/crates/api/src/lib.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReceiptInfo {
    pub key: String,
    /// PoVW log ID if PoVW is enabled, None otherwise
    pub povw_log_id: Option<String>,
    /// PoVW job number if PoVW is enabled, None otherwise
    pub povw_job_number: Option<String>,
}

/// Work receipt list matching Bento API format
/// Copied from bento/crates/api/src/lib.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReceiptList {
    pub receipts: Vec<WorkReceiptInfo>,
}

async fn fetch_work_receipts(
    log_id: PovwLogId,
    work_log: &WorkLog,
    bento_url: &Url,
) -> anyhow::Result<Vec<anyhow::Result<WorkReceipt>>> {
    // Call the /work-receipts endpoint on Bento.
    let list_url = bento_url.join("work-receipts")?;
    let response = reqwest::get(list_url.clone())
        .await
        .context("Failed to query Bento for work receipts")?
        .error_for_status()
        .with_context(|| format!("Failed to fetch work receipts list from {list_url}"))?;

    let receipt_list: WorkReceiptList = response
        .json()
        .await
        .with_context(|| format!("Failed to parse work receipts list from {list_url}"))?;

    // Filter the list for new receipts.
    let mut seen_log_ids = HashSet::new();
    let mut keys_to_fetch = HashSet::new();
    for info in receipt_list.receipts {
        let (info_log_id, info_job_number) = match parse_receipt_info(&info) {
            Ok(ok) => ok,
            Err(err) => {
                tracing::warn!(
                    "{:?}",
                    err.context(format!("Skipping receipt with key {}", info.key))
                );
                continue;
            }
        };

        if info_log_id != log_id {
            // Log any unknown log IDs we find, but only once.
            if !seen_log_ids.insert(info_log_id) {
                tracing::warn!("Skipping receipts with log ID {info_log_id:x} in Bento storage");
            }
            tracing::debug!("Skipping receipt with key {}; log ID does not match", info.key);
            continue;
        }

        if work_log.jobs.contains_key(&info_job_number) {
            tracing::debug!(
                "Skipping receipt with key {}; work log contains job number {}",
                info.key,
                info_job_number
            );
            continue;
        }
        if !keys_to_fetch.insert(info.key.clone()) {
            tracing::warn!(
                "Duplicate responses for work receipt key {} in work log list",
                info.key
            );
        }
    }

    // Fetch the new receipts.
    let mut work_receipts = Vec::new();
    for key in keys_to_fetch {
        // NOTE: We return the result so that the caller can decide whether to skip or bail.
        let work_receipt =
            fetch_work_receipt(bento_url, &key).await.context("Failed to fetch work receipt");

        if work_receipt.is_ok() {
            tracing::debug!("Loaded receipt with key: {key}");
        }

        work_receipts.push(work_receipt);
    }
    Ok(work_receipts)
}

// Parse the log ID and job ID from the WorkReceiptInfo, or return an error if they cannot be
// parsed are are not available.
fn parse_receipt_info(info: &WorkReceiptInfo) -> anyhow::Result<(PovwLogId, u64)> {
    let log_id =
        PovwLogId::from_str(info.povw_log_id.as_ref().context("Work receipt info has no log ID")?)
            .context("Failed to parse work receipt info log ID")?;
    let job_number = u64::from_str(
        info.povw_job_number.as_ref().context("Work receipt info has no job number")?,
    )
    .context("Failed to parse work receipt info job number")?;
    Ok((log_id, job_number))
}

async fn fetch_work_receipt(bento_url: &Url, key: &str) -> anyhow::Result<WorkReceipt> {
    let receipt_url = bento_url
        .join("work-receipts/")?
        .join(key)
        .with_context(|| format!("Failed to build URL to fetch work receipt with key {key}"))?;
    let response = reqwest::get(receipt_url.clone())
        .await
        .with_context(|| format!("Failed to fetch work receipt with key {key}"))?
        .error_for_status()
        .with_context(|| format!("Failed to fetch work receipt with key {key}"))?;

    let receipt_bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read work receipt bytes for key {key}"))?;
    bincode::deserialize(&receipt_bytes)
        .with_context(|| format!("Failed to deserialize receipt with key {key}"))
}
