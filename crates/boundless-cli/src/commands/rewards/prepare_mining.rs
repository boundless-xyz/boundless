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

use std::{borrow::Borrow, collections::HashSet, io::Write, path::PathBuf, str::FromStr};

use alloy::providers::{Provider, ProviderBuilder};
use anyhow::{bail, ensure, Context, Result};
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use clap::Args;
use colored::Colorize;
use risc0_povw::{
    guest::Journal as LogBuilderJournal, prover::WorkLogUpdateProver, PovwLogId, WorkLog,
};
use risc0_zkvm::{default_prover, GenericReceipt, ProverOpts, ReceiptClaim, WorkClaim};
use serde::{Deserialize, Serialize};
use url::Url;

use super::State;
use crate::{
    config::{GlobalConfig, ProvingBackendConfig, RewardsConfig},
    display::DisplayManager,
};

/// Private type alias for work receipts
type WorkReceipt = GenericReceipt<WorkClaim<ReceiptClaim>>;

/// Prepare mining work log update from work receipts
#[derive(Args, Clone, Debug)]
pub struct RewardsPrepareMining {
    /// Path to the mining state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "MINING_STATE_FILE")]
    state_file: Option<PathBuf>,

    /// Work receipt files to add to the work log (if provided, files are used instead of fetching from Bento)
    #[arg(long = "work-receipt-files")]
    work_receipt_files: Vec<PathBuf>,

    /// Bento API URL to fetch work receipts from (defaults to prover config bento_api_url)
    ///
    /// This is the Bento cluster where your work receipts are stored. If not specified,
    /// the Bento API URL from your prover configuration will be used.
    #[arg(long = "work-receipt-bento-api-url")]
    work_receipt_bento_api_url: Option<Url>,

    /// If set and there is an error loading a receipt, process all receipts that were loaded correctly
    #[arg(long)]
    allow_partial_update: bool,

    /// Skip creating a backup of the state file before updating
    #[arg(long)]
    skip_backup: bool,

    #[clap(flatten)]
    rewards_config: RewardsConfig,

    #[clap(flatten, next_help_heading = "Proving Backend")]
    proving_backend: ProvingBackendConfig,
}

impl RewardsPrepareMining {
    /// Run the prepare-mining command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.mining_state_file.clone().map(PathBuf::from))
            .context("No mining state file configured.\n\nTo configure: run 'boundless rewards setup' and enable mining\nOr set MINING_STATE_FILE env var")?;

        let display = DisplayManager::new();
        display.item_colored("Mining State File", state_path.display().to_string(), "cyan");

