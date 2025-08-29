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
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Result};
use clap::{Args, Subcommand};
use num_enum::TryFromPrimitive;
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId, WorkLog};
use risc0_zkvm::{default_prover, GenericReceipt, Receipt, ReceiptClaim, WorkClaim};
use serde::{Deserialize, Serialize};

/// Commands for Proof of Verifiable Work (PoVW) operations.
#[derive(Subcommand, Clone, Debug)]
pub enum PovwCommands {
    /// Compress a directory of work receipts into a work log update.
    ProveUpdate(PovwProveUpdate),
}

impl PovwCommands {
    /// Run the command.
    pub async fn run(&self) -> anyhow::Result<()> {
        match self {
            Self::ProveUpdate(cmd) => cmd.run().await,
        }
    }
}

/// State of the work log update process. This is stored as a file between executions of these
/// commands to allow continuation of building a work log.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct State {
    /// Work log identifier associated with the work log in this state.
    log_id: PovwLogId,
    /// A representation of the Merkle tree of nonces consumed as part of this work log.
    work_log: WorkLog,
    /// Receipt proving the most recent update. This receipt is used to verify the state loaded
    /// into the guest as part of the continuation of the log builder.
    receipt: Receipt,
}

/// A one-byte version number tacked on to the front of the encoded state for cross-version compat.
#[repr(u8)]
#[non_exhaustive]
#[derive(Copy, Clone, Debug, TryFromPrimitive)]
enum StateVersion {
    V1,
}

impl State {
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
}

/// Compress a directory of work receipts into a work log update.
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
    #[arg(short, long)]
    output: PathBuf,

    /// Continuation state and receipt from a previous log update.
    ///
    /// Set this flag to the output file from a previous run to update an existing work log.
    /// Either this flag or --new must be specified.
    #[arg(short, long, group = "state")]
    continuation: Option<PathBuf>,

    /// Create a new work log, adding the given receipts to it.
    ///
    /// Either this flag or --continuation must be specified.
    #[arg(short, long, group = "state")]
    new: bool,
}

impl PovwProveUpdate {
    /// Run the [PovwProveUpdate] command.
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting PoVW prove-update for log ID: {:x}", self.log_id);

        // Load work receipt files
        let work_receipts = self.load_work_receipts().context("Failed to load work receipts")?;
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        // Set up the work log update prover
        let mut prover_builder = WorkLogUpdateProver::builder().prover(default_prover());
        prover_builder
            .log_id(self.log_id)
            .log_builder_program(risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ELF)
            .context("Failed to build WorkLogUpdateProver")?;

        // Load continuation if provided
        if let Some(continuation_path) = &self.continuation {
            let state = self.load_state(continuation_path)?;
            prover_builder
                .work_log(state.work_log, state.receipt)
                .context("Failed to build prover with given state")?;
        }

        let mut prover = prover_builder.build().context("Failed to build WorkLogUpdateProver")?;

        // Prove the work log update
        let prove_info =
            prover.prove_update(work_receipts).context("Failed to prove work log update")?;

        // Save the output
        // TODO: This needs to save the state, and not just Receipt.
        self.save_receipt(&prove_info.receipt).context("Failed to save receipt")?;

        Ok(())
    }

    /// Load work receipts from the specified directory
    fn load_work_receipts(&self) -> Result<Vec<GenericReceipt<WorkClaim<ReceiptClaim>>>> {
        let mut receipts = Vec::new();
        for path in self.work_receipts.iter() {
            if !path.is_file() {
                bail!("Work receipt path is not a file: {}", path.display())
            }

            // Check for receipt file extensions
            let receipt = self
                .load_receipt_file(path)
                .with_context(|| format!("Failed to load receipt from {}", path.display()))?;
            tracing::info!("Loaded receipt from: {}", path.display());

            let work_claim = receipt
                .claim()
                .as_value()
                .context("Loaded receipt has a pruned claim")?
                .work
                .as_value()
                .context("Loaded receipt has a pruned work claim")?
                .clone();
            // NOTE: If nonce_max does not have the same log ID as nonce_min, the exec will fail.
            ensure!(work_claim.nonce_min.log == self.log_id,
                "Loaded reacipt has a log ID that does not match the specified log ID: loaded: {:x}, specified: {:x}",
                work_claim.nonce_min.log,
                self.log_id
            );

            receipts.push(receipt);
        }
        Ok(receipts)
    }

    /// Load a single receipt file
    fn load_receipt_file(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<GenericReceipt<WorkClaim<ReceiptClaim>>> {
        let path = path.as_ref();
        let data =
            fs::read(path).with_context(|| format!("Failed to read file: {}", path.display()))?;

        // Deserialize as GenericReceipt<WorkClaim<ReceiptClaim>>
        // TODO: Provide a common library implementation of encoding that can be used by Bento,
        // r0vm, and this crate. bincode works, but is fragile to any changes so e.g. adding a
        // version number would help.
        let receipt: GenericReceipt<WorkClaim<ReceiptClaim>> = bincode::deserialize(&data)
            .with_context(|| format!("Failed to deserialize receipt from: {}", path.display()))?;

        Ok(receipt)
    }

    /// Load continuation receipt and work log state
    fn load_state(&self, state_path: impl AsRef<Path>) -> anyhow::Result<State> {
        let state_path = state_path.as_ref();
        let state_data = fs::read(state_path).with_context(|| {
            format!("Failed to read work log state file: {}", state_path.display())
        })?;

        State::decode(&state_data)
            .with_context(|| format!("Failed to decode state from file: {}", state_path.display()))
    }

    /// Save the work log update receipt
    fn save_receipt(&self, receipt: &Receipt) -> Result<()> {
        let receipt_data = bincode::serialize(receipt).context("Failed to serialize receipt")?;

        fs::write(&self.output, &receipt_data)
            .with_context(|| format!("Failed to write receipt to {}", self.output.display()))?;

        tracing::info!("Successfully created work log update receipt: {}", self.output.display());

        // Log receipt information
        // TODO: Decode the journal to show the information about the work log update.
        tracing::info!("Receipt journal: {:x?}", receipt.journal);

        Ok(())
    }
}
