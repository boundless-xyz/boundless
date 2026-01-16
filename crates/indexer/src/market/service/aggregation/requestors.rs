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
use crate::db::RequestorDb;
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
use alloy::primitives::Address;
use alloy::providers::Provider;
use anyhow::anyhow;
use futures_util::future::try_join_all;
use std::str::FromStr;

/// Number of requestors to process in parallel per chunk
const REQUESTOR_CHUNK_SIZE: usize = 5;

/// Helper function to chunk a vector into smaller chunks
fn chunk_requestors(requestors: &[Address], chunk_size: usize) -> Vec<Vec<Address>> {
    requestors.chunks(chunk_size).map(|chunk| chunk.to_vec()).collect()
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(crate) async fn aggregate_hourly_requestor_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating hourly requestor data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_hourly_requestor_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_hourly_requestor_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (hour starts for hourly aggregation)
        let expected_from = get_hour_start(from_time);
        let expected_to = get_hour_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow!(
                "Time boundaries must be aligned to hour starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        // To know which requestors to process, we need to get the active requestors in the period.
        // Active here not only means they submitted a request, but also if someone locked one of their requests,
        // or if someone fulfilled one of their requests, or if someone slashed one of their requests,
        // or if one of their requests expired, etc.
        let requestors = self
            .db
            .get_active_requestor_addresses_in_period(from_time, to_time + SECONDS_PER_HOUR)
            .await?;

        tracing::debug!(
            "Processing {} requestors in hour range {} to {}",
            requestors.len(),
            from_time,
            to_time
        );

        // Process requestors in chunks in parallel
        let chunks = chunk_requestors(&requestors, REQUESTOR_CHUNK_SIZE);
        for chunk in chunks {
            let requestor_futures: Vec<_> = chunk
                .into_iter()
                .map(|requestor| {
                    let service = self;
                    async move {
                        for (hour_ts, hour_end) in iter_hourly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_requestor_summary(hour_ts, hour_end, requestor)
                                .await?;
                            service.db.upsert_hourly_requestor_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            // Wait for all requestors in this chunk to complete
            try_join_all(requestor_futures).await?;
        }

        tracing::info!("aggregate_hourly_requestor_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_daily_requestor_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating daily requestor data for past {} days from block {} timestamp {}",
            DAILY_AGGREGATION_RECOMPUTE_DAYS,
            to_block,
            current_time
        );

        let current_day_start = get_day_start(current_time);
        let start_day = current_day_start
            .saturating_sub((DAILY_AGGREGATION_RECOMPUTE_DAYS - 1) * SECONDS_PER_DAY);

        self.aggregate_daily_requestor_data_from(start_day, current_day_start).await
    }

    pub(crate) async fn aggregate_daily_requestor_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (day starts for daily aggregation)
        let expected_from = get_day_start(from_time);
        let expected_to = get_day_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow!(
                "Time boundaries must be aligned to day starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        // To know which requestors to process, we need to get the active requestors in the period.
        // Active here not only means they submitted a request, but also if someone locked one of their requests,
        // or if someone fulfilled one of their requests, or if someone slashed one of their requests,
        // or if one of their requests expired, etc.
        let requestors = self
            .db
            .get_active_requestor_addresses_in_period(from_time, to_time + SECONDS_PER_DAY)
            .await?;

        tracing::debug!(
            "Processing {} requestors in day range {} to {}",
            requestors.len(),
            from_time,
            to_time
        );

        // Process requestors in chunks in parallel
        let chunks = chunk_requestors(&requestors, REQUESTOR_CHUNK_SIZE);
        for chunk in chunks {
            let requestor_futures: Vec<_> = chunk
                .into_iter()
                .map(|requestor| {
                    let service = self;
                    async move {
                        for (day_ts, day_end) in iter_daily_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_requestor_summary(day_ts, day_end, requestor)
                                .await?;
                            service.db.upsert_daily_requestor_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            // Wait for all requestors in this chunk to complete
            try_join_all(requestor_futures).await?;
        }

        tracing::info!("aggregate_daily_requestor_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_weekly_requestor_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating weekly requestor data for past {} weeks from block {} timestamp {}",
            WEEKLY_AGGREGATION_RECOMPUTE_WEEKS,
            to_block,
            current_time
        );

        let current_week_start = get_week_start(current_time);
        let start_week = current_week_start
            .saturating_sub((WEEKLY_AGGREGATION_RECOMPUTE_WEEKS - 1) * SECONDS_PER_WEEK);

        self.aggregate_weekly_requestor_data_from(start_week, current_week_start).await
    }

    pub(crate) async fn aggregate_weekly_requestor_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (week starts for weekly aggregation)
        let expected_from = get_week_start(from_time);
        let expected_to = get_week_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow!(
                "Time boundaries must be aligned to week starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        // To know which requestors to process, we need to get the active requestors in the period.
        // Active here not only means they submitted a request, but also if someone locked one of their requests,
        // or if someone fulfilled one of their requests, or if someone slashed one of their requests,
        // or if one of their requests expired, etc.
        let requestors = self
            .db
            .get_active_requestor_addresses_in_period(from_time, to_time + SECONDS_PER_WEEK)
            .await?;

        tracing::debug!(
            "Processing {} requestors in week range {} to {}",
            requestors.len(),
            from_time,
            to_time
        );

        // Process requestors in chunks in parallel
        let chunks = chunk_requestors(&requestors, REQUESTOR_CHUNK_SIZE);
        for chunk in chunks {
            let requestor_futures: Vec<_> = chunk
                .into_iter()
                .map(|requestor| {
                    let service = self;
                    async move {
                        for (week_ts, week_end) in iter_weekly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_requestor_summary(week_ts, week_end, requestor)
                                .await?;
                            service.db.upsert_weekly_requestor_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            // Wait for all requestors in this chunk to complete
            try_join_all(requestor_futures).await?;
        }

        tracing::info!("aggregate_weekly_requestor_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) async fn aggregate_monthly_requestor_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating monthly requestor data for past {} months from block {} timestamp {}",
            MONTHLY_AGGREGATION_RECOMPUTE_MONTHS,
            to_block,
            current_time
        );

        let current_month_start = get_month_start(current_time);

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

        self.aggregate_monthly_requestor_data_from(start_month, current_month_start).await
    }

    pub(crate) async fn aggregate_monthly_requestor_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (month starts for monthly aggregation)
        let expected_from = get_month_start(from_time);
        let expected_to = get_month_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow!(
                "Time boundaries must be aligned to month starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        // Compute the next month after to_time for getting active requestors
        use chrono::{Datelike, TimeZone, Utc};
        let next_month = {
            let dt = Utc.timestamp_opt(to_time as i64, 0).unwrap();
            let next = if dt.month() == 12 {
                Utc.with_ymd_and_hms(dt.year() + 1, 1, 1, 0, 0, 0).unwrap()
            } else {
                Utc.with_ymd_and_hms(dt.year(), dt.month() + 1, 1, 0, 0, 0).unwrap()
            };
            next.timestamp() as u64
        };

        // To know which requestors to process, we need to get the active requestors in the period.
        // Active here not only means they submitted a request, but also if someone locked one of their requests,
        // or if someone fulfilled one of their requests, or if someone slashed one of their requests,
        // or if one of their requests expired, etc.
        let requestors =
            self.db.get_active_requestor_addresses_in_period(from_time, next_month).await?;

        tracing::debug!(
            "Processing {} requestors in month range {} to {}",
            requestors.len(),
            from_time,
            to_time
        );

        // Process requestors in chunks in parallel
        let chunks = chunk_requestors(&requestors, REQUESTOR_CHUNK_SIZE);
        for chunk in chunks {
            let requestor_futures: Vec<_> = chunk
                .into_iter()
                .map(|requestor| {
                    let service = self;
                    async move {
                        for (month_ts, month_end) in iter_monthly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_requestor_summary(month_ts, month_end, requestor)
                                .await?;
                            service.db.upsert_monthly_requestor_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            // Wait for all requestors in this chunk to complete
            try_join_all(requestor_futures).await?;
        }

        tracing::info!("aggregate_monthly_requestor_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_all_time_requestor_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating all-time requestor data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_all_time_requestor_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_all_time_requestor_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Verify boundaries are properly aligned (hour starts for all-time aggregation)
        let expected_from = get_hour_start(from_time);
        let expected_to = get_hour_start(to_time);
        if from_time != expected_from || to_time != expected_to {
            return Err(ServiceError::Error(anyhow!(
                "Time boundaries must be aligned to hour starts: from_time {} should be {}, to_time {} should be {}",
                from_time,
                expected_from,
                to_time,
                expected_to
            )));
        }

        let requestors = self
            .db
            .get_active_requestor_addresses_in_period(from_time, to_time + SECONDS_PER_HOUR)
            .await?;

        tracing::debug!(
            "Aggregating all-time summaries for {} requestors from {} to {}",
            requestors.len(),
            from_time,
            to_time
        );

        // Process requestors in chunks in parallel
        let chunks = chunk_requestors(&requestors, REQUESTOR_CHUNK_SIZE);
        for chunk in chunks {
            let requestor_futures: Vec<_> = chunk
                .into_iter()
                .map(|requestor| {
                    let service = self;
                    async move {
                        // Check if we have a previous all-time summary for this requestor and if there's a gap
                        // If so, extend from_time to cover the gap
                        let latest_all_time = service.db.get_latest_all_time_requestor_summary(requestor).await?;
                        let actual_start_hour = if let Some(latest) = &latest_all_time {
                            let next_expected_hour = latest.period_timestamp + SECONDS_PER_HOUR;
                            if next_expected_hour < from_time {
                                let gap_hours = (from_time - latest.period_timestamp) / SECONDS_PER_HOUR;
                                tracing::info!(
                                    "Detected gap of {} hours in all-time requestor summaries for {:?} (latest: {}, recompute window start: {}). Extending processing range to backfill.",
                                    gap_hours, requestor, latest.period_timestamp, from_time
                                );
                                next_expected_hour
                            } else {
                                from_time
                            }
                        } else {
                            from_time
                        };
                        // Get all hourly summaries for this requestor
                        let hourly_summaries = service.db
                            .get_hourly_requestor_summaries_by_range(requestor, actual_start_hour, to_time + 1)
                            .await?;

                        if hourly_summaries.is_empty() && latest_all_time.is_none() {
                            // No historical data and no current data - skip this requestor
                            return Ok::<(), ServiceError>(());
                        }

                        let base_timestamp = actual_start_hour.saturating_sub(SECONDS_PER_HOUR);

                        let mut cumulative_summary = match service.db
                            .get_all_time_requestor_summary_by_timestamp(requestor, base_timestamp)
                            .await?
                        {
                            Some(prev) => {
                                tracing::debug!("Found existing all-time requestor aggregate for {:?} at timestamp {}", requestor, base_timestamp);
                                prev
                            }
                            None => {
                                tracing::debug!("No previous all-time requestor aggregate for {:?}, initializing with zeros", requestor);
                                crate::db::market::AllTimeRequestorSummary {
                                    period_timestamp: base_timestamp,
                                    requestor_address: requestor,
                                    total_fulfilled: 0,
                                    unique_provers_locking_requests: 0,
                                    total_fees_locked: alloy::primitives::U256::ZERO,
                                    total_collateral_locked: alloy::primitives::U256::ZERO,
                                    total_locked_and_expired_collateral: alloy::primitives::U256::ZERO,
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
                                    locked_orders_fulfillment_rate_adjusted: 0.0,
                                    total_program_cycles: alloy::primitives::U256::ZERO,
                                    total_cycles: alloy::primitives::U256::ZERO,
                                    best_peak_prove_mhz: 0.0,
                                    best_peak_prove_mhz_prover: None,
                                    best_peak_prove_mhz_request_id: None,
                                    best_effective_prove_mhz: 0.0,
                                    best_effective_prove_mhz_prover: None,
                                    best_effective_prove_mhz_request_id: None,
                                }
                            }
                        };

                        for (hour_ts, _hour_end) in iter_hourly_periods(actual_start_hour, to_time) {
                            let hour_summary = hourly_summaries.iter().find(|s| s.period_timestamp == hour_ts);

                            if let Some(summary) = hour_summary {
                                // Add this hour's data
                                cumulative_summary.total_fulfilled += summary.total_fulfilled;
                                cumulative_summary.total_requests_submitted += summary.total_requests_submitted;
                                cumulative_summary.total_requests_submitted_onchain += summary.total_requests_submitted_onchain;
                                cumulative_summary.total_requests_submitted_offchain += summary.total_requests_submitted_offchain;
                                cumulative_summary.total_requests_locked += summary.total_requests_locked;
                                cumulative_summary.total_requests_slashed += summary.total_requests_slashed;
                                cumulative_summary.total_expired += summary.total_expired;
                                cumulative_summary.total_locked_and_expired += summary.total_locked_and_expired;
                                cumulative_summary.total_locked_and_fulfilled += summary.total_locked_and_fulfilled;
                                cumulative_summary.total_secondary_fulfillments += summary.total_secondary_fulfillments;
                                cumulative_summary.total_program_cycles += summary.total_program_cycles;
                                cumulative_summary.total_cycles += summary.total_cycles;
                                cumulative_summary.total_fees_locked += summary.total_fees_locked;
                                cumulative_summary.total_collateral_locked += summary.total_collateral_locked;
                                cumulative_summary.total_locked_and_expired_collateral += summary.total_locked_and_expired_collateral;

                                // Update best metrics
                                if summary.best_peak_prove_mhz > cumulative_summary.best_peak_prove_mhz {
                                    cumulative_summary.best_peak_prove_mhz = summary.best_peak_prove_mhz;
                                    cumulative_summary.best_peak_prove_mhz_prover = summary.best_peak_prove_mhz_prover.clone();
                                    cumulative_summary.best_peak_prove_mhz_request_id = summary.best_peak_prove_mhz_request_id;
                                }
                                if summary.best_effective_prove_mhz > cumulative_summary.best_effective_prove_mhz {
                                    cumulative_summary.best_effective_prove_mhz = summary.best_effective_prove_mhz;
                                    cumulative_summary.best_effective_prove_mhz_prover = summary.best_effective_prove_mhz_prover.clone();
                                    cumulative_summary.best_effective_prove_mhz_request_id = summary.best_effective_prove_mhz_request_id;
                                }
                            } else {
                                // No activity this hour - cumulative stays the same, but we still save an entry
                                tracing::debug!("No hourly requestor summary found for hour {}, maintaining cumulative", hour_ts);
                            }

                            cumulative_summary.period_timestamp = hour_ts;

                            // Query unique provers for this requestor up to this hour
                            cumulative_summary.unique_provers_locking_requests = service.db
                                .get_all_time_requestor_unique_provers(hour_ts + 1, requestor)
                                .await?;

                            // Recalculate fulfillment rate
                            let total_locked_outcomes = cumulative_summary.total_locked_and_fulfilled + cumulative_summary.total_locked_and_expired;
                            cumulative_summary.locked_orders_fulfillment_rate = if total_locked_outcomes > 0 {
                                (cumulative_summary.total_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
                            } else {
                                0.0
                            };

                            // Query adjusted counts and recalculate adjusted fulfillment rate
                            let adjusted_fulfilled = service.db
                                .get_all_time_requestor_locked_and_fulfilled_count_adjusted(hour_ts + 1, requestor)
                                .await?;
                            let adjusted_expired = service.db
                                .get_all_time_requestor_locked_and_expired_count_adjusted(hour_ts + 1, requestor)
                                .await?;
                            let total_locked_outcomes_adjusted = adjusted_fulfilled + adjusted_expired;
                            cumulative_summary.locked_orders_fulfillment_rate_adjusted = if total_locked_outcomes_adjusted > 0 {
                                (adjusted_fulfilled as f32 / total_locked_outcomes_adjusted as f32) * 100.0
                            } else {
                                0.0
                            };

                            // ALWAYS save the all-time requestor aggregate, even if there was no activity
                            service.db.upsert_all_time_requestor_summary(cumulative_summary.clone()).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            // Wait for all requestors in this chunk to complete
            try_join_all(requestor_futures).await?;
        }

        tracing::info!("aggregate_all_time_requestor_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub async fn compute_period_requestor_summary(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: alloy::primitives::Address,
    ) -> Result<crate::db::market::PeriodRequestorSummary, ServiceError> {
        use crate::db::market::PeriodRequestorSummary;

        // Execute all database queries in parallel
        let (
            total_fulfilled,
            unique_provers,
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            total_secondary_fulfillments,
            total_locked_and_fulfilled_adjusted,
            total_locked_and_expired_adjusted,
            locks,
            all_lock_collaterals,
            locked_and_expired_collaterals,
            total_program_cycles,
            total_cycles,
        ) = tokio::join!(
            self.db.get_period_requestor_fulfilled_count(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_unique_provers(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_requests_submitted(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_requests_submitted_onchain(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_requests_locked(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_requests_slashed(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_expired_count(period_start, period_end, requestor_address),
            self.db.get_period_requestor_locked_and_expired_count(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_locked_and_fulfilled_count(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_secondary_fulfillments_count(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_locked_and_fulfilled_count_adjusted(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_locked_and_expired_count_adjusted(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_lock_pricing_data(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_all_lock_collateral(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_locked_and_expired_collateral(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_program_cycles(
                period_start,
                period_end,
                requestor_address
            ),
            self.db.get_period_requestor_total_cycles(period_start, period_end, requestor_address),
        );

        let total_fulfilled = total_fulfilled?;
        let unique_provers = unique_provers?;
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
        let total_locked_and_fulfilled_adjusted = total_locked_and_fulfilled_adjusted?;
        let total_locked_and_expired_adjusted = total_locked_and_expired_adjusted?;
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

        let locked_orders_fulfillment_rate_adjusted = {
            let total_locked_outcomes_adjusted =
                total_locked_and_fulfilled_adjusted + total_locked_and_expired_adjusted;
            if total_locked_outcomes_adjusted > 0 {
                (total_locked_and_fulfilled_adjusted as f32 / total_locked_outcomes_adjusted as f32)
                    * 100.0
            } else {
                0.0
            }
        };

        let mut total_fees = alloy::primitives::U256::ZERO;
        let mut prices_per_cycle: Vec<alloy::primitives::Uint<256, 4>> = Vec::new();

        for lock in locks {
            let price = if let Some(lock_price_str) = &lock.lock_price {
                alloy::primitives::U256::from_str(lock_price_str).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse lock_price: {}", e))
                })?
            } else {
                let min_price =
                    alloy::primitives::U256::from_str(&lock.min_price).map_err(|e| {
                        ServiceError::Error(anyhow!("Failed to parse min_price: {}", e))
                    })?;
                let max_price =
                    alloy::primitives::U256::from_str(&lock.max_price).map_err(|e| {
                        ServiceError::Error(anyhow!("Failed to parse max_price: {}", e))
                    })?;

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

            if let Some(price_per_cycle_str) = &lock.lock_price_per_cycle {
                if let Ok(price_per_cycle) = alloy::primitives::U256::from_str(price_per_cycle_str)
                {
                    prices_per_cycle.push(price_per_cycle);
                }
            }
        }

        let mut total_collateral = alloy::primitives::U256::ZERO;
        for collateral_str in all_lock_collaterals {
            let lock_collateral =
                alloy::primitives::U256::from_str(&collateral_str).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e))
                })?;
            total_collateral += lock_collateral;
        }

        let mut total_locked_and_expired_collateral = alloy::primitives::U256::ZERO;
        for collateral_str in locked_and_expired_collaterals {
            let lock_collateral =
                alloy::primitives::U256::from_str(&collateral_str).map_err(|e| {
                    ServiceError::Error(anyhow!("Failed to parse lock_collateral: {}", e))
                })?;
            total_locked_and_expired_collateral += lock_collateral;
        }

        let percentiles = if !prices_per_cycle.is_empty() {
            let mut sorted_prices = prices_per_cycle;
            compute_percentiles(&mut sorted_prices, &[10, 25, 50, 75, 90, 95, 99])
        } else {
            vec![alloy::primitives::U256::ZERO; 7]
        };

        let best_peak_prove_mhz = 0.0;
        let best_peak_prove_mhz_prover = None;
        let best_peak_prove_mhz_request_id = None;
        let best_effective_prove_mhz = 0.0;
        let best_effective_prove_mhz_prover = None;
        let best_effective_prove_mhz_request_id = None;

        Ok(PeriodRequestorSummary {
            period_timestamp: period_start,
            requestor_address,
            total_fulfilled,
            unique_provers_locking_requests: unique_provers,
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
            locked_orders_fulfillment_rate_adjusted,
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
