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

use super::{
    IndexerService, DAILY_AGGREGATION_RECOMPUTE_DAYS, HOURLY_AGGREGATION_RECOMPUTE_HOURS,
    MONTHLY_AGGREGATION_RECOMPUTE_MONTHS, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_WEEK,
    WEEKLY_AGGREGATION_RECOMPUTE_WEEKS,
};
use crate::db::market::PeriodMarketSummary;
use crate::market::{
    pricing::compute_percentiles,
    time_boundaries::{get_day_start, get_month_start, get_next_day, get_next_month, get_next_week, get_week_start},
    ServiceError,
};
use ::boundless_market::contracts::pricing::price_at_time;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::U256;
use alloy::providers::Provider;
use anyhow::anyhow;
use std::str::FromStr;

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) async fn aggregate_hourly_market_data(
        &mut self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating hourly market data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

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

    pub(super) async fn aggregate_daily_market_data(
        &mut self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating daily market data for past {} days from block {} timestamp {}",
            DAILY_AGGREGATION_RECOMPUTE_DAYS,
            to_block,
            current_time
        );

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

    pub(super) async fn aggregate_weekly_market_data(
        &mut self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating weekly market data for past {} weeks from block {} timestamp {}",
            WEEKLY_AGGREGATION_RECOMPUTE_WEEKS,
            to_block,
            current_time
        );

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

    pub(super) async fn aggregate_monthly_market_data(
        &mut self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating monthly market data for past {} months from block {} timestamp {}",
            MONTHLY_AGGREGATION_RECOMPUTE_MONTHS,
            to_block,
            current_time
        );

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
    ) -> Result<PeriodMarketSummary, ServiceError> {
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
            all_lock_collaterals,
            total_program_cycles,
            total_cycles,
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
            self.db.get_period_all_lock_collateral(period_start, period_end),
            self.db.get_period_total_program_cycles(period_start, period_end),
            self.db.get_period_total_cycles(period_start, period_end),
        );

        // Unwrap all results
        let total_fulfilled = total_fulfilled?;
        let unique_provers = unique_provers?;
        let unique_requesters = unique_requesters?;
        let total_requests_submitted = total_requests_submitted?;
        let total_requests_submitted_onchain = total_requests_submitted_onchain?;
        let total_requests_submitted_offchain =
            total_requests_submitted - total_requests_submitted_onchain;
        let total_requests_locked = total_requests_locked?;
        let total_requests_slashed = total_requests_slashed?;
        let total_expired = total_expired?;
        let total_locked_and_expired = total_locked_and_expired?;
        let total_locked_and_fulfilled = total_locked_and_fulfilled?;
        let locks = locks?;
        let all_lock_collaterals = all_lock_collaterals?;
        let total_program_cycles = total_program_cycles?;
        let total_cycles = total_cycles?;

        let locked_orders_fulfillment_rate = {
            let total_locked_outcomes = total_locked_and_fulfilled + total_locked_and_expired;
            if total_locked_outcomes > 0 {
                (total_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
            } else {
                0.0
            }
        };

        // Compute fees and percentiles from fulfilled requests only
        // (where lock_prover_address == fulfill_prover_address)
        let mut total_fees = U256::ZERO;
        let mut prices: Vec<alloy::primitives::Uint<256, 4>> = Vec::new();

        for lock in locks {
            let min_price = U256::from_str(&lock.min_price)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse min_price: {}", e)))?;
            let max_price = U256::from_str(&lock.max_price)
                .map_err(|e| ServiceError::Error(anyhow!("Failed to parse max_price: {}", e)))?;

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
            prices.push(price);
        }

        // Compute total collateral from all locked requests (regardless of fulfillment)
        let mut total_collateral = U256::ZERO;
        for collateral_str in all_lock_collaterals {
            let lock_collateral = U256::from_str(&collateral_str).map_err(|e| {
                ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e))
            })?;
            total_collateral += lock_collateral;
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

        // TODO: Populate best prover metrics from fulfilled requests
        let best_peak_prove_mhz = 0;
        let best_peak_prove_mhz_prover = None;
        let best_peak_prove_mhz_request_id = None;
        let best_effective_prove_mhz = 0;
        let best_effective_prove_mhz_prover = None;
        let best_effective_prove_mhz_request_id = None;

        Ok(PeriodMarketSummary {
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
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id,
        })
    }
}
