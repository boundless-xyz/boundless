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

use super::{IndexerService, HOURLY_AGGREGATION_RECOMPUTE_HOURS, DAILY_AGGREGATION_RECOMPUTE_DAYS, WEEKLY_AGGREGATION_RECOMPUTE_WEEKS, MONTHLY_AGGREGATION_RECOMPUTE_MONTHS, SECONDS_PER_HOUR, SECONDS_PER_DAY, SECONDS_PER_WEEK};
use crate::db::market::HourlyMarketSummary;
use crate::market::{pricing::compute_percentiles, ServiceError};
use ::boundless_market::contracts::pricing::price_at_time;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::U256;
use alloy::providers::Provider;
use anyhow::anyhow;
use std::str::FromStr;

fn get_day_start(timestamp: u64) -> u64 {
    (timestamp / SECONDS_PER_DAY) * SECONDS_PER_DAY
}

/// Returns the start of the calendar week (Monday 00:00:00 UTC) for a given timestamp
/// Uses ISO 8601 standard where Monday is the first day of the week
fn get_week_start(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc, Weekday};
    
    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    let weekday = dt.weekday();
    
    // Calculate days to subtract to get to Monday
    let days_from_monday = match weekday {
        Weekday::Mon => 0,
        Weekday::Tue => 1,
        Weekday::Wed => 2,
        Weekday::Thu => 3,
        Weekday::Fri => 4,
        Weekday::Sat => 5,
        Weekday::Sun => 6,
    };
    
    let monday = dt - chrono::Duration::days(days_from_monday);
    let monday_start = monday.date_naive().and_hms_opt(0, 0, 0).unwrap();
    monday_start.and_utc().timestamp() as u64
}

/// Returns the start of the calendar month (1st day 00:00:00 UTC) for a given timestamp
fn get_month_start(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc};
    
    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    let month_start = Utc
        .with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0)
        .unwrap();
    month_start.timestamp() as u64
}

/// Returns the start of the next calendar day
fn get_next_day(timestamp: u64) -> u64 {
    get_day_start(timestamp) + SECONDS_PER_DAY
}

/// Returns the start of the next calendar week
fn get_next_week(timestamp: u64) -> u64 {
    get_week_start(timestamp) + SECONDS_PER_WEEK
}

