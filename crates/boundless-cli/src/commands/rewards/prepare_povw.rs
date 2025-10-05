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

use std::{borrow::Borrow, collections::HashSet, path::{Path, PathBuf}, str::FromStr};

use anyhow::{bail, ensure, Context, Result};
use clap::Args;
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId, WorkLog};
use risc0_zkvm::{default_prover, GenericReceipt, ProverOpts, ReceiptClaim, WorkClaim};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::commands::povw::State;
use crate::config::{GlobalConfig, ProverConfig, RewardsConfig};

/// Private type alias for work receipts
type WorkReceipt = GenericReceipt<WorkClaim<ReceiptClaim>>;

/// Prepare PoVW work log update from work receipts
#[derive(Args, Clone, Debug)]
pub struct RewardsPreparePoVW {
    /// Path to the PoVW state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "POVW_STATE_FILE")]
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

    /// Validate all work receipts without proving (dry-run)
    #[arg(long)]
    validate_receipts: bool,

    #[clap(flatten)]
    rewards_config: RewardsConfig,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl RewardsPreparePoVW {
    /// Run the prepare-povw command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.povw_state_file.clone().map(PathBuf::from))
            .context("No PoVW state file configured.\n\nTo configure: run 'boundless setup rewards' and enable PoVW\nOr set POVW_STATE_FILE env var")?;

        // Load existing state (MUST exist - no new log ID creation)
        let mut state = State::load(&state_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to load PoVW state file: {}\n\nEnsure the file exists and is valid. Run 'boundless setup rewards' to create one.",
                    state_path.display()
                )
            })?;

        tracing::info!("Loaded work log state from {}", state_path.display());
        tracing::debug!(commit = %state.work_log.commit(), "Loaded work log commit");
        tracing::info!("Preparing work log update for log ID: {:x}", state.log_id);

        // Determine work receipt source
        let work_receipt_results = if !self.work_receipt_files.is_empty() {
            // Load from files
            load_work_receipts(state.log_id, &state.work_log, &self.work_receipt_files).await
        } else {
            // Fetch from Bento (default behavior)
            let bento_url = match self.work_receipt_bento_api_url.clone() {
                Some(url) => url,
                None => Url::parse(&self.prover_config.bento_api_url)
                    .context("Failed to parse Bento API URL from prover config")?,
            };

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
            // Save the state file anyway, to ensure it exists
            state.save(&state_path).context("Failed to save state")?;
            return Ok(());
        }
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        // Perform deep validation on all receipts
        tracing::info!("Performing deep validation on {} receipts", work_receipts.len());
        let mut valid_count = 0;
        let mut total_size = 0usize;
        let mut deep_validation_errors = Vec::new();

        for (idx, receipt) in work_receipts.iter().enumerate() {
            // Track size by serializing
            let receipt_size = bincode::serialize(receipt)
                .map(|bytes| bytes.len())
                .unwrap_or(0);
            total_size += receipt_size;

            // Deep validation
            match check_work_receipt_deep(receipt) {
                Err(err) => {
                    tracing::error!("Receipt {idx} failed deep validation: {err:?}");
                    deep_validation_errors.push((idx, err));
                }
                Ok(_) => {
                    valid_count += 1;
                    tracing::debug!("Receipt {idx} passed deep validation ({receipt_size} bytes)");
                }
            }
        }

        // Report validation results
        let avg_size = if !work_receipts.is_empty() {
            total_size / work_receipts.len()
        } else {
            0
        };
        tracing::info!(
            "Validation complete: {valid_count}/{} receipts valid, avg size: {avg_size} bytes",
            work_receipts.len()
        );

        if !deep_validation_errors.is_empty() {
            tracing::error!("Found {} receipts with validation errors:", deep_validation_errors.len());
            for (idx, err) in &deep_validation_errors {
                tracing::error!("  Receipt {idx}: {err}");
            }
            bail!(
                "Deep validation failed for {}/{} receipts. Check Bento Redis memory if receipts are corrupted.",
                deep_validation_errors.len(),
                work_receipts.len()
            );
        }

        // If --validate-receipts, stop here
        if self.validate_receipts {
            println!("\n✓ Validation successful:");
            println!("  Total receipts: {}", work_receipts.len());
            println!("  Valid receipts: {valid_count}");
            println!("  Average size:   {avg_size} bytes");
            println!("  Total size:     {total_size} bytes");
            return Ok(());
        }

        // Set up the work log update prover
        self.prover_config.configure_proving_backend_with_health_check().await?;
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

        // Backup before modifying state
        if !self.skip_backup {
            save_state_backup(&state, &state_path)?;
        }

        // Update and save the output state
        state
            .update_work_log(prover.work_log, prove_info.receipt)
            .context("Failed to update state")?
            .save(&state_path)
            .context("Failed to save state")?;

        tracing::info!("Updated work log and prepared an update proof");
        Ok(())
    }
}

