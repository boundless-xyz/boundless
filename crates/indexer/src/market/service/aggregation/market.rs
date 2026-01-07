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

use super::super::{
    IndexerService, DAILY_AGGREGATION_RECOMPUTE_DAYS, HOURLY_AGGREGATION_RECOMPUTE_HOURS,
    MONTHLY_AGGREGATION_RECOMPUTE_MONTHS, SECONDS_PER_DAY, SECONDS_PER_HOUR, SECONDS_PER_WEEK,
    WEEKLY_AGGREGATION_RECOMPUTE_WEEKS,
};
use crate::db::market::{AllTimeMarketSummary, IndexerDb, PeriodMarketSummary};
use crate::market::{
    pricing::compute_percentiles,
    time_boundaries::{
        get_day_start, get_hour_start, get_month_start, get_week_start, iter_daily_periods,
        iter_hourly_periods, iter_monthly_periods, iter_weekly_periods,
    },
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
    pub(crate) async fn aggregate_hourly_market_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
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

        // Truncate to hour boundaries using DRY helper
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_hourly_market_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_hourly_market_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (hour starts for hourly aggregation)
        let expected_from = get_hour_start(from_time);
        let expected_to = get_hour_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow::anyhow!(
                "Time boundaries must be aligned to hour starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        tracing::debug!("Aggregating hours from {} to {}", from_time, to_time);

        // Process each hour using shared iteration helper
        // Always write an entry for each hour, even if there's no activity (creates zero-value entry)
        for (hour_ts, hour_end) in iter_hourly_periods(from_time, to_time) {
            let summary = self.compute_period_summary(hour_ts, hour_end).await?;
            // Always upsert, even if all values are zero - this ensures continuous time series
            self.db.upsert_hourly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_hourly_market_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_daily_market_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
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
        let start_day = current_day_start
            .saturating_sub((DAILY_AGGREGATION_RECOMPUTE_DAYS - 1) * SECONDS_PER_DAY);

        self.aggregate_daily_market_data_from(start_day, current_day_start).await
    }

    pub(crate) async fn aggregate_daily_market_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (day starts for daily aggregation)
        let expected_from = get_day_start(from_time);
        let expected_to = get_day_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow::anyhow!(
                "Time boundaries must be aligned to day starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        tracing::debug!("Aggregating days from {} to {}", from_time, to_time);

        // Process each day using shared iteration helper
        // Always write an entry for each day, even if there's no activity (creates zero-value entry)
        for (day_ts, day_end) in iter_daily_periods(from_time, to_time) {
            let summary = self.compute_period_summary(day_ts, day_end).await?;
            // Always upsert, even if all values are zero - this ensures continuous time series
            self.db.upsert_daily_market_summary(summary).await?;
        }

        tracing::info!("aggregate_daily_market_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_weekly_market_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
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
        let start_week = current_week_start
            .saturating_sub((WEEKLY_AGGREGATION_RECOMPUTE_WEEKS - 1) * SECONDS_PER_WEEK);

        self.aggregate_weekly_market_data_from(start_week, current_week_start).await
    }

    pub(crate) async fn aggregate_weekly_market_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (week starts for weekly aggregation)
        let expected_from = get_week_start(from_time);
        let expected_to = get_week_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow::anyhow!(
                "Time boundaries must be aligned to week starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        tracing::debug!("Aggregating weeks from {} to {}", from_time, to_time);

        // Process each week using shared iteration helper
        // Always write an entry for each week, even if there's no activity (creates zero-value entry)
        for (week_ts, week_end) in iter_weekly_periods(from_time, to_time) {
            let summary = self.compute_period_summary(week_ts, week_end).await?;
            // Always upsert, even if all values are zero - this ensures continuous time series
            self.db.upsert_weekly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_weekly_market_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_monthly_market_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
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

        // Calculate start month by going back N-1 months
        use chrono::{Datelike, TimeZone, Utc};
        let mut start_month = current_month_start;
        for _ in 0..(MONTHLY_AGGREGATION_RECOMPUTE_MONTHS - 1) {
            let dt = Utc.timestamp_opt(start_month as i64, 0).unwrap();
            let prev_month = if dt.month() == 1 {
                Utc.with_ymd_and_hms(dt.year() - 1, 12, 1, 0, 0, 0).unwrap()
            } else {
                Utc.with_ymd_and_hms(dt.year(), dt.month() - 1, 1, 0, 0, 0).unwrap()
            };
            start_month = prev_month.timestamp() as u64;
        }

        self.aggregate_monthly_market_data_from(start_month, current_month_start).await
    }

    pub(crate) async fn aggregate_monthly_market_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (month starts for monthly aggregation)
        let expected_from = get_month_start(from_time);
        let expected_to = get_month_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow::anyhow!(
                "Time boundaries must be aligned to month starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        tracing::debug!("Aggregating months from {} to {}", from_time, to_time);

        // Process each month using shared iteration helper
        // Always write an entry for each month, even if there's no activity (creates zero-value entry)
        for (month_ts, month_end) in iter_monthly_periods(from_time, to_time) {
            let summary = self.compute_period_summary(month_ts, month_end).await?;
            // Always upsert, even if all values are zero - this ensures continuous time series
            self.db.upsert_monthly_market_summary(summary).await?;
        }

        tracing::info!("aggregate_monthly_market_data_from completed in {:?}", start.elapsed());
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
            total_secondary_fulfillments,
            locks,
            all_lock_collaterals,
            locked_and_expired_collaterals,
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
            self.db.get_period_secondary_fulfillments_count(period_start, period_end),
            self.db.get_period_lock_pricing_data(period_start, period_end),
            self.db.get_period_all_lock_collateral(period_start, period_end),
            self.db.get_period_locked_and_expired_collateral(period_start, period_end),
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
        let total_secondary_fulfillments = total_secondary_fulfillments?;
        let locks = locks?;
        let all_lock_collaterals = all_lock_collaterals?;
        let locked_and_expired_collaterals = locked_and_expired_collaterals?;
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

        // Compute fees and per-cycle pricing percentiles from fulfilled requests only
        // (where lock_prover_address == fulfill_prover_address)
        let mut total_fees = U256::ZERO;
        let mut prices_per_cycle: Vec<alloy::primitives::Uint<256, 4>> = Vec::new();

        for lock in locks {
            // Use precomputed lock_price if available, otherwise compute it
            let price = if let Some(lock_price_str) = &lock.lock_price {
                U256::from_str(lock_price_str).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse lock_price: {}", e))
                })?
            } else {
                let min_price = U256::from_str(&lock.min_price).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse min_price: {}", e))
                })?;
                let max_price = U256::from_str(&lock.max_price).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse max_price: {}", e))
                })?;

                // Compute lock_timeout from lock_end and ramp_up_start
                let lock_timeout_u64 = lock.lock_end.saturating_sub(lock.ramp_up_start);
                let lock_timeout = u32::try_from(lock_timeout_u64).unwrap_or_else(|_| {
                    tracing::warn!(
                        "Lock timeout {} exceeds u32::MAX for request, using u32::MAX as fallback",
                        lock_timeout_u64
                    );
                    u32::MAX
                });

                price_at_time(
                    min_price,
                    max_price,
                    lock.ramp_up_start,
                    lock.ramp_up_period,
                    lock_timeout,
                    lock.lock_timestamp,
                )
            };

            total_fees += price;

            // Use precomputed lock_price_per_cycle if available
            if let Some(price_per_cycle_str) = &lock.lock_price_per_cycle {
                if let Ok(price_per_cycle) = U256::from_str(price_per_cycle_str) {
                    prices_per_cycle.push(price_per_cycle);
                }
            }
        }

        // Compute total collateral from all locked requests (regardless of fulfillment)
        let mut total_collateral = U256::ZERO;
        for collateral_str in all_lock_collaterals {
            let lock_collateral = U256::from_str(&collateral_str).map_err(|e| {
                ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e))
            })?;
            total_collateral += lock_collateral;
        }

        // Compute total collateral from locked requests that expired
        let mut total_locked_and_expired_collateral = U256::ZERO;
        for collateral_str in locked_and_expired_collaterals {
            let lock_collateral = U256::from_str(&collateral_str).map_err(|e| {
                ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e))
            })?;
            total_locked_and_expired_collateral += lock_collateral;
        }

        // Compute percentiles: p10, p25, p50, p75, p90, p95, p99
        let percentiles = if !prices_per_cycle.is_empty() {
            let mut sorted_prices = prices_per_cycle;
            compute_percentiles(&mut sorted_prices, &[10, 25, 50, 75, 90, 95, 99])
        } else {
            vec![U256::ZERO; 7]
        };

        // TODO: Populate best prover metrics from fulfilled requests
        let best_peak_prove_mhz = 0.0;
        let best_peak_prove_mhz_prover = None;
        let best_peak_prove_mhz_request_id = None;
        let best_effective_prove_mhz = 0.0;
        let best_effective_prove_mhz_prover = None;
        let best_effective_prove_mhz_request_id = None;

        Ok(PeriodMarketSummary {
            period_timestamp: period_start,
            total_fulfilled,
            unique_provers_locking_requests: unique_provers,
            unique_requesters_submitting_requests: unique_requesters,
            total_fees_locked: total_fees,
            total_collateral_locked: total_collateral,
            total_locked_and_expired_collateral,
            p10_lock_price_per_cycle: percentiles[0],
            p25_lock_price_per_cycle: percentiles[1],
            p50_lock_price_per_cycle: percentiles[2],
            p75_lock_price_per_cycle: percentiles[3],
            p90_lock_price_per_cycle: percentiles[4],
            p95_lock_price_per_cycle: percentiles[5],
            p99_lock_price_per_cycle: percentiles[6],
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            total_secondary_fulfillments,
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

/// Helper function to sum hourly aggregates into a base all-time aggregate
pub fn sum_hourly_aggregates_into_base(
    base: &mut AllTimeMarketSummary,
    hourly_summaries: &[PeriodMarketSummary],
) {
    if hourly_summaries.is_empty() {
        return;
    }

    // Sum numeric fields
    for hourly in hourly_summaries {
        base.total_fulfilled += hourly.total_fulfilled;
        base.total_requests_submitted += hourly.total_requests_submitted;
        base.total_requests_submitted_onchain += hourly.total_requests_submitted_onchain;
        base.total_requests_submitted_offchain += hourly.total_requests_submitted_offchain;
        base.total_requests_locked += hourly.total_requests_locked;
        base.total_requests_slashed += hourly.total_requests_slashed;
        base.total_expired += hourly.total_expired;
        base.total_locked_and_expired += hourly.total_locked_and_expired;
        base.total_locked_and_fulfilled += hourly.total_locked_and_fulfilled;
        base.total_secondary_fulfillments += hourly.total_secondary_fulfillments;
        base.total_program_cycles += hourly.total_program_cycles;
        base.total_cycles += hourly.total_cycles;
    }

    // Sum U256 fields (fees, collateral)
    for hourly in hourly_summaries {
        base.total_fees_locked += hourly.total_fees_locked;
        base.total_collateral_locked += hourly.total_collateral_locked;
        base.total_locked_and_expired_collateral += hourly.total_locked_and_expired_collateral;
    }

    // Find best metrics (maximum mhz)
    let mut best_peak_mhz = base.best_peak_prove_mhz;
    let mut best_peak_prover = base.best_peak_prove_mhz_prover.clone();
    let mut best_peak_request_id = base.best_peak_prove_mhz_request_id;

    let mut best_effective_mhz = base.best_effective_prove_mhz;
    let mut best_effective_prover = base.best_effective_prove_mhz_prover.clone();
    let mut best_effective_request_id = base.best_effective_prove_mhz_request_id;

    for hourly in hourly_summaries {
        if hourly.best_peak_prove_mhz > best_peak_mhz {
            best_peak_mhz = hourly.best_peak_prove_mhz;
            best_peak_prover = hourly.best_peak_prove_mhz_prover.clone();
            best_peak_request_id = hourly.best_peak_prove_mhz_request_id;
        }
        if hourly.best_effective_prove_mhz > best_effective_mhz {
            best_effective_mhz = hourly.best_effective_prove_mhz;
            best_effective_prover = hourly.best_effective_prove_mhz_prover.clone();
            best_effective_request_id = hourly.best_effective_prove_mhz_request_id;
        }
    }

    base.best_peak_prove_mhz = best_peak_mhz;
    base.best_peak_prove_mhz_prover = best_peak_prover;
    base.best_peak_prove_mhz_request_id = best_peak_request_id;
    base.best_effective_prove_mhz = best_effective_mhz;
    base.best_effective_prove_mhz_prover = best_effective_prover;
    base.best_effective_prove_mhz_request_id = best_effective_request_id;

    // Recalculate locked_orders_fulfillment_rate
    let total_locked_outcomes = base.total_locked_and_fulfilled + base.total_locked_and_expired;
    base.locked_orders_fulfillment_rate = if total_locked_outcomes > 0 {
        (base.total_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
    } else {
        0.0
    };
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(crate) async fn aggregate_all_time_market_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        // Get current time from the block timestamp
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating all-time market data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

        // Calculate hours ago based on configured recompute window (same as hourly aggregation)
        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);

        // Truncate to hour boundaries using DRY helper functions
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_all_time_market_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_all_time_market_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (hour starts for all-time aggregation)
        let expected_from = get_hour_start(from_time);
        let expected_to = get_hour_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow::anyhow!(
                "Time boundaries must be aligned to hour starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        tracing::debug!(
            "Aggregating all-time summaries for hours from {} to {}",
            from_time,
            to_time
        );

        // Query all hourly summaries from from_time to to_time (inclusive)
        let hourly_summaries =
            self.db.get_hourly_market_summaries_by_range(from_time, to_time + 1).await?;

        tracing::debug!(
            "Found {} hourly summaries to aggregate from {} to {}",
            hourly_summaries.len(),
            from_time,
            to_time
        );

        // Check if we have a previous all-time summary and if there's a gap
        // If so, extend from_time to cover the gap so the main loop will backfill it
        let latest_all_time = self.db.get_latest_all_time_market_summary().await?;
        let actual_start_hour = if let Some(latest) = &latest_all_time {
            let next_expected_hour = latest.period_timestamp + SECONDS_PER_HOUR;
            if next_expected_hour < from_time {
                let gap_hours = (from_time - latest.period_timestamp) / SECONDS_PER_HOUR;
                tracing::warn!(
                    "Detected gap of {} hours in all-time summaries (latest: {}, recompute window start: {}). Extending processing range to backfill.",
                    gap_hours, latest.period_timestamp, from_time
                );
                next_expected_hour
            } else {
                from_time
            }
        } else {
            from_time
        };

        // Get the all-time aggregate for the hour just before actual_start_hour
        // If it doesn't exist, initialize with zeros (first run)
        let base_timestamp = actual_start_hour.saturating_sub(SECONDS_PER_HOUR);

        let mut cumulative_summary =
            match self.db.get_all_time_market_summary_by_timestamp(base_timestamp).await? {
                Some(prev) => {
                    tracing::debug!(
                        "Found existing all-time aggregate at timestamp {}",
                        base_timestamp
                    );
                    prev
                }
                None => {
                    // No previous aggregate exists - this is the first run, initialize with zeros
                    tracing::info!(
                    "No previous all-time aggregate found, initializing with zeros at timestamp {}",
                    base_timestamp
                );
                    AllTimeMarketSummary {
                        period_timestamp: base_timestamp,
                        total_fulfilled: 0,
                        unique_provers_locking_requests: 0,
                        unique_requesters_submitting_requests: 0,
                        total_fees_locked: U256::ZERO,
                        total_collateral_locked: U256::ZERO,
                        total_locked_and_expired_collateral: U256::ZERO,
                        total_requests_submitted: 0,
                        total_requests_submitted_onchain: 0,
                        total_requests_submitted_offchain: 0,
                        total_requests_locked: 0,
                        total_requests_slashed: 0,
                        total_expired: 0,
                        total_locked_and_expired: 0,
                        total_locked_and_fulfilled: 0,
                        total_secondary_fulfillments: 0,
                        locked_orders_fulfillment_rate: 0.0,
                        total_program_cycles: U256::ZERO,
                        total_cycles: U256::ZERO,
                        best_peak_prove_mhz: 0.0,
                        best_peak_prove_mhz_prover: None,
                        best_peak_prove_mhz_request_id: None,
                        best_effective_prove_mhz: 0.0,
                        best_effective_prove_mhz_prover: None,
                        best_effective_prove_mhz_request_id: None,
                    }
                }
            };

        // Iteratively build up all-time aggregates from actual_start_hour (which may be extended to cover gaps)
        // Process each hour in the range, building cumulative all-time aggregates
        // For each hour, we:
        // 1. Add that hour's data to our cumulative summary (if available)
        // 2. Update unique counts from the database
        // 3. Save the all-time aggregate for that hour (always, even if no activity)
        // Note: We process up to and including to_time (even if not finished),
        // matching the behavior of other aggregation functions
        for (hour_ts, _hour_end) in iter_hourly_periods(actual_start_hour, to_time) {
            // Find the hourly summary for this hour
            let hour_summary = hourly_summaries.iter().find(|s| s.period_timestamp == hour_ts);

            // If a hourly summary exists for this hour, add its data to the cumulative
            if let Some(summary) = hour_summary {
                // Add this hour's data to the cumulative summary
                sum_hourly_aggregates_into_base(
                    &mut cumulative_summary,
                    std::slice::from_ref(summary),
                );
            } else {
                // No activity this hour - cumulative values stay the same, but we still
                // need to save an all-time entry for this hour to maintain the cumulative chain
                tracing::warn!(
                    "No hourly summary found for hour {}, maintaining cumulative with no change",
                    hour_ts
                );
            }

            // Update period_timestamp to reflect this hour
            cumulative_summary.period_timestamp = hour_ts;

            // Query unique counts from DB for all data up to this hour
            cumulative_summary.unique_provers_locking_requests =
                self.db.get_all_time_unique_provers(hour_ts).await?;
            cumulative_summary.unique_requesters_submitting_requests =
                self.db.get_all_time_unique_requesters(hour_ts).await?;

            // ALWAYS save the all-time aggregate for this hour, even if there was no activity
            // This ensures the cumulative chain is never broken
            self.db.upsert_all_time_market_summary(cumulative_summary.clone()).await?;
        }

        tracing::info!("aggregate_all_time_market_data_from completed in {:?}", start.elapsed());
        Ok(())
    }
}
