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

//! Time boundary calculation utilities for aggregation periods.
//!
//! This module provides functions to calculate period boundaries (day, week, month)
//! for market data aggregation. These functions are used by both the live indexer
//! and the backfill service to ensure consistent time boundaries across all
//! aggregation operations.

use super::service::{SECONDS_PER_DAY, SECONDS_PER_WEEK};

pub fn get_day_start(timestamp: u64) -> u64 {
    (timestamp / SECONDS_PER_DAY) * SECONDS_PER_DAY
}

/// Returns the start of the calendar week (Monday 00:00:00 UTC) for a given timestamp
/// Uses ISO 8601 standard where Monday is the first day of the week
pub fn get_week_start(timestamp: u64) -> u64 {
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
pub fn get_month_start(timestamp: u64) -> u64 {
    use chrono::{Datelike, TimeZone, Utc};

    let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
    let month_start = Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0).unwrap();
    month_start.timestamp() as u64
}

/// Returns the start of the next calendar day
pub fn get_next_day(timestamp: u64) -> u64 {
    get_day_start(timestamp) + SECONDS_PER_DAY
}

/// Returns the start of the next calendar week
pub fn get_next_week(timestamp: u64) -> u64 {
    get_week_start(timestamp) + SECONDS_PER_WEEK
}

/// Returns the start of the next calendar month
pub fn get_next_month(timestamp: u64) -> u64 {
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
