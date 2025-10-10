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

//! Display utilities for consistent CLI output formatting

use alloy::primitives::{Address, B256, U256, utils::{format_ether, format_units}};
use colored::Colorize;
use std::fmt::Display;

/// Standard display formatter for CLI output
pub struct DisplayManager {
    /// Optional network name to display in headers
    network: Option<String>,
}

impl DisplayManager {
    /// Create a new display manager
    pub fn new() -> Self {
        Self { network: None }
    }

    /// Create a display manager with network context
    pub fn with_network(network: impl Into<String>) -> Self {
        Self {
            network: Some(network.into()),
        }
    }

    /// Print a section header with optional network badge
    pub fn header(&self, title: &str) {
        match &self.network {
            Some(network) => println!("\n{} [{}]", title.bold(), network.blue().bold()),
            None => println!("\n{}", title.bold()),
        }
    }

    /// Print a labeled value with standard indentation
    pub fn item(&self, label: &str, value: impl Display) {
        println!("  {:<16} {}", format!("{}:", label), value);
    }

    /// Print a labeled value with custom color
    pub fn item_colored(&self, label: &str, value: impl Display, color: &str) {
        let colored_value = match color {
            "green" => value.to_string().green().to_string(),
            "cyan" => value.to_string().cyan().to_string(),
            "yellow" => value.to_string().yellow().to_string(),
            "dimmed" => value.to_string().dimmed().to_string(),
            _ => value.to_string(),
        };
        println!("  {:<16} {}", format!("{}:", label), colored_value);
    }

    /// Print an address with standard formatting
    pub fn address(&self, label: &str, address: Address) {
        self.item_colored(label, format!("{:#x}", address), "dimmed");
    }

    /// Print a transaction hash
    pub fn tx_hash(&self, hash: B256) {
        self.item_colored("Transaction", format!("{:#x}", hash), "cyan");
    }

    /// Print a balance with token symbol
    pub fn balance(&self, label: &str, amount: &str, symbol: &str, color: &str) {
        let colored_amount = match color {
            "green" => amount.green().bold().to_string(),
            "cyan" => amount.cyan().bold().to_string(),
            "yellow" => amount.yellow().bold().to_string(),
            _ => amount.to_string(),
        };
        let colored_symbol = match color {
            "green" => symbol.green().to_string(),
            "cyan" => symbol.cyan().to_string(),
            "yellow" => symbol.yellow().to_string(),
            _ => symbol.to_string(),
        };
        println!("  {:<16} {} {}", format!("{}:", label), colored_amount, colored_symbol);
    }

    /// Print a success message
    pub fn success(&self, message: &str) {
        println!("\n{} {}", "✓".green().bold(), message.green().bold());
    }

    /// Print a warning message
    pub fn warning(&self, message: &str) {
        println!("\n{} {}", "⚠".yellow(), message.yellow());
    }

    /// Print an info message
    pub fn info(&self, message: &str) {
        println!("\n{} {}", "ℹ".blue(), message);
    }

    /// Print an error message
    pub fn error(&self, message: &str) {
        println!("\n{} {}", "✗".red().bold(), message.red());
    }

    /// Print a status update
    pub fn status(&self, label: &str, status: &str, color: &str) {
        let colored_status = match color {
            "green" => status.green().bold().to_string(),
            "yellow" => status.yellow().to_string(),
            "cyan" => status.cyan().to_string(),
            _ => status.to_string(),
        };
        self.item(label, colored_status);
    }

    /// Print a progress step
    pub fn step(&self, current: usize, total: usize, description: &str) {
        println!("  Step {}/{}:      {}", current, total, description.yellow());
    }

    /// Print a section separator
    pub fn separator(&self) {
        println!("{}", "═══════════════════════════════════════════════════════".bold());
    }

    /// Print a note or additional info
    pub fn note(&self, message: &str) {
        println!("  {}", message.dimmed());
    }
}

impl Default for DisplayManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Format an amount for display (removes trailing zeros)
pub fn format_amount(amount: &str) -> String {
    crate::format_amount(amount)
}

/// Format ETH amount from wei
pub fn format_eth(wei: U256) -> String {
    format_amount(&format_ether(wei))
}

/// Format token amount with custom decimals
pub fn format_token(amount: U256, decimals: u8) -> anyhow::Result<String> {
    Ok(format_amount(&format_units(amount, decimals)?))
}

/// Format an address for display
pub fn format_address(address: Address) -> String {
    format!("{:#x}", address)
}

/// Format a transaction hash for display
pub fn format_tx_hash(hash: B256) -> String {
    format!("{:#x}", hash)
}

/// Display a transaction confirmation flow
pub async fn display_transaction(
    display: &DisplayManager,
    tx_hash: B256,
    pending_message: &str,
    success_message: &str,
) {
    display.tx_hash(tx_hash);
    display.status("Status", pending_message, "yellow");

    // After confirmation (this would be called after awaiting the receipt)
    display.success(success_message);
}

/// Standard balance display with deposited and available amounts
pub fn display_balance_pair(
    display: &DisplayManager,
    deposited: &str,
    available: &str,
    symbol: &str,
) {
    display.balance("Deposited", deposited, symbol, "green");
    display.balance("Available", available, symbol, "cyan");
}

/// Display epoch information
pub fn display_epoch_info(display: &DisplayManager, epoch: u64, start_time: u64, end_time: u64) {
    display.item("Epoch", epoch);
    display.item("Start Time", crate::indexer_client::format_timestamp(&start_time.to_string()));
    display.item("End Time", crate::indexer_client::format_timestamp(&end_time.to_string()));
}

/// Create a progress bar for long operations
pub fn progress_bar(current: usize, total: usize, width: usize) -> String {
    let percentage = (current as f64 / total as f64 * 100.0) as usize;
    let filled = (current as f64 / total as f64 * width as f64) as usize;
    let empty = width - filled;

    format!(
        "[{}{}] {}%",
        "█".repeat(filled).green(),
        "░".repeat(empty).dimmed(),
        percentage
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_amount() {
        assert_eq!(format_amount("1.500000"), "1.5");
        assert_eq!(format_amount("0.100000"), "0.1");
        assert_eq!(format_amount("10.000000"), "10");
    }

    #[test]
    fn test_format_eth() {
        let wei = U256::from(1_500_000_000_000_000_000u64); // 1.5 ETH
        assert_eq!(format_eth(wei), "1.5");
    }

    #[test]
    fn test_progress_bar() {
        assert!(progress_bar(5, 10, 20).contains("50%"));
        assert!(progress_bar(10, 10, 20).contains("100%"));
    }
}