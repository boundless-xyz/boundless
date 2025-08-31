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

//! Commands of the Boundless CLI for Proof of Verifiable Work (PoVW) operations.

use std::{
    borrow::Borrow,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, ensure, Context, Result};
use boundless_povw_guests::log_updater::{prover::LogUpdaterProver, IPovwAccounting};
use clap::{Args, Subcommand};
use num_enum::TryFromPrimitive;
use risc0_povw::{
    guest::Journal as LogBuilderJournal, guest::RISC0_POVW_LOG_BUILDER_ID,
    prover::WorkLogUpdateProver, PovwLogId, WorkLog,
};
use risc0_zkvm::{
    default_prover, Digest, GenericReceipt, ProverOpts, Receipt, ReceiptClaim, VerifierContext,
    WorkClaim,
};
use serde::{Deserialize, Serialize};

use crate::config::GlobalConfig;

/// Private type alias used to make the function definitions in this file more concise.
type WorkReceipt = GenericReceipt<WorkClaim<ReceiptClaim>>;

/// Commands for Proof of Verifiable Work (PoVW) operations.
#[derive(Subcommand, Clone, Debug)]
pub enum PovwCommands {
    /// Compress a directory of work receipts into a work log update.
    ProveUpdate(PovwProveUpdate),
    /// Send a work log update to the onchain accounting contract.
    SendUpdate(PovwSendUpdate),
}

impl PovwCommands {
    /// Run the command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::ProveUpdate(cmd) => cmd.run().await,
            Self::SendUpdate(cmd) => cmd.run(global_config).await,
        }
    }
}

// TODO(povw): Adjust log levels

/// State of the work log update process. This is stored as a file between executions of these
/// commands to allow continuation of building a work log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct State {
    /// Work log identifier associated with the work log in this state.
    pub log_id: PovwLogId,
    /// A representation of the Merkle tree of nonces consumed as part of this work log.
    pub work_log: WorkLog,
    /// An ordered list of receipts for updates to the work log. The last receipt in this list will
    /// be used to continue updating the work log. These receipts are used to verify the state
    /// loaded into the guest as part of the continuation of the log builder.
    ///
    /// A list of receipts is kept to ensure that records are not lost that could prevent the
    /// prover from completing the onchain log update and minting operations.
    pub log_builder_receipts: Vec<Receipt>,
    /// Time at which this state was last updated.
    pub updated_at: SystemTime,
}

/// A one-byte version number tacked on to the front of the encoded state for cross-version compat.
#[repr(u8)]
#[non_exhaustive]
#[derive(Copy, Clone, Debug, TryFromPrimitive)]
enum StateVersion {
    V1,
}

impl State {
    /// Initialize a new work log state.
    pub fn new(log_id: PovwLogId) -> Self {
        Self {
            log_id,
            work_log: WorkLog::EMPTY,
            log_builder_receipts: Vec::new(),
            updated_at: SystemTime::now(),
        }
    }

