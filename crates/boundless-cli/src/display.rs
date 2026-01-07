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

//! Display utilities for consistent CLI output formatting

use alloy::primitives::{
    utils::{format_ether, format_units},
    Address, B256, U256,
};
use chrono::{DateTime, Local};
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
        Self { network: Some(network.into()) }
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

    /// Print a subsection header (bold but not as prominent as main header)
    pub fn subsection(&self, title: &str) {
        println!("\n{}", title.bold().green());
    }

    /// Print a sub-item (extra indented under a main item)
    pub fn subitem(&self, prefix: &str, message: &str) {
        println!("    {} {}", prefix.dimmed(), message);
    }
}

impl Default for DisplayManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Format an amount with smart decimal precision.
/// Shows max 4 decimal places unless the first 4 are all zeros, in which case shows more.
/// Removes trailing zeros.
pub fn format_amount(amount_str: &str) -> String {
    if let Some((whole, decimal)) = amount_str.split_once('.') {
        let decimal_chars: Vec<char> = decimal.chars().collect();

        let first_four = decimal_chars.iter().take(4).collect::<String>();
        if first_four == "0000" || first_four.len() < 4 && first_four.chars().all(|c| c == '0') {
            let first_nonzero = decimal_chars.iter().position(|&c| c != '0');
            if let Some(pos) = first_nonzero {
                let precision = (pos + 2).min(decimal_chars.len());
                let trimmed = decimal_chars[..precision]
                    .iter()
                    .collect::<String>()
                    .trim_end_matches('0')
                    .to_string();
                if trimmed.is_empty() {
                    whole.to_string()
                } else {
                    format!("{}.{}", whole, trimmed)
                }
            } else {
                whole.to_string()
            }
        } else {
            let precision = 4.min(decimal_chars.len());
            let trimmed = decimal_chars[..precision]
                .iter()
                .collect::<String>()
                .trim_end_matches('0')
                .to_string();
            if trimmed.is_empty() {
                whole.to_string()
            } else {
                format!("{}.{}", whole, trimmed)
            }
        }
    } else {
        amount_str.to_string()
    }
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

/// Convert a timestamp to DateTime in the local timezone
pub fn convert_timestamp(timestamp: u64) -> DateTime<Local> {
    let t = DateTime::from_timestamp(timestamp as i64, 0).expect("invalid timestamp");
    t.with_timezone(&Local)
}

/// Get the network name from a chain ID
pub fn network_name_from_chain_id(chain_id: Option<u64>) -> &'static str {
    match chain_id {
        Some(1) => "Ethereum Mainnet",
        Some(8453) => "Base Mainnet",
        Some(84532) => "Base Sepolia",
        Some(11155111) => "Ethereum Sepolia",
        Some(_) => "Custom Network",
        None => "Unknown Network",
    }
}

/// Obscure a secret for display (show first 3 and last 3 characters)
pub fn obscure_secret(secret: &str) -> String {
    if secret.len() <= 8 {
        "****".to_string()
    } else {
        format!("{}...{}", &secret[..3], &secret[secret.len() - 3..])
    }
}

/// Obscure a single segment (between dots or slashes)
fn obscure_segment(segment: &str) -> String {
    let len = segment.len();
    if len <= 4 {
        segment.to_string()
    } else if len <= 10 {
        format!("{}***{}", segment[..2].to_lowercase(), segment[len - 2..].to_lowercase())
    } else {
        let show_chars = if len > 15 { 4 } else { 3 };
        let stars = if len > 15 { "*****" } else { "***" };
        format!(
            "{}{}{}",
            segment[..show_chars].to_lowercase(),
            stars,
            segment[len - show_chars..].to_lowercase()
        )
    }
}

/// Obscure a URL for display (obscure segments between dots and slashes)
pub fn obscure_url(url: &str) -> String {
    if let Some((protocol, rest)) = url.split_once("://") {
        let parts = rest.split('/').collect::<Vec<_>>();

        if parts.is_empty() {
            return url.to_string();
        }

        let host = parts[0];
        let obscured_host = host.split('.').map(obscure_segment).collect::<Vec<_>>().join(".");

        let obscured_path: Vec<String> =
            parts[1..].iter().map(|&segment| obscure_segment(segment)).collect();

        if obscured_path.is_empty() {
            format!("{}://{}", protocol, obscured_host)
        } else {
            format!("{}://{}/{}", protocol, obscured_host, obscured_path.join("/"))
        }
    } else {
        url.split('/').map(obscure_segment).collect::<Vec<_>>().join("/")
    }
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
    fn test_obscure_secret() {
        assert_eq!(obscure_secret("abc"), "****");
        assert_eq!(obscure_secret("12345678"), "****");
        assert_eq!(obscure_secret("abcdefghijk"), "abc...ijk");
        assert_eq!(obscure_secret("0x1234567890abcdef"), "0x1...def");
    }

    #[test]
    fn test_obscure_url_simple() {
        let url = "https://example.com/api/v1";
        let obscured = obscure_url(url);
        assert_eq!(obscured, "https://ex***le.com/api/v1");
    }

    #[test]
    fn test_obscure_url_with_api_key() {
        let url = "https://eth-mainnet.g.alchemy.com/v2/kEepgHsajdisoajJcfV";
        let obscured = obscure_url(url);
        assert_eq!(obscured, "https://eth***net.g.al***my.com/v2/keep*****jcfv");
    }

    #[test]
    fn test_obscure_url_long_segments() {
        let url = "https://verylongsubdomain.anotherlongdomain.com/verylongpath/anotherlongpath";
        let obscured = obscure_url(url);
        assert_eq!(obscured, "https://very*****main.anot*****main.com/ver***ath/ano***ath");
    }
}
