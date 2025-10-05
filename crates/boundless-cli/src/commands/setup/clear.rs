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

//! Clear all CLI configuration.

use anyhow::Result;
use clap::Args;
use colored::Colorize;
use inquire::Confirm;

use crate::config::GlobalConfig;
use crate::config_file::{Config, Secrets};

/// Clear all CLI configuration
#[derive(Args, Clone, Debug)]
pub struct SetupClear {
    /// Skip confirmation prompt
    #[clap(long, short)]
    pub force: bool,
}

impl SetupClear {
    /// Run the clear command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        println!("\n{}", "⚠️  Clear Boundless CLI Configuration".yellow().bold());
        println!("\nThis will delete all configuration files, including:");
        println!("  • Network settings");
        println!("  • Stored RPC URLs");
        println!("  • Private keys (if stored in config)");
        println!("\n{}", "ℹ️  This will NOT delete:".green());
        println!("  • PoVW state files (unsubmitted work remains safe)");
        println!("  • PoVW state file backups (in any directory)");

        if !self.force {
            let confirmed = Confirm::new("Are you sure you want to continue?")
                .with_default(false)
                .prompt()?;

            if !confirmed {
                println!("\n{}", "Cancelled".dimmed());
                return Ok(());
            }
        }

        println!("\n{}", "Clearing configuration...".bold());

        let mut cleared_any = false;

        // Try to delete config file
        if let Ok(config_path) = Config::path() {
            if config_path.exists() {
                std::fs::remove_file(&config_path)?;
                println!("  {} Removed {}", "✓".green().bold(), config_path.display());
                cleared_any = true;
            }
        }

        // Try to delete secrets file
        if let Ok(secrets_path) = Secrets::path() {
            if secrets_path.exists() {
                std::fs::remove_file(&secrets_path)?;
                println!("  {} Removed {}", "✓".green().bold(), secrets_path.display());
                cleared_any = true;
            }
        }

        if cleared_any {
            println!("\n{}", "✓ Configuration cleared successfully".green().bold());
            println!("  Run {} to reconfigure", "'boundless setup all'".cyan());
        } else {
            println!("\n{}", "No configuration files found".dimmed());
        }

        Ok(())
    }
}