    /// Encode this state into a buffer of bytes.
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        let mut buffer = vec![StateVersion::V1 as u8];
        buffer.extend_from_slice(&bincode::serialize(self)?);
        Ok(buffer)
    }

    /// Decode the state from a buffer of bytes.
    pub fn decode(buffer: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        let buffer = buffer.as_ref();
        if buffer.is_empty() {
            bail!("cannot decode state from empty buffer");
        }
        let (&[version], buffer) = buffer.split_at(1) else { unreachable!("can't touch this") };
        match version.try_into() {
            Ok(StateVersion::V1) => {
                bincode::deserialize(buffer).context("failed to deserialize state")
            }
            Err(_) => bail!("unknown state version number: {version}"),
        }
    }

    fn update(mut self, work_log: WorkLog, log_builder_receipt: Receipt) -> anyhow::Result<Self> {
        // Verify the Log Builder receipt. Ensure it matches the current state to avoid corruption.
        log_builder_receipt
            .verify(RISC0_POVW_LOG_BUILDER_ID)
            .context("Failed to verify Log Builder receipt")?;
        let log_builder_journal = LogBuilderJournal::decode(&log_builder_receipt.journal)
            .context("Failed to decode Log Builder journal")?;
        ensure!(
            log_builder_journal.self_image_id == Digest::from(RISC0_POVW_LOG_BUILDER_ID),
            "Log Builder journal self image ID does not match expected value: journal: {}, expected: {}",
            log_builder_journal.self_image_id,
            Digest::from(RISC0_POVW_LOG_BUILDER_ID),
        );
        ensure!(
            log_builder_journal.work_log_id == self.log_id,
            "Log Builder journal does not match the current state log ID: journal: {:x}, state: {:x}",
            log_builder_journal.work_log_id,
            self.log_id,
        );
        let initial_commit = self.work_log.commit();
        ensure!(
            log_builder_journal.initial_commit == initial_commit,
            "Log Builder journal does not match the current state commit: journal: {}, state: {}",
            log_builder_journal.initial_commit,
            initial_commit,
        );
        let updated_commit = work_log.commit();
        ensure!(
            log_builder_journal.updated_commit == updated_commit,
            "Log Builder journal does not match the updated work log commit: journal: {}, updated work log: {}",
            log_builder_journal.updated_commit,
            updated_commit,
        );

        self.log_builder_receipts.push(log_builder_receipt);
        self.work_log = work_log;
        self.updated_at = SystemTime::now();
        Ok(self)
    }

    /// Validate the consistency of this state by checking invariants.
    ///
    /// See [Self::validate_with_ctx].
    pub fn validate(&self) -> anyhow::Result<()> {
        self.validate_with_ctx(&VerifierContext::default())
    }

    /// Validate the consistency of this state by checking invariants.
    ///
    /// This method verifies:
    /// 1. All receipts in `log_builder_receipts` verify against the expected image ID
    /// 2. The journals form a proper chain with correct commit progression
    /// 3. All log IDs match the state's log ID
    ///
    /// Note that if the state contains many receipts, this could take a non-trivial amount of
    /// time to execute.
    ///
    /// The given verifier context is used for receipt verification.
    pub fn validate_with_ctx(&self, ctx: &VerifierContext) -> anyhow::Result<()> {
        // If there are no receipts, the state should have an empty work log
        if self.log_builder_receipts.is_empty() {
            ensure!(
                self.work_log.is_empty(),
                "State with no receipts should have an empty work log"
            );
            return Ok(());
        }

        // Validate the journal chain
        let mut expected_commit = WorkLog::EMPTY.commit();
        for (i, receipt) in self.log_builder_receipts.iter().enumerate() {
            receipt
                .verify_with_context(ctx, RISC0_POVW_LOG_BUILDER_ID)
                .with_context(|| format!("Receipt {} failed verification against image ID", i))?;

            let journal = LogBuilderJournal::decode(&receipt.journal)
                .with_context(|| format!("Failed to decode journal from receipt {}", i))?;

            if i == 0 {
                ensure!(
                    journal.initial_commit == WorkLog::EMPTY.commit(),
                    "First receipt initial commit should equal an empty work log commit. Expected: {}, Found: {}",
                    WorkLog::EMPTY.commit(),
                    journal.initial_commit
                );
            } else {
                ensure!(
                    journal.initial_commit == expected_commit,
                    "Receipt {} initial_commit should match previous receipt's updated_commit. Expected: {}, Found: {}",
                    i,
                    expected_commit,
                    journal.initial_commit
                );
            }

            ensure!(
                journal.work_log_id == self.log_id,
                "Receipt {} log ID should match state log ID. Expected: {:x}, Found: {:x}",
                i,
                self.log_id,
                journal.work_log_id
            );

            // Set up expected initial commit for next iteration
            expected_commit = journal.updated_commit;
        }

        // Verify that the final commit is equal to the work log commit.
        ensure!(
            expected_commit == self.work_log.commit(),
            "Final receipt updated commit should equal the current work log commit. Expected: {}, Found: {}",
            self.work_log.commit(),
            expected_commit
        );
        Ok(())
    }

    /// Load work log state from the given path.
    pub fn load(state_path: impl AsRef<Path>) -> anyhow::Result<State> {
        let state_path = state_path.as_ref();
        let state_data = fs::read(state_path).with_context(|| {
            format!("Failed to read work log state file: {}", state_path.display())
        })?;

        // TODO(povw): Apply some sanity checks here?
        State::decode(&state_data)
            .with_context(|| format!("Failed to decode state from file: {}", state_path.display()))
    }

    /// Save the work log state to the given path.
    pub fn save(&self, state_path: impl AsRef<Path>) -> Result<()> {
        let state_data = self.encode().context("Failed to serialize state")?;

        fs::write(state_path.as_ref(), &state_data).with_context(|| {
            format!("Failed to write state to {}", state_path.as_ref().display())
        })?;

        tracing::info!("Successfully saved work log state: {}", state_path.as_ref().display());
        tracing::info!("Updated commit: {}", self.work_log.commit());

        Ok(())
    }
}

