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

use alloy::primitives::{utils::format_ether, U256};
use std::str::FromStr;

/// Format wei amount to human-readable ZKC
/// Converts from 18 decimals to ZKC units with full precision
pub fn format_zkc(wei_str: &str) -> String {
    match U256::from_str(wei_str) {
        Ok(wei) => {
            let formatted = format_ether(wei);
            format!("{} ZKC", formatted)
        }
        Err(_) => "0.0 ZKC".to_string(),
    }
}

/// Format wei amount to human-readable ETH
/// Converts from 18 decimals to ETH units with full precision
pub fn format_eth(wei_str: &str) -> String {
    match U256::from_str(wei_str) {
        Ok(wei) => {
            let formatted = format_ether(wei);
            format!("{} ETH", formatted)
        }
        Err(_) => "0.0 ETH".to_string(),
    }
}

/// Format work amount to human-readable cycles with commas
/// Work values are raw cycle counts (no decimals)
pub fn format_cycles(cycles_str: &str) -> String {
    match U256::from_str(cycles_str) {
        Ok(cycles) => {
            // Work values are already in cycles (no decimal conversion needed)
            let formatted = format_with_commas_u256(cycles);
            format!("{} cycles", formatted)
        }
        Err(_) => "0 cycles".to_string(),
    }
}

/// Format a u64 number with comma separators
#[allow(dead_code)]
pub fn format_with_commas(num: u64) -> String {
    let s = num.to_string();
    let mut result = String::new();
    let mut count = 0;

    for ch in s.chars().rev() {
        if count == 3 {
            result.insert(0, ',');
            count = 0;
        }
        result.insert(0, ch);
        count += 1;
    }

    result
}

/// Format a U256 number with comma separators
fn format_with_commas_u256(num: U256) -> String {
    let s = num.to_string();
    let mut result = String::new();
    let mut count = 0;

    for ch in s.chars().rev() {
        if count == 3 {
            result.insert(0, ',');
            count = 0;
        }
        result.insert(0, ch);
        count += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_zkc() {
        // Large amounts
        assert_eq!(format_zkc("1000000000000000000000"), "1000.000000000000000000 ZKC");
        assert_eq!(format_zkc("1500000000000000000000000"), "1500000.000000000000000000 ZKC");
        assert_eq!(format_zkc("788626950526189926000000"), "788626.950526189926000000 ZKC");

        // Small amounts (fractional ZKC)
        assert_eq!(format_zkc("1554869558951"), "0.000001554869558951 ZKC");
        assert_eq!(format_zkc("1000000000000000"), "0.001000000000000000 ZKC");
        assert_eq!(format_zkc("100000000000000000"), "0.100000000000000000 ZKC");

        // Edge cases
        assert_eq!(format_zkc("0"), "0.000000000000000000 ZKC");
        assert_eq!(format_zkc("1"), "0.000000000000000001 ZKC");
        assert_eq!(format_zkc("invalid"), "0.0 ZKC");
    }

    #[test]
    fn test_format_eth() {
        // Large amounts
        assert_eq!(format_eth("1000000000000000000000"), "1000.000000000000000000 ETH");
        assert_eq!(format_eth("1500000000000000000000000"), "1500000.000000000000000000 ETH");
        assert_eq!(format_eth("788626950526189926000000"), "788626.950526189926000000 ETH");

        // Small amounts (gwei range - typical gas fees)
        assert_eq!(format_eth("1554869558951"), "0.000001554869558951 ETH");
        assert_eq!(format_eth("8619988255263"), "0.000008619988255263 ETH");
        assert_eq!(format_eth("20476994041239"), "0.000020476994041239 ETH");
        assert_eq!(format_eth("1196099996666666"), "0.001196099996666666 ETH");

        // Standard amounts
        assert_eq!(format_eth("1000000000000000000"), "1.000000000000000000 ETH");
        assert_eq!(format_eth("1000000000000000"), "0.001000000000000000 ETH");
        assert_eq!(format_eth("100000000000000000"), "0.100000000000000000 ETH");

        // Edge cases
        assert_eq!(format_eth("0"), "0.000000000000000000 ETH");
        assert_eq!(format_eth("1"), "0.000000000000000001 ETH");
        assert_eq!(format_eth("invalid"), "0.0 ETH");
    }

    #[test]
    fn test_format_cycles() {
        assert_eq!(format_cycles("1"), "1 cycles");
        assert_eq!(format_cycles("1000"), "1,000 cycles");
        assert_eq!(format_cycles("30711723851776"), "30,711,723,851,776 cycles");
        assert_eq!(format_cycles("5000000"), "5,000,000 cycles");
        assert_eq!(format_cycles("0"), "0 cycles");
    }

    #[test]
    fn test_format_with_commas() {
        assert_eq!(format_with_commas(0), "0");
        assert_eq!(format_with_commas(100), "100");
        assert_eq!(format_with_commas(1000), "1,000");
        assert_eq!(format_with_commas(10000), "10,000");
        assert_eq!(format_with_commas(100000), "100,000");
        assert_eq!(format_with_commas(1000000), "1,000,000");
        assert_eq!(format_with_commas(1234567890), "1,234,567,890");
    }
}
