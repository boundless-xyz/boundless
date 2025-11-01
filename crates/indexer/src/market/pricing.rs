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
use anyhow::Result;

/// Calculates the price at a given timestamp based on the offer parameters.
///
/// This implements the same logic as the Solidity `priceAt` function in Offer.sol.
///
/// Price increases linearly during the ramp-up period, then remains at the max price
/// until the lock deadline. After the lock deadline, the price goes to zero.
///
/// # Arguments
/// * `min_price` - Price at the start of the bidding period
/// * `max_price` - Price at the end of the bidding period
/// * `ramp_up_start` - Time at which the ramp-up period starts (UNIX timestamp)
/// * `ramp_up_period` - Length of the ramp-up period in seconds
/// * `lock_timeout` - Timeout for the lock, expressed as seconds from ramp up start
/// * `timestamp` - The time to calculate the price for (UNIX timestamp)
///
/// # Returns
/// The price at the given timestamp
pub fn price_at_time(
    min_price: U256,
    max_price: U256,
    ramp_up_start: u64,
    ramp_up_period: u32,
    lock_timeout: u32,
    timestamp: u64,
) -> U256 {
    // If before ramp-up starts, return min price
    if timestamp <= ramp_up_start {
        return min_price;
    }

    // Calculate lock deadline
    let lock_deadline = ramp_up_start + (lock_timeout as u64);

    // If after lock deadline, price is zero
    if timestamp > lock_deadline {
        return U256::ZERO;
    }

    // If within ramp-up period, calculate linear interpolation
    let ramp_up_end = ramp_up_start + (ramp_up_period as u64);
    if timestamp <= ramp_up_end {
        // Avoid division by zero
        if ramp_up_period == 0 {
            return min_price;
        }

        let rise = max_price - min_price;
        let run = U256::from(ramp_up_period);
        let delta = U256::from(timestamp - ramp_up_start);

        // price = minPrice + (delta * rise) / run
        let price = min_price + (delta * rise) / run;
        return price;
    }

    // After ramp-up period but before lock deadline, return max price
    max_price
}

/// Computes a single percentile from a list of U256 values.
///
/// # Arguments
/// * `values` - Sorted list of U256 values
/// * `percentile` - Percentile to compute (0-100)
///
/// # Returns
/// The value at the given percentile
pub fn compute_percentile(values: &[U256], percentile: u8) -> U256 {
    if values.is_empty() {
        return U256::ZERO;
    }

    if percentile == 0 {
        return values[0];
    }

    if percentile >= 100 {
        return values[values.len() - 1];
    }

    // Use linear interpolation between closest ranks
    let rank = (percentile as f64 / 100.0) * (values.len() - 1) as f64;
    let lower_idx = rank.floor() as usize;
    let upper_idx = rank.ceil() as usize;

    if lower_idx == upper_idx {
        return values[lower_idx];
    }

    // Linear interpolation between lower and upper values
    let lower_val = values[lower_idx];
    let upper_val = values[upper_idx];
    let fraction = rank - lower_idx as f64;

    // Interpolate: lower + fraction * (upper - lower)
    let diff = if upper_val > lower_val {
        upper_val - lower_val
    } else {
        return lower_val;
    };

    let fraction_u256 = U256::from((fraction * 1_000_000.0) as u64);
    let interpolated = (fraction_u256 * diff) / U256::from(1_000_000);

    lower_val + interpolated
}

/// Computes multiple percentiles from a list of U256 values.
///
/// # Arguments
/// * `values` - Mutable list of U256 values (will be sorted in place)
/// * `percentiles` - List of percentiles to compute (0-100)
///
/// # Returns
/// Vec of computed percentile values
pub fn compute_percentiles(values: &mut Vec<U256>, percentiles: &[u8]) -> Vec<U256> {
    if values.is_empty() {
        return vec![U256::ZERO; percentiles.len()];
    }

    // Sort values in place
    values.sort();

    percentiles.iter().map(|&p| compute_percentile(values, p)).collect()
}

/// Encodes a list of U256 values as a BYTEA blob.
///
/// Each U256 is encoded as 32 bytes in big-endian format.
///
/// # Arguments
/// * `values` - List of U256 values to encode
///
/// # Returns
/// Byte vector containing all encoded values
pub fn encode_percentiles_to_bytes(values: &[U256]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 32);
    for value in values {
        bytes.extend_from_slice(&value.to_be_bytes::<32>());
    }
    bytes
}