/// Compress a directory of work receipts into a work log update.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwProveUpdate {
    /// Serialized work receipt files to add to the work log.
    #[arg(required = true, requires = "state")]
    work_receipts: Vec<PathBuf>,

    /// Work log identifier.
    ///
    /// The work log identifier is a 160-bit public key hash (i.e. an Ethereum address) which is
    /// used to identify the work log. A work log is a collection of work claims, including their
    /// value and nonces. A single work log can only include a nonce (and so a receipt) once.
    ///
    /// A prover may have one or more work logs, and may set the work log ID equal to their onchain
    /// prover address, or to a new address just used as the work log ID.
    #[arg(short, long)]
    log_id: PovwLogId,

    /// Output file for the Log Builder receipt and work log state.
    #[arg(short = 'o', long)]
    state_out: PathBuf,

    /// Continuation state and receipt from a previous log update.
    ///
    /// Set this flag to the output file from a previous run to update an existing work log.
    /// Either this flag or --new must be specified.
    #[arg(short = 'i', long, group = "state")]
    state_in: Option<PathBuf>,

    /// Create a new work log, adding the given receipts to it.
    ///
    /// Either this flag or --state-in must be specified.
    #[arg(short, long, group = "state")]
    new: bool,
}

impl PovwProveUpdate {
    /// Run the [PovwProveUpdate] command.
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting PoVW prove-update for log ID: {:x}", self.log_id);

        // Set up the work log update prover
        let prover_builder = WorkLogUpdateProver::builder()
            .prover(default_prover())
            .prover_opts(ProverOpts::succinct())
            .log_id(self.log_id)
            .log_builder_program(risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ELF)
            .context("Failed to build WorkLogUpdateProver")?;

        // Load the continuation state, if provided.
        let state = if let Some(continuation_path) = &self.state_in {
            let state = State::load(continuation_path)?;
            tracing::info!(
                "Loaded work log state from {} with commit {}",
                continuation_path.display(),
                state.work_log.commit()
            );
            state
        } else {
            tracing::info!("Initializing a new work log with ID {:x}", self.log_id);
            State::new(self.log_id)
        };

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

        // Load work receipt files, filtering out receipt files that we cannot add to the log.
        let work_receipts = self
            .load_work_receipts(&prover.work_log)
            .filter_map(|result| {
                result.map_err(|err| tracing::warn!("{:?}", err.context("Skipping receipt"))).ok()
            })
            .collect::<Vec<_>>();
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        ensure!(!work_receipts.is_empty(), "No work receipts will be processed");

        // Prove the work log update
        let prove_info =
            prover.prove_update(work_receipts).context("Failed to prove work log update")?;

        // Update and save the output state.
        let updated_state =
            state.update(prover.work_log, prove_info.receipt).context("Failed to update state")?;
        updated_state.save(&self.state_out).context("Failed to save state")?;

        Ok(())
    }

    /// Load work receipts from the specified directory
    fn load_work_receipts<'a>(
        &self,
        work_log: &'a WorkLog,
    ) -> impl Iterator<Item = anyhow::Result<WorkReceipt>> + use<'a, '_> {
        self.work_receipts.iter().map(|path| {
            if !path.is_file() {
                bail!("Work receipt path is not a file: {}", path.display())
            }

            // Check for receipt file extensions
            let work_receipt = self
                .load_work_receipt_file(path)
                .with_context(|| format!("Failed to load receipt from {}", path.display()))?;
            tracing::info!("Loaded receipt from: {}", path.display());

            self.check_work_receipt(work_log, work_receipt)
                .with_context(|| format!("Receipt from path {}", path.display()))
        })
    }

    /// Load a single receipt file
    fn load_work_receipt_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> anyhow::Result<WorkReceipt> {
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
        &self,
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
            work_claim.nonce_min.log == self.log_id,
            "Receipt has a log ID that does not match the work log: receipt: {:x}, work log: {:x}",
            work_claim.nonce_min.log,
            self.log_id
        );

        ensure!(
            !work_log.jobs.contains_key(&work_claim.nonce_min.job),
            "Receipt has job ID that is already in the work log: {}",
            work_claim.nonce_min.job,
        );
        Ok(work_receipt)
    }
}

/// Send a work log update to the PoVW accounting contract.
///
/// To prepare the update, this command creates a Groth16 proof, compressing the updates to be sent
/// and proving that they are authorized by the signing key for the work log.
#[non_exhaustive]
#[derive(Args, Clone, Debug)]
pub struct PovwSendUpdate {
    /// State of the work log, including receipts produced by the prove-update command.
    #[arg(short, long)]
    pub state: PathBuf,

    /// Private key used to sign work log updates. Should have an address equal to the work log ID.
    ///
    /// If this option is not set, the value of the private key from global config will be used.
    #[clap(long, env = "WORK_LOG_PRIVATE_KEY", hide_env_values = true)]
    pub work_log_private_key: Option<PrivateKeySigner>,

    // TODO(povw): Provide a default here, similar to the Deployment struct in boundless-market.
    /// Address of the PoVW accounting contract.
    pub povw_accounting_address: Address,
}

