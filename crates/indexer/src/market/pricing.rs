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

use alloy::primitives::U256;

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
/// * `values` - Mutable slice of U256 values (will be sorted in place)
/// * `percentiles` - List of percentiles to compute (0-100)
///
/// # Returns
/// Vec of computed percentile values
pub fn compute_percentiles(values: &mut [U256], percentiles: &[u8]) -> Vec<U256> {
    if values.is_empty() {
        return vec![U256::ZERO; percentiles.len()];
    }

    // Sort values in place
    values.sort();

    percentiles.iter().map(|&p| compute_percentile(values, p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let values =
            vec![U256::from(10), U256::from(20), U256::from(30), U256::from(40), U256::from(50)];
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
    fn test_compute_percentiles_with_p5() {
        // 20 values: 1, 2, 3, ..., 20
        let mut values: Vec<U256> = (1..=20).map(U256::from).collect();
        let percentiles = compute_percentiles(&mut values, &[5, 10, 25, 50, 75, 90, 95, 99]);

        // p5 should be near the bottom of the distribution
        assert!(percentiles[0] <= percentiles[1], "p5 <= p10");
        assert!(percentiles[1] <= percentiles[2], "p10 <= p25");
        assert!(percentiles[2] <= percentiles[3], "p25 <= p50");
        assert!(percentiles[3] <= percentiles[4], "p50 <= p75");
        assert!(percentiles[4] <= percentiles[5], "p75 <= p90");
        assert!(percentiles[5] <= percentiles[6], "p90 <= p95");
        assert!(percentiles[6] <= percentiles[7], "p95 <= p99");

        // p5 of [1..=20] with linear interpolation: rank = 0.05 * 19 = 0.95
        // lower = values[0] = 1, upper = values[1] = 2, frac = 0.95
        // result = 1 + 0.95 * (2 - 1) = 1.95, truncated to 1 (U256 integer math)
        assert!(percentiles[0] >= U256::from(1) && percentiles[0] <= U256::from(2));
    }

    #[test]
    fn test_compute_latency_percentiles_p50_p90() {
        // Simulate time-to-lock values in seconds (10 requests)
        let time_to_lock_secs: Vec<u64> = vec![2, 5, 8, 12, 15, 20, 25, 30, 45, 60];
        let mut values: Vec<U256> = time_to_lock_secs.iter().map(|&v| U256::from(v)).collect();
        let percentiles = compute_percentiles(&mut values, &[50, 90]);

        let p50 = percentiles[0].to::<u64>();
        let p90 = percentiles[1].to::<u64>();

        // p50 should be around the median (between 15 and 20)
        assert!((15..=20).contains(&p50), "p50 should be ~15-20, got {}", p50);
        // p90 should be near the top (between 45 and 60)
        assert!((45..=60).contains(&p90), "p90 should be ~45-60, got {}", p90);
        // p90 >= p50 always
        assert!(p90 >= p50, "p90 ({}) should be >= p50 ({})", p90, p50);
    }

    #[test]
    fn test_compute_latency_percentiles_single_value() {
        let mut values = vec![U256::from(42u64)];
        let percentiles = compute_percentiles(&mut values, &[50, 90]);

        assert_eq!(percentiles[0], U256::from(42));
        assert_eq!(percentiles[1], U256::from(42));
    }

    #[test]
    fn test_compute_latency_percentiles_empty() {
        let mut values: Vec<U256> = vec![];
        let percentiles = compute_percentiles(&mut values, &[50, 90]);

        assert_eq!(percentiles[0], U256::ZERO);
        assert_eq!(percentiles[1], U256::ZERO);
    }
}