        let mut state = State::load(&state_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to load PoVW state file: {}\n\nEnsure the file exists and is valid. Run 'boundless rewards setup' to create one.",
                    state_path.display()
                )
            })?;

        tracing::debug!(commit = %state.work_log.commit(), "Loaded work log commit");

        // Check if there's already prepared work that hasn't been submitted on-chain
        if !state.log_builder_receipts.is_empty() {
            // Only check on-chain state if RPC URL is configured
            if let Some(rpc_url) = rewards_config.reward_rpc_url.clone() {
                display.subsection("Checking On-Chain State");
                print!("  Querying mining accounting contract... ");
                std::io::stdout().flush()?;
                let provider = ProviderBuilder::new()
                    .connect(rpc_url.as_str())
                    .await
                    .with_context(|| format!("Failed to connect to {rpc_url}"))?;

                let chain_id = provider.get_chain_id().await?;
                let deployment = Deployment::from_chain_id(chain_id)
                    .context("Could not determine deployment from chain ID")?;

                let povw_accounting =
                    IPovwAccounting::new(deployment.povw_accounting_address, &provider);

                // Query on-chain commit
                let onchain_commit =
                    povw_accounting.workLogCommit(state.log_id.into()).call().await.with_context(
                        || {
                            format!(
                                "Failed to get work log commit for {:x} from {:x}",
                                state.log_id, deployment.povw_accounting_address
                            )
                        },
                    )?;

                println!("{}", "✓".green());

                // Check latest local receipt
                let latest_receipt = state.log_builder_receipts.last().unwrap();
                let latest_journal = LogBuilderJournal::decode(&latest_receipt.journal.bytes)
                    .context("Failed to decode journal from latest receipt")?;
                let latest_local_commit =
                    bytemuck::cast::<_, [u8; 32]>(latest_journal.updated_commit);

                // If they don't match, warn the user
                if latest_local_commit != *onchain_commit {
                    display.warning("WARNING");
                    display.note("You have previously prepared mining work that has not yet been submitted on-chain.");
                    display.note("We recommend only preparing once per epoch.");
                    display.note("Running prepare multiple times before submitting work on-chain will increase gas costs during submission.");
                    println!();
                } else {
                    display.status(
                        "Status",
                        "Up to date (all prepared work has been submitted)",
                        "green",
                    );
                }
            }
        }

        // Determine work receipt source
        display.subsection("Loading mining work receipts");
        let work_receipt_results = if !self.work_receipt_files.is_empty() {
            // Load from files
            display.item_colored(
                "Loading from",
                format!("{} files", self.work_receipt_files.len()),
                "cyan",
            );
            load_work_receipts(state.log_id, &state.work_log, &self.work_receipt_files).await
        } else {
            // Fetch from Bento (default behavior)
            let bento_url = match self.work_receipt_bento_api_url.clone() {
                Some(url) => url,
                None => Url::parse(&self.proving_backend.bento_api_url)
                    .context("Failed to parse Bento API URL from proving backend config")?,
            };

            display.item_colored("Fetching from Bento", bento_url.to_string(), "cyan");
            display.note("(This may take several minutes)");
            fetch_work_receipts(state.log_id, &state.work_log, &bento_url)
                .await
                .context("Failed to fetch work receipts from Bento")?
        };

        // Check to see if there were errors in loading the receipts and decide whether to continue
        let mut warning = false;
        let mut work_receipts = Vec::new();
        for result in work_receipt_results {
            match result {
                Err(err) => {
                    println!("  {} Skipping receipt: {}", "⚠".yellow(), err.to_string().dimmed());
                    warning = true;
                }
                Ok(receipt) => work_receipts.push(receipt),
            }
        }
        if warning && !self.allow_partial_update {
            bail!("Encountered errors in loading receipts");
        }

        if work_receipts.is_empty() {
            display.status("Status", "No new receipts (nothing to process)", "yellow");
            display.note(
                "All work receipts have already been added to the state file. Nothing to do.",
            );
            println!();
            return Ok(());
        }

        display.item_colored(
            "Fetched",
            format!("{} new mining work receipts", work_receipts.len()),
            "cyan",
        );

        // Set up the work log update prover
        display.subsection("Setting up prover for aggregating mining work receipts");
        self.proving_backend.configure_proving_backend_with_health_check().await?;

        let prover_builder = WorkLogUpdateProver::builder()
            .prover(default_prover())
            .prover_opts(ProverOpts::succinct())
            .log_id(state.log_id)
            .log_builder_program(risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ELF)
            .context("Failed to build WorkLogUpdateProver")?;

        // Add the initial state to the prover
        let prover_builder = if !state.work_log.is_empty() {
            let Some(receipt) = state.log_builder_receipts.last() else {
                bail!("State contains non-empty work log and no log builder receipts")
            };
            display.item_colored("Initial state", "Existing work log", "cyan");
            prover_builder
                .work_log(state.work_log.clone(), receipt.clone())
                .context("Failed to build prover with given state")?
        } else {
            display.item_colored("Initial state", "Empty work log", "cyan");
            prover_builder
        };

        let mut prover = prover_builder.build().context("Failed to build WorkLogUpdateProver")?;

        let num_receipts = work_receipts.len();
        // Prove the work log update
        display.subsection("Aggregating mining work receipts and generating proof");
        display.item_colored("Receipts", num_receipts.to_string(), "cyan");
        println!("{}", "  Proving...".yellow());
        display.note("(This may take several minutes)");

        // NOTE: We use tokio block_in_place here to mitigate two issues. One is that when using
        // the Bonsai version of the default prover, tokio may panic with an error about the
        // runtime being dropped. And spawn_blocking cannot be used because VerifierContext,
        // default_prover(), and ExecutorEnv do not implement Send.
        let prove_info = tokio::task::block_in_place(|| {
            prover.prove_update(work_receipts).context("Failed to prove work log update")
        })?;

        display.status("Status", "Complete", "green");

        // Backup before modifying state
        display.subsection("Saving updated mining state file");
        if !self.skip_backup {
            let backup_path = state.save_backup(&state_path)?;
            display.item_colored("Saved backup to", backup_path.display().to_string(), "dimmed");
        } else {
            println!("  {:<16} {} {}", "Backup:", "Skipped".yellow(), "(--skip-backup)".dimmed());
        }

        // Update and save the output state
        let updated_state = state
            .update_work_log(prover.work_log.clone(), prove_info.receipt)
            .context("Failed to update state")?;

        updated_state.save(&state_path).context("Failed to save state")?;

        let new_commit = prover.work_log.commit();
        display.status("State file", "Updated", "green");
        display.item_colored("Saved to", state_path.display().to_string(), "cyan");
        display.item_colored("New commit", new_commit.to_string(), "cyan");

        display.success(&format!("Successfully prepared mining state file update. Added {} new receipts to the work log.", num_receipts));
        println!();

        Ok(())
    }
}

