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
use risc0_povw::{prover::WorkLogUpdateProver, PovwLogId};
use risc0_zkvm::{default_prover, GenericReceipt, Receipt, ReceiptClaim, WorkClaim};

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

/// Compress a directory of work receipts into a work log update.
#[derive(Args, Clone, Debug)]
pub struct PovwProveUpdate {
    /// Serialized work receipt files to add to the work log.
    #[arg(required = true)]
    work_receipts: Vec<PathBuf>,

    /// Work log identifier
    #[arg(long)]
    log_id: PovwLogId,

    /// Output file for the work log update receipt
    #[arg(long)]
    output: Option<PathBuf>,

    /// Continuation receipt from previous updates
    #[arg(long)]
    continuation: Option<PathBuf>,
}

impl PovwProveUpdate {
    /// Run the [PovwProveUpdate] command.
    pub async fn run(&self) -> Result<()> {
        tracing::info!("Starting PoVW prove-update for log ID: {:x}", self.log_id);

        // Load work receipt files
        let work_receipts = self.load_work_receipts().context("Failed to load work receipts")?;
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        // Set up the work log update prover
        let mut prover = WorkLogUpdateProver::builder()
            .prover(default_prover())
            .log_id(self.log_id)
            .log_builder_program(risc0_povw::guest::RISC0_POVW_LOG_BUILDER_ELF)
            .context("Failed to build WorkLogUpdateProver")?
            .build()
            .context("Failed to build WorkLogUpdateProver")?;

        // Load continuation if provided
        if let Some(continuation_path) = &self.continuation {
            self.load_continuation(&mut prover, continuation_path)
                .context("Failed to load continuation")?;
        }

        // Prove the work log update
        let prove_info =
            prover.prove_update(work_receipts).context("Failed to prove work log update")?;

        // Save the output
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
        let receipt: GenericReceipt<WorkClaim<ReceiptClaim>> = bincode::deserialize(&data)
            .with_context(|| format!("Failed to deserialize receipt from: {}", path.display()))?;

        Ok(receipt)
    }

    /// Load continuation receipt and work log state
    fn load_continuation(
        &self,
        _prover: &mut WorkLogUpdateProver<impl risc0_zkvm::Prover>,
        continuation_path: impl AsRef<Path>,
    ) -> Result<()> {
        let continuation_path = continuation_path.as_ref();
        let _continuation_data = fs::read(continuation_path).with_context(|| {
            format!("Failed to read continuation file: {}", continuation_path.display())
        })?;

        // TODO: Load continuation receipt and work log state
        // This requires the WorkLog state and previous receipt
        tracing::warn!("Continuation support not fully implemented yet");

        Ok(())
    }

    /// Save the work log update receipt
    fn save_receipt(&self, receipt: &Receipt) -> Result<()> {
        let output_path =
            self.output.clone().unwrap_or_else(|| PathBuf::from("work_log_update.receipt"));

        let receipt_data = bincode::serialize(receipt).context("Failed to serialize receipt")?;

        fs::write(&output_path, &receipt_data)
            .with_context(|| format!("Failed to write receipt to {}", output_path.display()))?;

        tracing::info!("Successfully created work log update receipt: {}", output_path.display());

        // Log receipt information
        // TODO: Decode the journal to show the information about the work log update.
        tracing::info!("Receipt journal: {:x?}", receipt.journal);

        Ok(())
    }
}
