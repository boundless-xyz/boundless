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

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use risc0_povw::guest::Journal as LogBuilderJournal;

use super::State;
use crate::{
    config::{GlobalConfig, RewardsConfig},
    display::DisplayManager,
};

/// Inspect a mining state file and display detailed statistics
#[derive(Args, Clone, Debug)]
pub struct RewardsInspectMiningState {
    /// Path to the mining state file (defaults to configured state file from setup)
    #[arg(long = "state-file", env = "MINING_STATE_FILE")]
    state_file: Option<PathBuf>,

    #[clap(flatten)]
    rewards_config: RewardsConfig,
}

impl RewardsInspectMiningState {
    /// Run the inspect-mining-state command
    pub async fn run(&self, _global_config: &GlobalConfig) -> Result<()> {
        let rewards_config = self.rewards_config.clone().load_from_files()?;

        // Determine state file path (param > config > error)
        let state_path = self.state_file.clone()
            .or_else(|| rewards_config.mining_state_file.clone().map(PathBuf::from))
            .context("No mining state file configured.\n\nTo configure: run 'boundless rewards setup' and enable mining\nOr set MINING_STATE_FILE env var")?;

        // Expand ~ to home directory if present
        let expanded_path = if state_path.to_string_lossy().starts_with("~/") {
            let home = dirs::home_dir().context("Failed to get home directory")?;
            home.join(state_path.to_string_lossy().strip_prefix("~/").unwrap())
        } else {
            state_path
        };

        // Get absolute path for display
        let display_path = std::fs::canonicalize(&expanded_path).unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(&expanded_path))
                .unwrap_or_else(|_| expanded_path.clone())
        });

        // Get file metadata
        let file_metadata = std::fs::metadata(&expanded_path)
            .with_context(|| format!("Failed to read file metadata: {}", display_path.display()))?;

        let file_size = file_metadata.len();
        let file_modified = file_metadata
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .map(format_duration)
            .unwrap_or_else(|| "unknown".to_string());

        // Load state
        let state = State::load(&expanded_path).await.with_context(|| {
            format!("Failed to load PoVW state file: {}", display_path.display())
        })?;

        // Print header
        let display = DisplayManager::new();
        println!();
        display.separator();
        println!("{}", "           Mining State File Inspector".bold().cyan());
        display.separator();

        // File Information
        display.subsection("File Information");
        display.item_colored("Path", display_path.display().to_string(), "cyan");
        display.item_colored("Size", format!("{} bytes", file_size), "cyan");
        println!("  {:<16} {}", "Last Modified:", file_modified.dimmed());

        // State Metadata
        display.subsection("State Metadata");
        display.item_colored("Log ID", format!("0x{:x}", state.log_id), "cyan");

        if let Ok(elapsed) = state.updated_at.elapsed() {
            println!("  {:<16} {}", "Last Updated:", format_duration(elapsed).dimmed());
        }

        // Work Log Information
        display.subsection("Work Log");
        display.item_colored("Commit", state.work_log.commit().to_string(), "cyan");

        if state.work_log.is_empty() {
            println!("  {:<16} {} {}", "Jobs:", "0".yellow(), "(empty work log)".dimmed());
        } else {
            let total_jobs = state.work_log.jobs.len();
            display.item_colored("Jobs", total_jobs.to_string(), "cyan");

            let mut job_ids: Vec<_> = state.work_log.jobs.keys().collect();
            job_ids.sort();

            let jobs_to_show: Vec<_> = job_ids.iter().rev().take(10).cloned().collect();
            let showing_count = jobs_to_show.len();

            println!(
                "\n  Last {} Jobs: (showing {} of {})",
                showing_count, showing_count, total_jobs
            );

            for (display_idx, job_id) in jobs_to_show.iter().enumerate() {
                if let Some(job_data) = state.work_log.jobs.get(*job_id) {
                    println!(
                        "    {}. Job ID: {} | Commit: {}",
                        (display_idx + 1).to_string().dimmed(),
                        job_id.to_string().cyan(),
                        format!("{:?}", job_data).dimmed()
                    );
                }
            }
        }

        // Log Builder Receipts
        display.subsection("Log Builder Receipts");
        display.item_colored("Count", state.log_builder_receipts.len().to_string(), "cyan");

        // Build a map of updated_commit -> receipt index for linking transactions
        use std::collections::HashMap;
        let mut commit_to_receipt: HashMap<String, usize> = HashMap::new();

        if !state.log_builder_receipts.is_empty() {
            println!("\n  Receipt Details:");

            for (idx, receipt) in state.log_builder_receipts.iter().enumerate() {
                println!(
                    "    {}. Receipt #{}",
                    (idx + 1).to_string().dimmed(),
                    idx.to_string().cyan()
                );

                if let Ok(journal) = LogBuilderJournal::decode(&receipt.journal.bytes) {
                    println!(
                        "       Initial commit:  {}",
                        journal.initial_commit.to_string().dimmed()
                    );
                    println!(
                        "       Updated commit:  {}",
                        journal.updated_commit.to_string().dimmed()
                    );

                    // Store commit mapping for later use in transaction linking
                    commit_to_receipt.insert(journal.updated_commit.to_string(), idx);
                } else {
                    println!("       {}", "Failed to decode journal".yellow());
                }
            }
        }

        // On-Chain Submissions
        display.subsection("On-Chain Submissions");

        // Check for unsubmitted work
        let has_unsubmitted_work = if !state.log_builder_receipts.is_empty() {
            // Get the latest receipt's updated commit
            if let Some(latest_receipt) = state.log_builder_receipts.last() {
                if let Ok(journal) = LogBuilderJournal::decode(&latest_receipt.journal.bytes) {
                    let latest_commit_str = journal.updated_commit.to_string();
                    // Check if any confirmed transaction has this commit
                    !state.update_transactions.values().any(|tx_state| {
                        if let Some(ref event) = tx_state.update_event {
                            format!("{:?}", event.updatedCommit) == latest_commit_str
                        } else {
                            false
                        }
                    })
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        if has_unsubmitted_work {
            display.warning("Has unsubmitted work");
            display.note("(run 'boundless rewards submit-mining')");
        } else if state.update_transactions.is_empty() {
            display.warning("Never submitted");
            display.note("(no transactions recorded)");
        } else {
            display.success("Up to date");
            display.note("(all work submitted)");
        }

        if !state.update_transactions.is_empty() {
            display.item_colored(
                "Transactions",
                state.update_transactions.len().to_string(),
                "cyan",
            );
            println!("\n  Transaction Details:");

            let mut txs: Vec<_> = state.update_transactions.iter().collect();
            txs.sort_by_key(|(hash, _)| format!("{:x}", hash));

            for (idx, (tx_hash, tx_state)) in txs.iter().enumerate() {
                println!(
                    "    {}. TX Hash:    {}",
                    (idx + 1).to_string().dimmed(),
                    format!("{:#x}", tx_hash).cyan()
                );

                if let Some(block_num) = tx_state.block_number {
                    println!("       Block:      {}", block_num.to_string().cyan());
                } else {
                    println!("       Block:      {}", "Pending".yellow());
                }

                if let Some(ref event) = tx_state.update_event {
                    println!("       Update Value: {}", event.updateValue.to_string().cyan());

                    // Link transaction to receipt
                    let updated_commit_str = format!("{:?}", event.updatedCommit);
                    if let Some(&receipt_idx) = commit_to_receipt.get(&updated_commit_str) {
                        println!("       Receipt:      #{}", receipt_idx.to_string().cyan());
                    }

                    println!(
                        "       Initial Commit: {}",
                        format!("{:?}", event.initialCommit).dimmed()
                    );
                    println!(
                        "       Updated Commit: {}",
                        format!("{:?}", event.updatedCommit).dimmed()
                    );
                }
            }
        }

        // Validation
        display.subsection("Validation");
        print!("  State Check:   ");
        match state.validate() {
            Ok(_) => {
                println!("{}", "Valid and consistent".green().bold());
            }
            Err(e) => {
                println!("{}", "Validation failed".red().bold());
                display.item_colored("Error", e.to_string(), "red");
            }
        }

        println!();
        display.separator();
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
