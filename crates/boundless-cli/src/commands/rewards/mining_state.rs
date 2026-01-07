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

//! Mining state management for rewards commands.

use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use alloy::{primitives::B256, rpc::types::TransactionReceipt};
use anyhow::{bail, ensure, Context, Result};
use atomicwrites::{AtomicFile, OverwriteBehavior};
use boundless_povw::log_updater::IPovwAccounting::{self, WorkLogUpdated};
use num_enum::TryFromPrimitive;
use risc0_povw::{
    guest::Journal as LogBuilderJournal, guest::RISC0_POVW_LOG_BUILDER_ID, PovwLogId, WorkLog,
};
use risc0_zkvm::{Digest, Receipt, VerifierContext};
use serde::{Deserialize, Serialize};

// TODO(povw): Add a test that decodes a byte string that is checks into git, to detect any
// breaking changes to decoding.

// NOTE: Any modifications that might break bincode encoding (most changes) should ensure there is
// a migration path to read the old state version and update to the new one.
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
    /// A map of the transaction hashes to related state. Used to determine which blocks have
    /// update events for the claim rewards operation.
    pub update_transactions: HashMap<B256, UpdateTransactionState>,
    /// Time at which this state was last updated.
    pub updated_at: SystemTime,
}

/// State of a log update transaction sent to the chain.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct UpdateTransactionState {
    /// Block number from the receipt for the confirmed transaction. None if pending.
    pub block_number: Option<u64>,
    /// [WorkLogUpdated] event from the confirmed transaction. None if pending.
    pub update_event: Option<WorkLogUpdated>,
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
            update_transactions: HashMap::new(),
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

    /// Update the work log state to the new [WorkLog] value and add a receipt.
    pub fn update_work_log(
        &mut self,
        work_log: WorkLog,
        log_builder_receipt: Receipt,
    ) -> anyhow::Result<&mut Self> {
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

    /// Add a pending transaction hash for a log update transaction.
    pub fn add_pending_update_tx(&mut self, tx_hash: B256) -> anyhow::Result<&mut Self> {
        self.update_transactions
            .entry(tx_hash)
            .or_insert(UpdateTransactionState { block_number: None, update_event: None });
        self.updated_at = SystemTime::now();
        Ok(self)
    }

    /// Add a confirmed transaction receipt for a log update.
    pub fn confirm_update_tx(
        &mut self,
        tx_receipt: &TransactionReceipt,
    ) -> anyhow::Result<&mut Self> {
        // Extract the WorkLogUpdated event
        let work_log_updated_event = tx_receipt
            .logs()
            .iter()
            .filter_map(|log| log.log_decode::<IPovwAccounting::WorkLogUpdated>().ok())
            .next()
            .with_context(|| {
                format!(
                    "No WorkLogUpdated event in transaction receipt for {}",
                    tx_receipt.transaction_hash
                )
            })?;

        let block_number = tx_receipt.block_number.with_context(|| {
            format!(
                "No block number event in transaction receipt for {}",
                tx_receipt.transaction_hash
            )
        })?;

        self.update_transactions.insert(
            tx_receipt.transaction_hash,
            UpdateTransactionState {
                block_number: Some(block_number),
                update_event: Some(work_log_updated_event.data().clone()),
            },
        );
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
    pub async fn load(state_path: impl AsRef<Path>) -> anyhow::Result<State> {
        let state_path = state_path.as_ref();
        let state_data = tokio::fs::read(state_path).await.with_context(|| {
            format!("Failed to read work log state file: {}", state_path.display())
        })?;

        State::decode(&state_data)
            .with_context(|| format!("Failed to decode state from file: {}", state_path.display()))
    }

    /// Save the work log state to the given path.
    pub fn save(&self, state_path: impl AsRef<Path>) -> Result<()> {
        let state_data = self.encode().context("Failed to serialize state")?;

        // Write the state data. Use AtomicFile to reduce the chance of corruption.
        AtomicFile::new(state_path.as_ref(), OverwriteBehavior::AllowOverwrite)
            .write(|f| f.write_all(&state_data))
            .with_context(|| {
                format!("Failed to write state to {}", state_path.as_ref().display())
            })?;

        tracing::debug!("Saved work log state: {}", state_path.as_ref().display());
        tracing::debug!("Updated commit: {}", self.work_log.commit());

        Ok(())
    }

    /// Save a backup of the state file to ~/.boundless/backups
    ///
    /// The backup filename format is: `{original_filename}.{timestamp}.{log_id_hex}.bak`
    /// Returns the path where the backup was saved.
    pub fn save_backup(&self, original_path: impl AsRef<Path>) -> Result<PathBuf> {
        let original_path = original_path.as_ref();
        let backup_path = compute_backup_path(original_path, self.log_id)?;

        self.save(&backup_path)
            .with_context(|| format!("Failed to save backup to {}", backup_path.display()))?;

        Ok(backup_path)
    }
}

/// Get the backup directory path (~/.boundless/backups)
fn backup_dir() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Failed to get home directory")?;
    let backup_dir = home_dir.join(".boundless").join("backups");

    std::fs::create_dir_all(&backup_dir)
        .with_context(|| format!("Failed to create backup directory: {}", backup_dir.display()))?;

    Ok(backup_dir)
}

/// Compute the backup file path for a given state file
///
/// Format: `~/.boundless/backups/{original_filename}.{timestamp}.{log_id_hex}.bak`
fn compute_backup_path(original_path: &Path, log_id: PovwLogId) -> Result<PathBuf> {
    let backup_dir = backup_dir()?;

    let original_filename = original_path
        .file_name()
        .context("Failed to get filename from state path")?
        .to_str()
        .context("State filename is not valid UTF-8")?;

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Failed to get current timestamp")?
        .as_secs();

    let log_id_hex = format!("{:x}", log_id);
    let backup_filename = format!("{}.{}.{}.bak", original_filename, timestamp, log_id_hex);

    Ok(backup_dir.join(backup_filename))
}