/// Returns the start of the next calendar month
fn get_next_month(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc};

    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();

    // Add one month
    let next_month = if dt.month() == 12 {
        Utc.with_ymd_and_hms(dt.year() + 1, 1, 1, 0, 0, 0).unwrap()
    } else {
        Utc.with_ymd_and_hms(dt.year(), dt.month() + 1, 1, 0, 0, 0).unwrap()
    };

    next_month.timestamp() as u64
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) async fn aggregate_hourly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating hourly market data for past {} hours from block {} timestamp {}", HOURLY_AGGREGATION_RECOMPUTE_HOURS, to_block, current_time);

        // Calculate hours ago based on configured recompute window
        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);

        // Truncate to hour boundaries
        let start_hour = (hours_ago / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;
        let current_hour = (current_time / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;

        tracing::debug!(
            "Aggregating hours from {} to {} ({} hour window). Up to block timestamp: {}",
            start_hour,
            current_hour,
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            current_time
        );

        // Process each hour
        for hour_ts in (start_hour..=current_hour).step_by(SECONDS_PER_HOUR as usize) {
            let hour_end = hour_ts.saturating_add(SECONDS_PER_HOUR);
            let summary = self.compute_period_summary(hour_ts, hour_end).await?;
            self.db.upsert_hourly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_hourly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(super) async fn aggregate_daily_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating daily market data for past {} days from block {} timestamp {}", DAILY_AGGREGATION_RECOMPUTE_DAYS, to_block, current_time);

        // Get the current day start and calculate days ago
        let current_day_start = get_day_start(current_time);
        
        // Calculate which days to recompute
        let mut periods = Vec::new();
        let mut day_start = current_day_start;
        for _ in 0..DAILY_AGGREGATION_RECOMPUTE_DAYS {
            periods.push(day_start);
            // Go back one day
            day_start = day_start.saturating_sub(SECONDS_PER_DAY);
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} days from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_day_start
        );

        // Process each day
        for day_ts in periods {
            let day_end = get_next_day(day_ts);
            let summary = self.compute_period_summary(day_ts, day_end).await?;
            self.db.upsert_daily_market_summary(summary).await?;
        }

        tracing::info!("aggregate_daily_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(super) async fn aggregate_weekly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating weekly market data for past {} weeks from block {} timestamp {}", WEEKLY_AGGREGATION_RECOMPUTE_WEEKS, to_block, current_time);

        // Get the current week start and calculate weeks ago
        let current_week_start = get_week_start(current_time);
        
        // Calculate which weeks to recompute
        let mut periods = Vec::new();
        let mut week_start = current_week_start;
        for _ in 0..WEEKLY_AGGREGATION_RECOMPUTE_WEEKS {
            periods.push(week_start);
            // Go back one week
            week_start = week_start.saturating_sub(SECONDS_PER_WEEK);
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} weeks from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_week_start
        );

        // Process each week
        for week_ts in periods {
            let week_end = get_next_week(week_ts);
            let summary = self.compute_period_summary(week_ts, week_end).await?;
            self.db.upsert_weekly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_weekly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(super) async fn aggregate_monthly_market_data(&mut self, to_block: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!("Aggregating monthly market data for past {} months from block {} timestamp {}", MONTHLY_AGGREGATION_RECOMPUTE_MONTHS, to_block, current_time);

        // Get the current month start and calculate months ago
        let current_month_start = get_month_start(current_time);
        
        // Calculate which months to recompute
        let mut periods = Vec::new();
        let mut month_ts = current_month_start;
        for _ in 0..MONTHLY_AGGREGATION_RECOMPUTE_MONTHS {
            periods.push(month_ts);
            // Go back one month - need to use chrono for proper month arithmetic
            use chrono::{Datelike, TimeZone, Utc};
            let dt = Utc.timestamp_opt(month_ts as i64, 0).unwrap();
            let prev_month = if dt.month() == 1 {
                Utc.with_ymd_and_hms(dt.year() - 1, 12, 1, 0, 0, 0).unwrap()
            } else {
                Utc.with_ymd_and_hms(dt.year(), dt.month() - 1, 1, 0, 0, 0).unwrap()
            };
            month_ts = prev_month.timestamp() as u64;
        }
        periods.reverse();

        tracing::debug!(
            "Aggregating {} months from {} to {}",
            periods.len(),
            periods.first().unwrap_or(&0),
            current_month_start
        );

        // Process each month
        for month_ts in periods {
            let month_end = get_next_month(month_ts);
            let summary = self.compute_period_summary(month_ts, month_end).await?;
            self.db.upsert_monthly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_monthly_market_data completed in {:?}", start.elapsed());
        Ok(())
    }

    pub async fn compute_period_summary(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<HourlyMarketSummary, ServiceError> {
        // Execute all initial database queries in parallel
        let (
            total_fulfilled,
            unique_provers,
            unique_requesters,
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            locks,
        ) = tokio::join!(
            self.db.get_period_fulfilled_count(period_start, period_end),
            self.db.get_period_unique_provers(period_start, period_end),
            self.db.get_period_unique_requesters(period_start, period_end),
            self.db.get_period_total_requests_submitted(period_start, period_end),
            self.db.get_period_total_requests_submitted_onchain(period_start, period_end),
            self.db.get_period_total_requests_locked(period_start, period_end),
            self.db.get_period_total_requests_slashed(period_start, period_end),
            self.db.get_period_expired_count(period_start, period_end),
            self.db.get_period_locked_and_expired_count(period_start, period_end),
            self.db.get_period_locked_and_fulfilled_count(period_start, period_end),
            self.db.get_period_lock_pricing_data(period_start, period_end),
        );

        // Unwrap all results
        let total_fulfilled = total_fulfilled?;
        let unique_provers = unique_provers?;
        let unique_requesters = unique_requesters?;
        let total_requests_submitted = total_requests_submitted?;
        let total_requests_submitted_onchain = total_requests_submitted_onchain?;
        let total_requests_submitted_offchain = total_requests_submitted - total_requests_submitted_onchain;
        let total_requests_locked = total_requests_locked?;
        let total_requests_slashed = total_requests_slashed?;
        let total_expired = total_expired?;
        let total_locked_and_expired = total_locked_and_expired?;
        let total_locked_and_fulfilled = total_locked_and_fulfilled?;
        let locks = locks?;

        let locked_orders_fulfillment_rate = {
            let total_locked_outcomes = total_locked_and_fulfilled + total_locked_and_expired;
            if total_locked_outcomes > 0 {
                (total_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
            } else {
                0.0
            }
        };

        // Compute fees and collateral
        let mut total_fees = U256::ZERO;
        let mut total_collateral = U256::ZERO;
        let mut prices = Vec::new();

        for lock in locks {
            let min_price = U256::from_str(&lock.min_price)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse min_price: {}", e)))?;
            let max_price = U256::from_str(&lock.max_price)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse max_price: {}", e)))?;
            let lock_collateral = U256::from_str(&lock.lock_collateral)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e)))?;

            // Compute lock_timeout from lock_end and bidding_start
            // Use saturating conversion to handle edge case where timeout exceeds u32::MAX
            let lock_timeout_u64 = lock.lock_end.saturating_sub(lock.bidding_start);
            let lock_timeout = u32::try_from(lock_timeout_u64).unwrap_or_else(|_| {
                tracing::warn!(
                    "Lock timeout {} exceeds u32::MAX for request, using u32::MAX as fallback",
                    lock_timeout_u64
                );
                u32::MAX
            });

            // Compute price at lock time
            let price = price_at_time(
                min_price,
                max_price,
                lock.bidding_start,
                lock.ramp_up_period,
                lock_timeout,
                lock.lock_timestamp,
            );

            total_fees += price;
            total_collateral += lock_collateral;
            prices.push(price);
        }

        // Compute percentiles: p10, p25, p50, p75, p90, p95, p99
        let percentiles = if !prices.is_empty() {
            let mut sorted_prices = prices;
            compute_percentiles(&mut sorted_prices, &[10, 25, 50, 75, 90, 95, 99])
        } else {
            vec![U256::ZERO; 7]
        };

        // Format U256 values as zero-padded 78-character strings (matching rewards pattern)
        fn format_u256(value: U256) -> String {
            format!("{:0>78}", value)
        }

        Ok(HourlyMarketSummary {
            period_timestamp: period_start,
            total_fulfilled,
            unique_provers_locking_requests: unique_provers,
            unique_requesters_submitting_requests: unique_requesters,
            total_fees_locked: format_u256(total_fees),
            total_collateral_locked: format_u256(total_collateral),
            p10_fees_locked: format_u256(percentiles[0]),
            p25_fees_locked: format_u256(percentiles[1]),
            p50_fees_locked: format_u256(percentiles[2]),
            p75_fees_locked: format_u256(percentiles[3]),
            p90_fees_locked: format_u256(percentiles[4]),
            p95_fees_locked: format_u256(percentiles[5]),
            p99_fees_locked: format_u256(percentiles[6]),
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            locked_orders_fulfillment_rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_day_start() {
        // Test with a known midnight UTC timestamp
        let midnight_utc = 1699920000; // 2023-11-14 00:00:00 UTC

        // Midnight should return itself
        assert_eq!(get_day_start(midnight_utc), midnight_utc);

        // Various times within the same day should return the same day start (midnight)
        assert_eq!(get_day_start(midnight_utc + 1), midnight_utc); // 00:00:01
        assert_eq!(get_day_start(midnight_utc + 3600), midnight_utc); // 01:00:00
        assert_eq!(get_day_start(midnight_utc + 43200), midnight_utc); // 12:00:00 (noon)
        assert_eq!(get_day_start(midnight_utc + 86399), midnight_utc); // 23:59:59 (last second of day)

        // First second of next day should return next day's midnight
        assert_eq!(get_day_start(midnight_utc + 86400), midnight_utc + SECONDS_PER_DAY);
    }

    #[test]
    fn test_get_week_start() {
        use chrono::{Datelike, TimeZone, Weekday};
        
        // Test that weeks start on Monday (ISO 8601)
        // Using a known date: 2023-11-15 is a Wednesday
        let wednesday = 1700000000; // 2023-11-15 00:00:00 UTC (approximately)
        let week_start = get_week_start(wednesday);
        
        // Week start should be a Monday
        let dt = chrono::Utc.timestamp_opt(week_start as i64, 0).unwrap();
        assert_eq!(dt.weekday(), Weekday::Mon);
        
        // All days in the same week should return the same Monday
        let thursday = wednesday + 86400;
        let friday = wednesday + 2 * 86400;
        assert_eq!(get_week_start(thursday), week_start);
        assert_eq!(get_week_start(friday), week_start);
        
        // Sunday should still be in the same week (ISO 8601)
        let sunday = week_start + 6 * 86400;
        assert_eq!(get_week_start(sunday), week_start);
        
        // Next Monday should be a different week
        let next_monday = week_start + 7 * 86400;
        assert_eq!(get_week_start(next_monday), next_monday);
    }

    #[test]
    fn test_get_month_start() {
        use chrono::TimeZone;
        
        // Test mid-month timestamp
        let mid_month = chrono::Utc.with_ymd_and_hms(2023, 11, 15, 12, 30, 45).unwrap();
        let month_start = get_month_start(mid_month.timestamp() as u64);
        
        // Should return 1st of November at 00:00:00
        let expected = chrono::Utc.with_ymd_and_hms(2023, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(month_start, expected.timestamp() as u64);
        
        // Test last day of month
        let end_of_month = chrono::Utc.with_ymd_and_hms(2023, 11, 30, 23, 59, 59).unwrap();
        assert_eq!(get_month_start(end_of_month.timestamp() as u64), month_start);
        
        // Test first day of month
        let first_day = chrono::Utc.with_ymd_and_hms(2023, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(get_month_start(first_day.timestamp() as u64), month_start);
    }

    #[test]
    fn test_get_next_day() {
        let day_start = 1700000000;
        let day_start_aligned = (day_start / SECONDS_PER_DAY) * SECONDS_PER_DAY;
        
        let next_day = get_next_day(day_start_aligned);
        assert_eq!(next_day, day_start_aligned + SECONDS_PER_DAY);
        
        // Should work from any time within the day
        let mid_day = day_start_aligned + 43200; // noon
        assert_eq!(get_next_day(mid_day), day_start_aligned + SECONDS_PER_DAY);
    }

    #[test]
    fn test_get_next_week() {
        let wednesday = 1700000000;
        let week_start = get_week_start(wednesday);
        
        let next_week = get_next_week(wednesday);
        assert_eq!(next_week, week_start + SECONDS_PER_WEEK);
        
        // Should work from any day in the week
        let friday = wednesday + 2 * 86400;
        assert_eq!(get_next_week(friday), week_start + SECONDS_PER_WEEK);
    }

    #[test]
    fn test_get_next_month() {
        use chrono::TimeZone;
        
        // Test November -> December
        let november = chrono::Utc.with_ymd_and_hms(2023, 11, 15, 12, 30, 45).unwrap();
        let next_month = get_next_month(november.timestamp() as u64);
        let expected_dec = chrono::Utc.with_ymd_and_hms(2023, 12, 1, 0, 0, 0).unwrap();
        assert_eq!(next_month, expected_dec.timestamp() as u64);
        
        // Test December -> January (year rollover)
        let december = chrono::Utc.with_ymd_and_hms(2023, 12, 20, 10, 0, 0).unwrap();
        let next_month = get_next_month(december.timestamp() as u64);
        let expected_jan = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(next_month, expected_jan.timestamp() as u64);
    }

    #[test]
    fn test_month_boundaries() {
        use chrono::TimeZone;
        
        // Test months with different numbers of days
        // January (31 days)
        let jan = chrono::Utc.with_ymd_and_hms(2024, 1, 31, 23, 59, 59).unwrap();
        let next = get_next_month(jan.timestamp() as u64);
        let expected_feb = chrono::Utc.with_ymd_and_hms(2024, 2, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_feb.timestamp() as u64);
        
        // February leap year (29 days)
        let feb = chrono::Utc.with_ymd_and_hms(2024, 2, 29, 12, 0, 0).unwrap();
        let next = get_next_month(feb.timestamp() as u64);
        let expected_mar = chrono::Utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_mar.timestamp() as u64);
        
        // February non-leap year (28 days)
        let feb = chrono::Utc.with_ymd_and_hms(2023, 2, 28, 12, 0, 0).unwrap();
        let next = get_next_month(feb.timestamp() as u64);
        let expected_mar = chrono::Utc.with_ymd_and_hms(2023, 3, 1, 0, 0, 0).unwrap();
        assert_eq!(next, expected_mar.timestamp() as u64);
    }
}
