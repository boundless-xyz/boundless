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

use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use risc0_binfmt::PovwLogId;
use risc0_povw::prover::WorkLogUpdateProver;
use risc0_zkvm::{
    default_prover, GenericReceipt, Receipt, ReceiptClaim, SuccinctReceipt, WorkClaim,
};

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
        tracing::info!("Starting PoVW prove-update for log ID: {}", self.log_id);

        // Load work receipt files
        let work_receipts = self.load_work_receipts().context("Failed to load work receipts")?;
        tracing::info!("Loaded {} work receipts", work_receipts.len());

        // TODO: Set up the work log update prover
        // TODO: Load continuation if provided
        // TODO: Prove the work log update
        // TODO: Save the output

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
                .load_receipt_file(&path)
                .with_context(|| format!("Failed to load receipt from {}", path.display()))?;
            tracing::info!("Loaded receipt from: {}", path.display());

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
}