/// Save a backup of the state file to ~/.boundless
fn save_state_backup(state: &State, original_path: &Path) -> Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let home_dir = dirs::home_dir().context("Failed to get home directory")?;
    let backup_dir = home_dir.join(".boundless");

    std::fs::create_dir_all(&backup_dir)
        .with_context(|| format!("Failed to create backup directory: {}", backup_dir.display()))?;

    let original_filename = original_path
        .file_name()
        .context("Failed to get filename from state path")?
        .to_str()
        .context("State filename is not valid UTF-8")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Failed to get current timestamp")?
        .as_secs();

    let log_id_hex = format!("{:x}", state.log_id);
    let backup_filename = format!("{}.{}.{}.bak", original_filename, timestamp, log_id_hex);
    let backup_path = backup_dir.join(&backup_filename);

    state.save(&backup_path)
        .with_context(|| format!("Failed to save backup to {}", backup_path.display()))?;

    println!("Backup saved to: {}", backup_path.display());

    Ok(())
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
        !work_log.jobs.contains_key(&work_claim.nonce_min.job),
        "Receipt has job ID that is already in the work log: {}",
        work_claim.nonce_min.job,
    );
    Ok(work_receipt)
}

/// Deep validation of work receipt - checks nested data can be accessed
fn check_work_receipt_deep(work_receipt: &WorkReceipt) -> anyhow::Result<()> {
    // Try to access the work claim through check_work_receipt logic
    // This validates claim structure and work claim accessibility
    let work_claim = work_receipt
        .claim()
        .as_value()
        .context("Receipt has a pruned claim")?
        .work
        .as_value()
        .context("Receipt has a pruned work claim")?
        .clone();

    // Validate nonce data is accessible and non-zero
    ensure!(
        work_claim.nonce_min.log != risc0_povw::PovwLogId::ZERO,
        "Receipt has invalid nonce_min log ID (ZERO)"
    );

    ensure!(
        work_claim.nonce_max.log == work_claim.nonce_min.log,
        "Receipt has mismatched log IDs in nonce_min/nonce_max"
    );

    // Verify we can re-serialize the receipt (this is what gets sent to Bento)
    // If Redis corrupted the data, serialization might succeed but the bytes would be different
    let _receipt_bytes = bincode::serialize(work_receipt)
        .context("Failed to re-serialize receipt (may indicate internal corruption)")?;

    tracing::trace!("Deep validation passed for receipt");

    Ok(())
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
            // Log any unknown log IDs we find, but only once
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

    // Fetch the new receipts
    let mut work_receipts = Vec::new();
    for key in keys_to_fetch {
        // NOTE: We return the result so that the caller can decide whether to skip or bail
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

    // Check Content-Length header for Redis OOM detection
    let expected_size = response.headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok());

    let receipt_bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read work receipt bytes for key {key}"))?;

    let actual_size = receipt_bytes.len();
    tracing::debug!("Fetched receipt {key}: {} bytes", actual_size);

    // Detect potential Redis OOM or truncation
    if let Some(expected) = expected_size {
        if actual_size != expected {
            tracing::warn!(
                "Receipt size mismatch for {key}: expected {expected} bytes, got {actual_size} bytes. \
                This may indicate Redis memory issues or network problems."
            );
        }
    }

    // Warn if receipt seems suspiciously small
    if actual_size < 100 {
        tracing::warn!(
            "Receipt {key} is very small ({actual_size} bytes). \
            This may indicate corruption or incomplete data. Check Bento Redis memory."
        );
    }

    bincode::deserialize(&receipt_bytes)
        .with_context(|| format!("Failed to deserialize receipt with key {key} ({actual_size} bytes)"))
}
