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
use crate::db::ProversDb;
use crate::market::{
    pricing::compute_percentiles,
    service::DbResultExt,
    time_boundaries::{
        get_day_start, get_hour_start, get_month_start, get_week_start, iter_daily_periods,
        iter_hourly_periods, iter_monthly_periods, iter_weekly_periods,
    },
    ServiceError,
};
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use anyhow::anyhow;
use futures_util::future::try_join_all;
use sqlx::Row;
use std::str::FromStr;

const PROVER_CHUNK_SIZE: usize = 5;

fn chunk_provers(provers: &[Address], chunk_size: usize) -> Vec<Vec<Address>> {
    provers.chunks(chunk_size).map(|chunk| chunk.to_vec()).collect()
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(crate) async fn aggregate_hourly_prover_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating hourly prover data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_hourly_prover_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_hourly_prover_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

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

        let provers = self
            .db
            .get_active_prover_addresses_in_period(from_time, to_time + SECONDS_PER_HOUR)
            .await?;

        tracing::debug!(
            "Processing {} provers in hour range {} to {}",
            provers.len(),
            from_time,
            to_time
        );

        let chunks = chunk_provers(&provers, PROVER_CHUNK_SIZE);
        for chunk in chunks {
            let prover_futures: Vec<_> = chunk
                .into_iter()
                .map(|prover| {
                    let service = self;
                    async move {
                        for (hour_ts, hour_end) in iter_hourly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_prover_summary(hour_ts, hour_end, prover)
                                .await?;
                            service.db.upsert_hourly_prover_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            try_join_all(prover_futures).await?;
        }

        tracing::info!("aggregate_hourly_prover_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_daily_prover_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating daily prover data for past {} days from block {} timestamp {}",
            DAILY_AGGREGATION_RECOMPUTE_DAYS,
            to_block,
            current_time
        );

        let current_day_start = get_day_start(current_time);
        let start_day = current_day_start
            .saturating_sub((DAILY_AGGREGATION_RECOMPUTE_DAYS - 1) * SECONDS_PER_DAY);

        self.aggregate_daily_prover_data_from(start_day, current_day_start).await
    }

    pub(crate) async fn aggregate_daily_prover_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

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

        let provers = self
            .db
            .get_active_prover_addresses_in_period(from_time, to_time + SECONDS_PER_DAY)
            .await?;

        tracing::debug!(
            "Processing {} provers in day range {} to {}",
            provers.len(),
            from_time,
            to_time
        );

        let chunks = chunk_provers(&provers, PROVER_CHUNK_SIZE);
        for chunk in chunks {
            let prover_futures: Vec<_> = chunk
                .into_iter()
                .map(|prover| {
                    let service = self;
                    async move {
                        for (day_ts, day_end) in iter_daily_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_prover_summary(day_ts, day_end, prover)
                                .await?;
                            service.db.upsert_daily_prover_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            try_join_all(prover_futures).await?;
        }

        tracing::info!("aggregate_daily_prover_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_weekly_prover_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating weekly prover data for past {} weeks from block {} timestamp {}",
            WEEKLY_AGGREGATION_RECOMPUTE_WEEKS,
            to_block,
            current_time
        );

        let current_week_start = get_week_start(current_time);
        let start_week = current_week_start
            .saturating_sub((WEEKLY_AGGREGATION_RECOMPUTE_WEEKS - 1) * SECONDS_PER_WEEK);

        self.aggregate_weekly_prover_data_from(start_week, current_week_start).await
    }

    pub(crate) async fn aggregate_weekly_prover_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

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

        let provers = self
            .db
            .get_active_prover_addresses_in_period(from_time, to_time + SECONDS_PER_WEEK)
            .await?;

        tracing::debug!(
            "Processing {} provers in week range {} to {}",
            provers.len(),
            from_time,
            to_time
        );

        let chunks = chunk_provers(&provers, PROVER_CHUNK_SIZE);
        for chunk in chunks {
            let prover_futures: Vec<_> = chunk
                .into_iter()
                .map(|prover| {
                    let service = self;
                    async move {
                        for (week_ts, week_end) in iter_weekly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_prover_summary(week_ts, week_end, prover)
                                .await?;
                            service.db.upsert_weekly_prover_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            try_join_all(prover_futures).await?;
        }

        tracing::info!("aggregate_weekly_prover_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) async fn aggregate_monthly_prover_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating monthly prover data for past {} months from block {} timestamp {}",
            MONTHLY_AGGREGATION_RECOMPUTE_MONTHS,
            to_block,
            current_time
        );

        let current_month_start = get_month_start(current_time);
        let start_month = current_month_start
            .saturating_sub((MONTHLY_AGGREGATION_RECOMPUTE_MONTHS - 1) * 30 * SECONDS_PER_DAY);

        self.aggregate_monthly_prover_data_from(start_month, current_month_start).await
    }

    pub(crate) async fn aggregate_monthly_prover_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

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

        let provers = self
            .db
            .get_active_prover_addresses_in_period(from_time, to_time + 31 * SECONDS_PER_DAY)
            .await?;

        tracing::debug!(
            "Processing {} provers in month range {} to {}",
            provers.len(),
            from_time,
            to_time
        );

        let chunks = chunk_provers(&provers, PROVER_CHUNK_SIZE);
        for chunk in chunks {
            let prover_futures: Vec<_> = chunk
                .into_iter()
                .map(|prover| {
                    let service = self;
                    async move {
                        for (month_ts, month_end) in iter_monthly_periods(from_time, to_time) {
                            let summary = service
                                .compute_period_prover_summary(month_ts, month_end, prover)
                                .await?;
                            service.db.upsert_monthly_prover_summary(summary).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            try_join_all(prover_futures).await?;
        }

        tracing::info!("aggregate_monthly_prover_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub(crate) async fn aggregate_all_time_prover_data(
        &self,
        to_block: u64,
    ) -> Result<(), ServiceError> {
        let current_time = self.block_timestamp(to_block).await?;

        tracing::debug!(
            "Aggregating all-time prover data for past {} hours from block {} timestamp {}",
            HOURLY_AGGREGATION_RECOMPUTE_HOURS,
            to_block,
            current_time
        );

        let hours_ago = current_time - (HOURLY_AGGREGATION_RECOMPUTE_HOURS * SECONDS_PER_HOUR);
        let start_hour = get_hour_start(hours_ago);
        let current_hour = get_hour_start(current_time);

        self.aggregate_all_time_prover_data_from(start_hour, current_hour).await
    }

    pub(crate) async fn aggregate_all_time_prover_data_from(
        &self,
        from_time: u64,
        to_time: u64,
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

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

        let provers = self
            .db
            .get_active_prover_addresses_in_period(from_time, to_time + SECONDS_PER_HOUR)
            .await?;

        tracing::debug!(
            "Processing {} provers for all-time aggregation in hour range {} to {}",
            provers.len(),
            from_time,
            to_time
        );

        let chunks = chunk_provers(&provers, PROVER_CHUNK_SIZE);
        for chunk in chunks {
            let prover_futures: Vec<_> = chunk
                .into_iter()
                .map(|prover| {
                    let service = self;
                    async move {
                        let latest_all_time = service.db.get_latest_all_time_prover_summary(prover).await?;
                        let actual_start_hour = if let Some(latest) = &latest_all_time {
                            let next_expected_hour = latest.period_timestamp + SECONDS_PER_HOUR;
                            if next_expected_hour < from_time {
                                let gap_hours = (from_time - latest.period_timestamp) / SECONDS_PER_HOUR;
                                tracing::info!(
                                    "Detected gap of {} hours in all-time prover summaries for {:?} (latest: {}, recompute window start: {}). Extending processing range to backfill.",
                                    gap_hours, prover, latest.period_timestamp, from_time
                                );
                                next_expected_hour
                            } else {
                                from_time
                            }
                        } else {
                            from_time
                        };
                        let hourly_summaries = service.db
                            .get_hourly_prover_summaries_by_range(prover, actual_start_hour, to_time + 1)
                            .await?;

                        if hourly_summaries.is_empty() &&
                            latest_all_time.is_none() {
                            tracing::debug!("No hourly prover summaries found for {:?} and no existing all-time summary, skipping", prover);
                            return Ok::<(), ServiceError>(());
                        }

                        let mut cumulative_summary = if let Some(latest) = latest_all_time {
                            latest.clone()
                        } else {
                            tracing::debug!("No previous all-time prover aggregate for {:?}, initializing with zeros", prover);
                            crate::db::market::AllTimeProverSummary {
                                period_timestamp: actual_start_hour,
                                prover_address: prover,
                                total_requests_locked: 0,
                                total_requests_fulfilled: 0,
                                total_unique_requestors: 0,
                                total_fees_earned: alloy::primitives::U256::ZERO,
                                total_collateral_locked: alloy::primitives::U256::ZERO,
                                total_collateral_slashed: alloy::primitives::U256::ZERO,
                                total_collateral_earned: alloy::primitives::U256::ZERO,
                                total_requests_locked_and_expired: 0,
                                total_requests_locked_and_fulfilled: 0,
                                locked_orders_fulfillment_rate: 0.0,
                                total_program_cycles: alloy::primitives::U256::ZERO,
                                total_cycles: alloy::primitives::U256::ZERO,
                                best_peak_prove_mhz: 0.0,
                                best_peak_prove_mhz_request_id: None,
                                best_effective_prove_mhz: 0.0,
                                best_effective_prove_mhz_request_id: None,
                            }
                        };

                        for (hour_ts, _hour_end) in iter_hourly_periods(actual_start_hour, to_time) {
                            let hour_summary = hourly_summaries.iter().find(|s| s.period_timestamp == hour_ts);

                            if let Some(summary) = hour_summary {
                                cumulative_summary.total_requests_locked += summary.total_requests_locked;
                                cumulative_summary.total_requests_fulfilled += summary.total_requests_fulfilled;
                                cumulative_summary.total_fees_earned += summary.total_fees_earned;
                                cumulative_summary.total_collateral_locked += summary.total_collateral_locked;
                                cumulative_summary.total_collateral_slashed += summary.total_collateral_slashed;
                                cumulative_summary.total_collateral_earned += summary.total_collateral_earned;
                                cumulative_summary.total_requests_locked_and_expired += summary.total_requests_locked_and_expired;
                                cumulative_summary.total_requests_locked_and_fulfilled += summary.total_requests_locked_and_fulfilled;
                                cumulative_summary.total_program_cycles += summary.total_program_cycles;
                                cumulative_summary.total_cycles += summary.total_cycles;

                                if summary.best_peak_prove_mhz > cumulative_summary.best_peak_prove_mhz {
                                    cumulative_summary.best_peak_prove_mhz = summary.best_peak_prove_mhz;
                                    cumulative_summary.best_peak_prove_mhz_request_id = summary.best_peak_prove_mhz_request_id;
                                }
                                if summary.best_effective_prove_mhz > cumulative_summary.best_effective_prove_mhz {
                                    cumulative_summary.best_effective_prove_mhz = summary.best_effective_prove_mhz;
                                    cumulative_summary.best_effective_prove_mhz_request_id = summary.best_effective_prove_mhz_request_id;
                                }
                            } else {
                                tracing::debug!("No hourly prover summary found for hour {}, maintaining cumulative", hour_ts);
                            }

                            cumulative_summary.period_timestamp = hour_ts;

                            cumulative_summary.total_unique_requestors = service.db
                                .get_all_time_prover_unique_requestors(hour_ts + 1, prover)
                                .await?;

                            let total_locked_outcomes = cumulative_summary.total_requests_locked_and_fulfilled + cumulative_summary.total_requests_locked_and_expired;
                            cumulative_summary.locked_orders_fulfillment_rate = if total_locked_outcomes > 0 {
                                (cumulative_summary.total_requests_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
                            } else {
                                0.0
                            };

                            service.db.upsert_all_time_prover_summary(cumulative_summary.clone()).await?;
                        }
                        Ok::<(), ServiceError>(())
                    }
                })
                .collect();

            try_join_all(prover_futures).await?;
        }

        tracing::info!("aggregate_all_time_prover_data_from completed in {:?}", start.elapsed());
        Ok(())
    }

    pub async fn compute_period_prover_summary(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<crate::db::market::PeriodProverSummary, ServiceError> {
        use crate::db::market::PeriodProverSummary;

        let (
            total_requests_locked,
            total_requests_fulfilled,
            total_unique_requestors,
            total_fees_earned,
            total_collateral_locked,
            total_collateral_slashed,
            total_collateral_earned,
            total_requests_locked_and_expired,
            total_requests_locked_and_fulfilled,
            locks,
            total_program_cycles,
            total_cycles,
        ) = tokio::join!(
            async {
                self.db
                    .get_period_prover_requests_locked_count(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_requests_locked_count")
            },
            async {
                self.db
                    .get_period_prover_requests_fulfilled_count(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_requests_fulfilled_count")
            },
            async {
                self.db
                    .get_period_prover_unique_requestors(period_start, period_end, prover_address)
                    .await
                    .with_db_context("get_period_prover_unique_requestors")
            },
            async {
                self.db
                    .get_period_prover_total_fees_earned(period_start, period_end, prover_address)
                    .await
                    .with_db_context("get_period_prover_total_fees_earned")
            },
            async {
                self.db
                    .get_period_prover_total_collateral_locked(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_total_collateral_locked")
            },
            async {
                self.db
                    .get_period_prover_total_collateral_slashed(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_total_collateral_slashed")
            },
            async {
                self.db
                    .get_period_prover_total_collateral_earned(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_total_collateral_earned")
            },
            async {
                self.db
                    .get_period_prover_locked_and_expired_count(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_locked_and_expired_count")
            },
            async {
                self.db
                    .get_period_prover_locked_and_fulfilled_count(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_locked_and_fulfilled_count")
            },
            async {
                self.db
                    .get_period_prover_lock_pricing_data(period_start, period_end, prover_address)
                    .await
                    .with_db_context("get_period_prover_lock_pricing_data")
            },
            async {
                self.db
                    .get_period_prover_total_program_cycles(
                        period_start,
                        period_end,
                        prover_address,
                    )
                    .await
                    .with_db_context("get_period_prover_total_program_cycles")
            },
            async {
                self.db
                    .get_period_prover_total_cycles(period_start, period_end, prover_address)
                    .await
                    .with_db_context("get_period_prover_total_cycles")
            },
        );

        let total_requests_locked = total_requests_locked?;
        let total_requests_fulfilled = total_requests_fulfilled?;
        let total_unique_requestors = total_unique_requestors?;
        let total_fees_earned = total_fees_earned?;
        let total_collateral_locked = total_collateral_locked?;
        let total_collateral_slashed = total_collateral_slashed?;
        let total_collateral_earned = total_collateral_earned?;
        let total_requests_locked_and_expired = total_requests_locked_and_expired?;
        let total_requests_locked_and_fulfilled = total_requests_locked_and_fulfilled?;
        let locks = locks?;
        let total_program_cycles = total_program_cycles?;
        let total_cycles = total_cycles?;

        let locked_orders_fulfillment_rate = {
            let total_locked_outcomes =
                total_requests_locked_and_fulfilled + total_requests_locked_and_expired;
            if total_locked_outcomes > 0 {
                (total_requests_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
            } else {
                0.0
            }
        };

        let mut prices_per_cycle: Vec<alloy::primitives::Uint<256, 4>> = Vec::new();

        for lock in locks {
            if let Some(price_per_cycle_str) = &lock.lock_price_per_cycle {
                if let Ok(price_per_cycle) = alloy::primitives::U256::from_str(price_per_cycle_str)
                {
                    prices_per_cycle.push(price_per_cycle);
                }
            }
        }

        let percentiles = if !prices_per_cycle.is_empty() {
            let mut sorted_prices = prices_per_cycle;
            compute_percentiles(&mut sorted_prices, &[10, 25, 50, 75, 90, 95, 99])
        } else {
            vec![alloy::primitives::U256::ZERO; 7]
        };

        // TODO: Remove these fields
        let best_peak_prove_mhz = 0.0;
        let best_peak_prove_mhz_request_id = None;

        let mut best_effective_prove_mhz = 0.0;
        let mut best_effective_prove_mhz_request_id = None;

        let fulfilled_rows = sqlx::query(
            "SELECT prover_effective_prove_mhz, request_id FROM request_status
             WHERE fulfill_prover_address = $1
             AND request_status = 'fulfilled'
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $2 AND fulfilled_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.db.pool())
        .await
        .map_err(|e| ServiceError::DatabaseError(crate::db::DbError::SqlErr(e)))?;

        for row in fulfilled_rows {
            let effective_mhz: Option<f64> =
                row.try_get::<Option<f64>, _>("prover_effective_prove_mhz").ok().flatten();
            let request_id_str: Option<String> =
                row.try_get::<Option<String>, _>("request_id").ok().flatten();

            if let Some(effective) = effective_mhz {
                if effective > best_effective_prove_mhz {
                    best_effective_prove_mhz = effective;
                    if let Some(rid) = &request_id_str {
                        best_effective_prove_mhz_request_id = U256::from_str(rid).ok();
                    }
                }
            }
        }

        Ok(PeriodProverSummary {
            period_timestamp: period_start,
            prover_address,
            total_requests_locked,
            total_requests_fulfilled,
            total_unique_requestors,
            total_fees_earned,
            total_collateral_locked,
            total_collateral_slashed,
            total_collateral_earned,
            total_requests_locked_and_expired,
            total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate,
            p10_lock_price_per_cycle: percentiles[0],
            p25_lock_price_per_cycle: percentiles[1],
            p50_lock_price_per_cycle: percentiles[2],
            p75_lock_price_per_cycle: percentiles[3],
            p90_lock_price_per_cycle: percentiles[4],
            p95_lock_price_per_cycle: percentiles[5],
            p99_lock_price_per_cycle: percentiles[6],
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz,
            best_effective_prove_mhz_request_id,
        })
    }
}
