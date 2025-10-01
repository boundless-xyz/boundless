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

//! Commands for CLI setup and configuration.

mod completions;
mod config;

pub use completions::SetupCompletions;
pub use config::SetupConfig;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for setup and configuration
#[derive(Subcommand, Clone, Debug)]
pub enum SetupCommands {
    /// Display configuration and environment variables
    Config(SetupConfig),
    /// Print shell completions to stdout
    Completions(SetupCompletions),
}

impl SetupCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Config(cmd) => cmd.run(global_config).await,
            Self::Completions(cmd) => cmd.run(),
        }
    }
}