impl PovwSendUpdate {
    /// Run the [PovwSendUpdate] command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        let tx_signer = global_config.require_private_key()?;
        let work_log_signer = self.work_log_private_key.as_ref().unwrap_or(&tx_signer);
        let rpc_url = global_config.require_rpc_url()?;

        // Load the state and check to make sure the private key matches.
        let state = State::load(&self.state)
            .with_context(|| format!("Failed to load state from {}", self.state.display()))?;
        ensure!(
            Address::from(state.log_id) == work_log_signer.address(),
            "Signer does not match the state log ID: signer: {}, state: {}",
            work_log_signer.address(),
            state.log_id
        );

        // Connect to the chain.
        let provider = ProviderBuilder::new()
            .wallet(tx_signer.clone())
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let chain_id = provider
            .get_chain_id()
            .await
            .with_context(|| format!("failed to get chain ID from {rpc_url}"))?;
        let povw_accounting = IPovwAccounting::new(self.povw_accounting_address, provider.clone());

        // Get the current work log commit, to determine which update(s) should be applied.
        let onchain_commit =
            povw_accounting.workLogCommit(state.log_id.into()).call().await.with_context(|| {
                format!(
                    "Failed to get work log commit for {:x} from {:x}",
                    state.log_id, self.povw_accounting_address
                )
            })?;

        // Check if the latest log builder receipt has an updated_commit value equal to what is
        // onchain. If so, the onchain work log is already up to date.
        let Some(latest_receipt) = state.log_builder_receipts.last() else {
            bail!("Loaded state has no log builder receipts")
        };
        let latest_receipt_journal = LogBuilderJournal::decode(&latest_receipt.journal.bytes)
            .context("Failed to decode journal from latest receipt")?;
        if bytemuck::cast::<_, [u8; 32]>(latest_receipt_journal.updated_commit) == *onchain_commit {
            tracing::info!("Onchain PoVW accounting contract is already up to date with the latest commit in state");
            return Ok(());
        }

        // Find the index of the receipt in the state that has an initial commit equal to the
        // commit current onchain. We will send all updates after that point.
        let matching_receipt_index = state
            .log_builder_receipts
            .iter()
            .enumerate()
            .rev()
            .map(|(i, receipt)| {
                let journal =
                    LogBuilderJournal::decode(&receipt.journal.bytes).with_context(|| {
                        format!("Failed to decode journal from receipt in state at index {i}")
                    })?;
                anyhow::Ok(
                    (bytemuck::cast::<_, [u8; 32]>(journal.initial_commit) == *onchain_commit)
                        .then_some(i),
                )
            })
            .find_map(|x| x.transpose())
            .with_context(|| {
                format!("Failed to find receipt with initial commit matching {onchain_commit}")
            })??;

        // Iterate over all the log builder receipts that should be sent to the chain.
        // NOTE: In most cases, this will be one receipt. It may be more if the prover previously
        // built a work log update but it failed to send (e.g. network instability or high gas
        // fees caused the transaction not to go through).
        let receipts_for_update = state.log_builder_receipts[matching_receipt_index..].to_vec();
        if receipts_for_update.len() > 1 {
            tracing::info!(
                "Updating onchain work log {:x} with {} update receipts",
                state.log_id,
                receipts_for_update.len()
            )
        }
        for receipt in receipts_for_update {
            let prover = LogUpdaterProver::builder()
                .prover(default_prover())
                .chain_id(chain_id)
                .contract_address(self.povw_accounting_address)
                .prover_opts(ProverOpts::groth16())
                .build()
                .context("Failed to build prover for Log Updater")?;

            // Sign and prove the authorized work log update.
            // TODO(povw): Provide more info here.
            tracing::info!("Proving work log update");
            let prove_info = prover
                .prove_update(receipt, work_log_signer)
                .await
                .context("Failed to prove authorized log update")?;

            tracing::info!("Sending work log update transaction");
            let tx_result = povw_accounting
                .update_work_log(&prove_info.receipt)
                .context("Failed to construct update transaction")?
                .send()
                .await
                .context("Failed to send update transaction")?;
            tracing::info!(tx_hash = %tx_result.tx_hash(), "Sent transaction for work log update");

            let tx_receipt = tx_result
                .get_receipt()
                .await
                .context("Failed to receive receipt for update transaction")?;

            // Extract the WorkLogUpdated event
            let work_log_updated_event = tx_receipt
                .logs()
                .iter()
                .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
                .next();

            if let Some(event) = work_log_updated_event {
                let data = event.inner.data;
                tracing::info!(updated_commit = %data.updatedCommit, update_value = data.updateValue.to::<u64>(), "Work log update confirmed");
            } else {
                tracing::info!("Work log update confirmed");
                tracing::warn!("No WorkLogUpdated event in transaction receipt");
            }
        }

        Ok(())
    }
}
