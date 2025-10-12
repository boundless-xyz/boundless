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

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use risc0_povw::guest::Journal as LogBuilderJournal;

use super::State;
use crate::config::{GlobalConfig, RewardsConfig};

/// Inspect a PoVW state file and display detailed statistics
#[derive(Args, Clone, Debug)]
pub struct RewardsInspectPovwState {
    /// Path to the PoVW state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "POVW_STATE_FILE")]
    state_file: Option<PathBuf>,

    #[clap(flatten)]
    rewards_config: RewardsConfig,
}

impl RewardsInspectPovwState {
    /// Run the inspect-povw-state command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.povw_state_file.clone().map(PathBuf::from))
            .context("No PoVW state file configured.\n\nTo configure: run 'boundless rewards setup' and enable PoVW\nOr set POVW_STATE_FILE env var")?;

        // Expand ~ to home directory if present
        let expanded_path = if state_path.to_string_lossy().starts_with("~/") {
            let home = dirs::home_dir().context("Failed to get home directory")?;
            home.join(state_path.to_string_lossy().strip_prefix("~/").unwrap())
        } else {
            state_path
        };

        // Get absolute path for display
        let display_path = std::fs::canonicalize(&expanded_path)
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .map(|cwd| cwd.join(&expanded_path))
                    .unwrap_or_else(|_| expanded_path.clone())
            });

        // Get file metadata
        let file_metadata = std::fs::metadata(&expanded_path)
            .with_context(|| format!("Failed to read file metadata: {}", display_path.display()))?;

        let file_size = file_metadata.len();
        let file_modified = file_metadata.modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(|d| format_duration(d))
            .unwrap_or_else(|| "unknown".to_string());

        // Load state
        let state = State::load(&expanded_path)
            .await
            .with_context(|| format!("Failed to load PoVW state file: {}", display_path.display()))?;

        // Print header
        println!("\n{}", "═══════════════════════════════════════════════════════".bold());
        println!("{}", "           PoVW State File Inspector".bold().cyan());
        println!("{}", "═══════════════════════════════════════════════════════".bold());

        // File Information
        println!("\n{}", "📁 File Information".bold().green());
        println!("  Path:          {}", display_path.display().to_string().cyan());
        println!("  Size:          {} bytes", file_size.to_string().cyan());
        println!("  Last Modified: {}", file_modified.dimmed());

        // State Metadata
        println!("\n{}", "📊 State Metadata".bold().green());
        println!("  Log ID:        {}", format!("{:x}", state.log_id).cyan());

        if let Ok(elapsed) = state.updated_at.elapsed() {
            println!("  Last Updated:  {}", format_duration(elapsed).dimmed());
        }

        // Work Log Information
        println!("\n{}", "🔨 Work Log".bold().green());
        println!("  Commit:        {}", state.work_log.commit().to_string().cyan());

        if state.work_log.is_empty() {
            println!("  Jobs:          {} {}", "0".yellow(), "(empty work log)".dimmed());
        } else {
            println!("  Jobs:          {}", state.work_log.jobs.len().to_string().cyan());
            println!("\n  {} Job Details:", "➤".bold());

            let mut job_ids: Vec<_> = state.work_log.jobs.keys().collect();
            job_ids.sort();

            for (idx, job_id) in job_ids.iter().enumerate() {
                if let Some(job_data) = state.work_log.jobs.get(*job_id) {
                    println!("    {}. Job ID: {} | Commit: {}",
                        (idx + 1).to_string().dimmed(),
                        job_id.to_string().cyan(),
                        format!("{:?}", job_data).dimmed());
                }
            }
        }

        // Log Builder Receipts
        println!("\n{}", "📜 Log Builder Receipts".bold().green());
        println!("  Count:         {}", state.log_builder_receipts.len().to_string().cyan());

        if !state.log_builder_receipts.is_empty() {
            println!("\n  {} Receipt Details:", "➤".bold());

            for (idx, receipt) in state.log_builder_receipts.iter().enumerate() {
                println!("    {}. Receipt #{}",
                    (idx + 1).to_string().dimmed(),
                    idx.to_string().cyan());

                if let Ok(journal) = LogBuilderJournal::decode(&receipt.journal.bytes) {
                    println!("       Initial:  {}", journal.initial_commit.to_string().dimmed());
                    println!("       Updated:  {}", journal.updated_commit.to_string().dimmed());
                } else {
                    println!("       {}", "Failed to decode journal".yellow());
                }
            }
        }

        // On-Chain Submissions
        println!("\n{}", "⛓️  On-Chain Submissions".bold().green());

        if state.update_transactions.is_empty() {
            println!("  Status:        {} {}", "Never submitted".yellow(), "(no transactions recorded)".dimmed());
        } else {
            println!("  Transactions:  {}", state.update_transactions.len().to_string().cyan());
            println!("\n  {} Transaction Details:", "➤".bold());

            let mut txs: Vec<_> = state.update_transactions.iter().collect();
            txs.sort_by_key(|(hash, _)| format!("{:x}", hash));

            for (idx, (tx_hash, tx_state)) in txs.iter().enumerate() {
                println!("    {}. TX Hash:    {}",
                    (idx + 1).to_string().dimmed(),
                    format!("{:#x}", tx_hash).cyan());

                if let Some(block_num) = tx_state.block_number {
                    println!("       Block:      {}", block_num.to_string().cyan());
                } else {
                    println!("       Block:      {}", "Pending".yellow());
                }

                if let Some(ref event) = tx_state.update_event {
                    println!("       Update Value: {}", event.updateValue.to_string().cyan());
                }
            }
        }

        // Validation
        println!("\n{}", "✓ Validation".bold().green());
        print!("  State Check:   ");
        match state.validate() {
            Ok(_) => {
                println!("{} {}", "✓".green().bold(), "State is valid and consistent".green());
            }
            Err(e) => {
                println!("{} {}", "✗".red().bold(), "State validation failed".red());
                println!("  Error:         {}", e.to_string().red());
            }
        }

        println!("\n{}", "═══════════════════════════════════════════════════════".bold());
        println!();

        Ok(())
    }
}

fn format_duration(duration: std::time::Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{}s ago", secs)
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}
