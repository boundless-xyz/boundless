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

mod claim;
mod prepare;
mod state;
mod submit;

pub use claim::PovwClaim;
pub use prepare::PovwPrepare;
pub use state::State;
pub use submit::PovwSubmit;

use clap::Subcommand;
use risc0_zkvm::{GenericReceipt, ReceiptClaim, WorkClaim};

use crate::config::GlobalConfig;

/// Private type alias used to make the function definitions in this file more concise.
type WorkReceipt = GenericReceipt<WorkClaim<ReceiptClaim>>;

/// Commands for Proof of Verifiable Work (PoVW) operations.
#[derive(Subcommand, Clone, Debug)]
pub enum PovwCommands {
    /// Compress work receipts to prepare a work log update.
    Prepare(PovwPrepare),
    /// Authorize and submit a prepared work log update.
    Submit(PovwSubmit),
    /// Claim ZKC rewards associated with submitted work log updates in past epochs.
    Claim(PovwClaim),
}

impl PovwCommands {
    /// Run the command.
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Prepare(cmd) => cmd.run().await,
            Self::Submit(cmd) => cmd.run(global_config).await,
            Self::Claim(cmd) => cmd.run(global_config).await,
        }
    }
}
