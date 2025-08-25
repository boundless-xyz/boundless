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

use clap::Subcommand;

/// Commands for Proof of Verifiable Work (PoVW) operations.
#[derive(Subcommand, Clone, Debug)]
pub enum PovwCommands {
    /// nop
    Nop,
}

impl PovwCommands {
    /// Run the command.
    pub async fn run(&self) -> anyhow::Result<()> {
        match self {
            Self::Nop => Ok(()),
        }
    }
}