/// Load work receipts from the specified files
async fn load_work_receipts(
    log_id: PovwLogId,
    work_log: &WorkLog,
    files: &[PathBuf],
) -> Vec<anyhow::Result<WorkReceipt>> {
    let mut work_receipts = Vec::new();
    for path in files {
        // Load the receipts, propagating an error on failure or if the receipt isn't for this log
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
        work_claim.nonce_max.log == log_id,
        "Receipt has a log ID that does not match the work log: receipt: {:x}, work log: {:x}",
        work_claim.nonce_max.log,
        log_id
    );

    ensure!(
        work_claim.nonce_min.job == work_claim.nonce_max.job,
        "Work claim nonce min and max job numbers do not match: {} != {}",
        work_claim.nonce_min.job,
        work_claim.nonce_max.job
    );
    ensure!(work_claim.nonce_min.segment == 0, "work claim nonce min segment number is not 0");

    ensure!(
        !work_log.jobs.contains_key(&work_claim.nonce_min.job),
        "Receipt has job ID that is already in the work log: {}",
        work_claim.nonce_min.job,
    );
    Ok(work_receipt)
}

/// Work receipt info matching Bento API format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReceiptInfo {
    pub key: String,
    /// PoVW log ID if PoVW is enabled, None otherwise
    pub povw_log_id: Option<String>,
    /// PoVW job number if PoVW is enabled, None otherwise
    pub povw_job_number: Option<String>,
}

/// Work receipt list matching Bento API format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkReceiptList {
    pub receipts: Vec<WorkReceiptInfo>,
}

async fn fetch_work_receipts(
    log_id: PovwLogId,
    work_log: &WorkLog,
    bento_url: &Url,
) -> anyhow::Result<Vec<anyhow::Result<WorkReceipt>>> {
    // Call the /work-receipts endpoint on Bento
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

    // Filter the list for new receipts
    let mut seen_log_ids = HashSet::new();
    let mut keys_to_fetch = HashSet::new();
    for info in &receipt_list.receipts {
        let (info_log_id, info_job_number) = match parse_receipt_info(info) {
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
            // Log any unknown log IDs we find, but only once
            if !seen_log_ids.insert(info_log_id) {
                // count number of receipts associated with this log id
                let count = receipt_list
                    .receipts
                    .iter()
                    .filter(|r| r.povw_log_id == Some(info_log_id.to_string()))
                    .count();
                println!(
                    "  {} {}",
                    "⚠".yellow(),
                    format!(
                        "Skipping {} receipts associated with Reward Address {info_log_id:x}",
                        count
                    )
                    .dimmed()
                );
            }
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

    // Fetch the new receipts
    let mut work_receipts = Vec::new();
    for key in keys_to_fetch {
        // NOTE: We return the result so that the caller can decide whether to skip or bail
        let work_receipt = fetch_work_receipt(bento_url, &key)
            .await
            .context("Failed to fetch work receipt")
            .and_then(|receipt| {
                check_work_receipt(log_id, work_log, receipt)
                    .with_context(|| format!("Receipt with key: {}", key))
            });

        if work_receipt.is_ok() {
            tracing::debug!("Loaded receipt with key: {key}");
        }

        work_receipts.push(work_receipt);
    }
    Ok(work_receipts)
}

// Parse the log ID and job ID from the WorkReceiptInfo, or return an error if they cannot be
// parsed or are not available
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
