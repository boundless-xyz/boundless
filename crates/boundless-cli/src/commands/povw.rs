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

use std::path::PathBuf;

use clap::{Subcommand, Args};

/// Commands for Proof of Verifiable Work (PoVW) operations.
#[derive(Subcommand, Clone, Debug)]
pub enum PovwCommands {
    /// nop
    #[group(requires = "rpc_url")]
    Nop,

    /// Compress a directory of work receipts into a work log update.
    ProveUpdate(PovwProveUpdate),
}

impl PovwCommands {
    /// Run the command.
    pub async fn run(&self) -> anyhow::Result<()> {
        match self {
            Self::Nop => Ok(()),
            Self::ProveUpdate(cmd) => cmd.run(),
        }
    }
}

/// Compress a directory of work receipts into a work log update.
#[derive(Args, Clone, Debug)]
pub struct PovwProveUpdate {
    #[arg(long)]
    work_receipts_dir: PathBuf,
}

impl PovwProveUpdate {
    /// Run the [PovwProveUpdate] command.
    pub fn run(&self) -> anyhow::Result<()> {
        todo!()
    }
}
