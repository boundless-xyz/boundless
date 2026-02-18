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

//! Commands for managing AI tool skill files.

mod dev;
mod install;

pub use dev::SkillDev;
pub use install::SkillInstall;

use clap::Subcommand;

use crate::config::GlobalConfig;

/// Commands for managing AI tool skill files
#[derive(Subcommand, Clone, Debug)]
pub enum SkillCommands {
    /// Install Boundless skill files for AI coding tools
    Install(SkillInstall),

    /// Symlink skill source files for live development (no recompile needed)
    Dev(SkillDev),
}

impl SkillCommands {
    /// Run the command
    pub async fn run(&self, global_config: &GlobalConfig) -> anyhow::Result<()> {
        match self {
            Self::Install(cmd) => cmd.run(global_config).await,
            Self::Dev(cmd) => cmd.run(global_config).await,
        }
    }
}
