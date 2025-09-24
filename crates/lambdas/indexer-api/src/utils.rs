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

use alloy::primitives::U256;
use std::str::FromStr;

/// Format wei amount to human-readable ZKC with commas
/// Converts from 18 decimals to ZKC units
pub fn format_zkc(wei_str: &str) -> String {
    match U256::from_str(wei_str) {
        Ok(wei) => {
            // ZKC has 18 decimals
            let divisor = U256::from(10u64).pow(U256::from(18));
            let zkc = wei / divisor;
            let formatted = format_with_commas_u256(zkc);
            format!("{} ZKC", formatted)
        }
        Err(_) => "0 ZKC".to_string(),
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
        assert_eq!(format_zkc("1000000000000000000000"), "1,000 ZKC");
        assert_eq!(format_zkc("1500000000000000000000000"), "1,500,000 ZKC");
        assert_eq!(format_zkc("788626950526189926000000"), "788,626 ZKC");
        assert_eq!(format_zkc("0"), "0 ZKC");
        assert_eq!(format_zkc("invalid"), "0 ZKC");
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
