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

use alloy_primitives::U256;

/// Calculates the price at a given timestamp based on offer pricing parameters.
///
/// This function is separated from the Offer struct to allow reuse in contexts where
/// we have individual pricing parameters rather than an Offer struct (e.g., database
/// queries in the indexer). This ensures consistent pricing logic across the codebase.
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
        return min_price + (delta * rise) / run;
    }

    // After ramp-up period but before lock deadline, return max price
    max_price
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

        // Should return min_price when ramp_up_period is 0 (avoids division by zero)
        assert_eq!(price, min_price);
    }
}