/// Decodes a BYTEA blob into a list of U256 values.
///
/// Each U256 is expected to be encoded as 32 bytes in big-endian format.
///
/// # Arguments
/// * `bytes` - Byte slice containing encoded values
///
/// # Returns
/// Result containing vector of decoded U256 values
pub fn decode_percentiles_from_bytes(bytes: &[u8]) -> Result<Vec<U256>> {
    if bytes.len() % 32 != 0 {
        anyhow::bail!("Invalid percentile blob length: expected multiple of 32 bytes");
    }

    let mut values = Vec::new();
    for chunk in bytes.chunks(32) {
        let mut arr = [0u8; 32];
        arr.copy_from_slice(chunk);
        values.push(U256::from_be_bytes(arr));
    }

    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_price_at_time_before_ramp_up() {
        let min_price = U256::from(100);
        let max_price = U256::from(200);
        let ramp_up_start = 1000;
        let ramp_up_period = 60;
        let lock_timeout = 600;
        let timestamp = 500; // Before ramp-up starts

        let price = price_at_time(
            min_price,
            max_price,
            ramp_up_start,
            ramp_up_period,
            lock_timeout,
            timestamp,
        );

        assert_eq!(price, min_price);
    }

    #[test]
    fn test_price_at_time_during_ramp_up() {
        let min_price = U256::from(100);
        let max_price = U256::from(200);
        let ramp_up_start = 1000;
        let ramp_up_period = 100;
        let lock_timeout = 600;
        let timestamp = 1050; // Halfway through ramp-up

        let price = price_at_time(
            min_price,
            max_price,
            ramp_up_start,
            ramp_up_period,
            lock_timeout,
            timestamp,
        );

        // Should be halfway between min and max = 150
        assert_eq!(price, U256::from(150));
    }

    #[test]
    fn test_price_at_time_after_ramp_up() {
        let min_price = U256::from(100);
        let max_price = U256::from(200);
        let ramp_up_start = 1000;
        let ramp_up_period = 60;
        let lock_timeout = 600;
        let timestamp = 1100; // After ramp-up period, before lock deadline

        let price = price_at_time(
            min_price,
            max_price,
            ramp_up_start,
            ramp_up_period,
            lock_timeout,
            timestamp,
        );

        assert_eq!(price, max_price);
    }

    #[test]
    fn test_price_at_time_after_lock_deadline() {
        let min_price = U256::from(100);
        let max_price = U256::from(200);
        let ramp_up_start = 1000;
        let ramp_up_period = 60;
        let lock_timeout = 600;
        let timestamp = 2000; // After lock deadline

        let price = price_at_time(
            min_price,
            max_price,
            ramp_up_start,
            ramp_up_period,
            lock_timeout,
            timestamp,
        );

        assert_eq!(price, U256::ZERO);
    }

    #[test]
    fn test_price_at_time_zero_ramp_up_period() {
        let min_price = U256::from(100);
        let max_price = U256::from(200);
        let ramp_up_start = 1000;
        let ramp_up_period = 0; // Instant ramp-up
        let lock_timeout = 600;
        let timestamp = 1000; // At ramp-up start

        let price = price_at_time(
            min_price,
            max_price,
            ramp_up_start,
            ramp_up_period,
            lock_timeout,
            timestamp,
        );

        // Should return min_price when ramp_up_period is 0
        assert_eq!(price, min_price);
    }

    #[test]
    fn test_compute_percentile_empty() {
        let values: Vec<U256> = vec![];
        let p50 = compute_percentile(&values, 50);
        assert_eq!(p50, U256::ZERO);
    }

    #[test]
    fn test_compute_percentile_single_value() {
        let values = vec![U256::from(100)];
        let p50 = compute_percentile(&values, 50);
        assert_eq!(p50, U256::from(100));
    }

    #[test]
    fn test_compute_percentile_median() {
        let values = vec![
            U256::from(10),
            U256::from(20),
            U256::from(30),
            U256::from(40),
            U256::from(50),
        ];
        let p50 = compute_percentile(&values, 50);
        assert_eq!(p50, U256::from(30)); // Median of 5 values
    }

    #[test]
    fn test_compute_percentiles() {
        let mut values =
            vec![U256::from(10), U256::from(20), U256::from(30), U256::from(40), U256::from(50)];
        let percentiles = compute_percentiles(&mut values, &[0, 25, 50, 75, 100]);

        assert_eq!(percentiles[0], U256::from(10)); // min
        assert_eq!(percentiles[2], U256::from(30)); // median
        assert_eq!(percentiles[4], U256::from(50)); // max
    }

    #[test]
    fn test_encode_decode_percentiles() {
        let values =
            vec![U256::from(100), U256::from(200), U256::from(300), U256::from(400), U256::from(500)];

        let encoded = encode_percentiles_to_bytes(&values);
        assert_eq!(encoded.len(), 5 * 32); // 5 U256s * 32 bytes each

        let decoded = decode_percentiles_from_bytes(&encoded).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn test_decode_invalid_length() {
        let invalid_bytes = vec![0u8; 31]; // Not a multiple of 32
        let result = decode_percentiles_from_bytes(&invalid_bytes);
        assert!(result.is_err());
    }
}
