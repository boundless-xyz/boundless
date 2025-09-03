use std::{
    borrow::Borrow,
    collections::{HashSet},
    fs,
    path::PathBuf,
    str::FromStr,
};

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
pub struct PovwProveUpdate {
    /// Serialized work receipt files to add to the work log.
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
    #[arg(long, group = "source", requires = "from-bento")]
    from_bento_url: Option<Url>,

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

    /// Path for the Log Builder receipt and work log state.
    #[arg(short, long)]
    state: PathBuf,

    #[clap(flatten, next_help_heading = "Prover")]
    prover_config: ProverConfig,
}

impl PovwProveUpdate {
    /// Run the [PovwProveUpdate] command.
    pub async fn run(&self) -> Result<()> {
        // Load the existing state, if provided.
        let mut state = if let Some(log_id) = self.new_log_id {
            if self.state.exists() {
                bail!("File already exists at the state path; refusing to overwrite");
            }
            tracing::info!("Initializing a new work log with ID {log_id:x}");
            State::new(log_id)
        } else {
            let state = State::load(&self.state).context("Failed to load state file")?;
            tracing::info!(
                "Loaded work log state from {} with commit {}",
                self.state.display(),
                state.work_log.commit()
            );
            state
        };

        tracing::info!("Starting PoVW prove-update for log ID: {:x}", state.log_id);

        let work_receipts = if self.from_bento {
            // Load the work receipts from Bento.
            let bento_url = self.from_bento_url.as_ref().unwrap_or(&self.prover_config.bento_api_url);
            fetch_work_receipts(state.log_id, &state.work_log, bento_url).await.context("Failed to fetch work receipts from Bento")?
        } else {
            // Load work receipt files, filtering out receipt files that we cannot add to the log.
            load_work_receipts(state.log_id, &state.work_log, &self.work_receipts_files)
                .filter_map(|result| {
                    result
                        .map_err(|err| tracing::warn!("{:?}", err.context("Skipping receipt")))
                        .ok()
                })
                .collect::<Vec<_>>()
        };
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        ensure!(!work_receipts.is_empty(), "No work receipts will be processed");

        // Set up the work log update prover
        self.prover_config.configure_proving_backend();
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
        let prove_info =
            prover.prove_update(work_receipts).context("Failed to prove work log update")?;

        // Update and save the output state.
        state
            .update_work_log(prover.work_log, prove_info.receipt)
            .context("Failed to update state")?
            .save(&self.state)
            .context("Failed to save state")?;

        Ok(())
    }
}

/// Load work receipts from the specified directory
fn load_work_receipts<'a, 'b>(
    log_id: PovwLogId,
    work_log: &'a WorkLog,
    files: &'b [PathBuf],
) -> impl Iterator<Item = anyhow::Result<WorkReceipt>> + use<'a, 'b> {
    files.iter().map(move |path| {
        if !path.is_file() {
            bail!("Work receipt path is not a file: {}", path.display())
        }

        // Check for receipt file extensions
        let work_receipt = load_work_receipt_file(path)
            .with_context(|| format!("Failed to load receipt from {}", path.display()))?;
        tracing::info!("Loaded receipt from: {}", path.display());

        check_work_receipt(log_id, work_log, work_receipt)
            .with_context(|| format!("Receipt from path {}", path.display()))
    })
}

/// Load a single receipt file
fn load_work_receipt_file(path: impl AsRef<std::path::Path>) -> anyhow::Result<WorkReceipt> {
    let path = path.as_ref();
    let data =
        fs::read(path).with_context(|| format!("Failed to read file: {}", path.display()))?;

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
) -> anyhow::Result<Vec<WorkReceipt>> {
    // Call the /work_receipts endpoint on Bento.
    let list_url = bento_url.join("work_receipts")?;
    let response =
        reqwest::get(list_url.clone()).await.context("Failed to query Bento for work receipts")?;
    let receipt_list: WorkReceiptList = response
        .json()
        .await
        .with_context(|| format!("Failed to fetch work receipts list from {list_url}"))?;

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
        // NOTE: Bail here instead of just warning as it may be preferable to retry the whole
        // update rather than building an update that includes receipts the failed to fetch due to
        // a temporary error.
        let work_receipt = fetch_work_receipt(bento_url, &key).await.context("Failed to fetch work receipt")?;
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
        .join("work_receipts/")?
        .join(key)
        .with_context(|| format!("Failed to build URL to fetch work receipt with key {key}"))?;
    let response = reqwest::get(receipt_url.clone())
        .await
        .with_context(|| format!("Failed to fetch work receipt with key {key}"))?;
    let receipt_bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to fetch work receipt with key {key}"))?;
    bincode::deserialize(&receipt_bytes)
        .with_context(|| format!("Failed to deserialize receipt with key {key}"))
}
