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

use crate::db::market::IndexerDb;
use crate::db::RequestorDb;
use crate::market::{
    time_boundaries::{
        iter_daily_periods, iter_hourly_periods, iter_monthly_periods, iter_weekly_periods,
    },
    IndexerService, ServiceError,
};
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::B256;
use alloy::providers::Provider;
use std::collections::HashSet;

const DIGEST_BATCH_SIZE: i64 = 5000;
const STATUS_BATCH_SIZE: usize = 1000;

#[derive(Debug, Clone, Copy)]
pub enum BackfillMode {
    StatusesAndAggregates,
    Aggregates,
}

pub struct BackfillService<P, ANP> {
    pub indexer: IndexerService<P, ANP>,
    pub mode: BackfillMode,
    pub end_block: u64,
}

impl<P, ANP> BackfillService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub fn new(indexer: IndexerService<P, ANP>, mode: BackfillMode, end_block: u64) -> Self {
        Self { indexer, mode, end_block }
    }

    pub async fn run(&mut self) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "Starting backfill with mode: {:?}, end_block: {}",
            self.mode,
            self.end_block
        );

        match self.mode {
            BackfillMode::StatusesAndAggregates => {
                self.backfill_statuses().await?;
                self.backfill_aggregates().await?;
            }
            BackfillMode::Aggregates => {
                self.backfill_aggregates().await?;
            }
        }

        tracing::info!("Backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_statuses(&mut self) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting status backfill...");

        let current_timestamp = self.indexer.block_timestamp(self.end_block).await?;
        tracing::info!(
            "Using end block {} with timestamp {} as 'current time' for status computation",
            self.end_block,
            current_timestamp
        );

        let mut cursor: Option<B256> = None;
        let mut total_processed = 0;
        let mut batch_num = 0;

        loop {
            batch_num += 1;
            let batch_start = std::time::Instant::now();

            // Fetch next batch of digests
            let digests =
                self.indexer.db.get_request_digests_paginated(cursor, DIGEST_BATCH_SIZE).await?;

            if digests.is_empty() {
                tracing::info!("No more digests to process");
                break;
            }

            let digest_count = digests.len();
            tracing::info!(
                "Batch {}: Fetched {} digests in {:?}",
                batch_num,
                digest_count,
                batch_start.elapsed()
            );

            // Update cursor for next iteration
            cursor = digests.last().copied();

            // Process in smaller sub-batches to avoid overwhelming the DB
            for (chunk_idx, chunk) in digests.chunks(STATUS_BATCH_SIZE).enumerate() {
                let chunk_start = std::time::Instant::now();

                let digest_set: HashSet<B256> = chunk.iter().copied().collect();

                // Fetch comprehensive data for these requests
                let requests_comprehensive =
                    self.indexer.db.get_requests_comprehensive(&digest_set).await?;

                // Compute statuses
                let request_statuses: Vec<_> = requests_comprehensive
                    .into_iter()
                    .map(|req| self.indexer.compute_request_status(req, current_timestamp))
                    .collect();

                // Upsert statuses
                self.indexer.db.upsert_request_statuses(&request_statuses).await?;

                total_processed += request_statuses.len();

                tracing::info!(
                    "Batch {} chunk {}: Processed {} statuses in {:?} (total: {})",
                    batch_num,
                    chunk_idx + 1,
                    request_statuses.len(),
                    chunk_start.elapsed(),
                    total_processed
                );
            }

            tracing::info!("Batch {} completed in {:?}", batch_num, batch_start.elapsed());
        }

        tracing::info!(
            "Status backfill completed: {} statuses updated in {:?}",
            total_processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_aggregates(&mut self) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting aggregate backfill...");

        // Get the timestamp range from start block (0 or earliest) to end block
        let start_block = 0u64;
        let start_timestamp = self.indexer.block_timestamp(start_block).await.unwrap_or(0);
        let end_timestamp = self.indexer.block_timestamp(self.end_block).await?;

        tracing::info!(
            "Recomputing aggregates for timestamp range {} to {} (blocks {} to {})",
            start_timestamp,
            end_timestamp,
            start_block,
            self.end_block
        );

        // Recompute hourly aggregates
        self.backfill_hourly_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute daily aggregates
        self.backfill_daily_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute weekly aggregates
        self.backfill_weekly_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute monthly aggregates
        self.backfill_monthly_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute all-time aggregates
        self.backfill_all_time_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute per-requestor aggregates
        self.backfill_requestor_aggregates(start_timestamp, end_timestamp).await?;

        tracing::info!("Aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_hourly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        // Collect periods first to get count for logging
        let periods: Vec<_> = iter_hourly_periods(start_ts, end_ts).collect();
        let total_hours = periods.len();

        tracing::info!(
            "Recomputing {} hourly periods from {} to {}",
            total_hours,
            periods.first().map(|p| p.0).unwrap_or(0),
            periods.last().map(|p| p.0).unwrap_or(0)
        );

        let mut processed = 0;
        for (hour_ts, hour_end) in periods {
            let summary = self.indexer.compute_period_summary(hour_ts, hour_end).await?;
            self.indexer.db.upsert_hourly_market_summary(summary).await?;

            processed += 1;
            if processed % 100 == 0 {
                tracing::info!("Processed {} / {} hourly periods", processed, total_hours);
            }
        }

        tracing::info!(
            "Hourly aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_daily_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        // Collect periods first to get count for logging
        let periods: Vec<_> = iter_daily_periods(start_ts, end_ts).collect();
        let total_days = periods.len();

        tracing::info!(
            "Recomputing {} daily periods from {} to {}",
            total_days,
            periods.first().map(|p| p.0).unwrap_or(0),
            periods.last().map(|p| p.0).unwrap_or(0)
        );

        let mut processed = 0;
        for (day_ts, day_end) in periods {
            let summary = self.indexer.compute_period_summary(day_ts, day_end).await?;
            self.indexer.db.upsert_daily_market_summary(summary).await?;

            processed += 1;
            if processed % 10 == 0 {
                tracing::info!("Processed {} / {} daily periods", processed, total_days);
            }
        }

        tracing::info!(
            "Daily aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_weekly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        // Collect periods first to get count for logging
        let periods: Vec<_> = iter_weekly_periods(start_ts, end_ts).collect();
        let total_weeks = periods.len();

        tracing::info!(
            "Recomputing {} weekly periods from {} to {}",
            total_weeks,
            periods.first().map(|p| p.0).unwrap_or(0),
            periods.last().map(|p| p.0).unwrap_or(0)
        );

        for (idx, (week_ts, week_end)) in periods.iter().enumerate() {
            let summary = self.indexer.compute_period_summary(*week_ts, *week_end).await?;
            self.indexer.db.upsert_weekly_market_summary(summary).await?;

            if (idx + 1) % 10 == 0 {
                tracing::info!("Processed {} / {} weekly periods", idx + 1, total_weeks);
            }
        }

        tracing::info!(
            "Weekly aggregate backfill completed: {} periods in {:?}",
            total_weeks,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_monthly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        // Collect periods first to get count for logging
        let periods: Vec<_> = iter_monthly_periods(start_ts, end_ts).collect();
        let total_months = periods.len();

        tracing::info!(
            "Recomputing {} monthly periods from {} to {}",
            total_months,
            periods.first().map(|p| p.0).unwrap_or(0),
            periods.last().map(|p| p.0).unwrap_or(0)
        );

        for (idx, (month_ts, month_end)) in periods.iter().enumerate() {
            let summary = self.indexer.compute_period_summary(*month_ts, *month_end).await?;
            self.indexer.db.upsert_monthly_market_summary(summary).await?;

            tracing::info!("Processed {} / {} monthly periods", idx + 1, total_months);
        }

        tracing::info!(
            "Monthly aggregate backfill completed: {} periods in {:?}",
            total_months,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_all_time_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        use crate::db::market::AllTimeMarketSummary;
        use crate::market::service::sum_hourly_aggregates_into_base;
        use crate::market::time_boundaries::{get_hour_start, get_previous_finished_hour, iter_hourly_periods};
        use alloy::primitives::U256;

        // Get current hour and previous finished hour using DRY helper functions
        let _current_hour = get_hour_start(end_ts);
        let previous_finished_hour = get_previous_finished_hour(end_ts);

        // Get latest all-time aggregate (if exists)
        let latest_all_time = self.indexer.db.get_latest_all_time_market_summary().await?;

        // Determine start hour
        let start_hour_aligned = get_hour_start(start_ts);
        let start_hour = if let Some(ref latest) = latest_all_time {
            // Start from the hour after the latest aggregate, or start_ts if later
            std::cmp::max(latest.period_timestamp + crate::market::service::SECONDS_PER_HOUR, start_hour_aligned)
        } else {
            start_hour_aligned
        };

        // Collect periods first to get count for logging
        let periods: Vec<_> = iter_hourly_periods(start_hour, previous_finished_hour + 1).collect();
        let total_hours = periods.len();

        if total_hours == 0 {
            tracing::info!("No hours to process for all-time aggregate backfill");
            return Ok(());
        }

        tracing::info!(
            "Recomputing {} all-time aggregate periods from {} to {}",
            total_hours,
            periods.first().map(|p| p.0).unwrap_or(0),
            periods.last().map(|p| p.0).unwrap_or(0)
        );

        // Get earliest hourly summary timestamp for cumulative aggregation
        let earliest_hourly_ts = self.indexer.db.get_earliest_hourly_summary_timestamp().await?
            .map(|ts| get_hour_start(ts))
            .unwrap_or(start_hour);

        // Fetch ALL hourly summaries once (from earliest to end)
        let all_hourly_summaries = self.indexer.db
            .get_hourly_market_summaries_by_range(earliest_hourly_ts, previous_finished_hour + 1)
            .await?;

        // Build a map for quick lookup: hour_timestamp -> hourly_summary
        let mut hourly_map: std::collections::BTreeMap<u64, _> = std::collections::BTreeMap::new();
        for summary in all_hourly_summaries {
            hourly_map.insert(summary.period_timestamp, summary);
        }

        // Start with base aggregate or zeros
        let mut current_all_time = if let Some(ref latest) = latest_all_time {
            // Only use latest if it's before start_hour
            if latest.period_timestamp < start_hour {
                latest.clone()
            } else {
                // Latest is at or after start_hour, start fresh
                AllTimeMarketSummary {
                    period_timestamp: start_hour,
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
                    locked_orders_fulfillment_rate: 0.0,
                    total_program_cycles: U256::ZERO,
                    total_cycles: U256::ZERO,
                    best_peak_prove_mhz: 0,
                    best_peak_prove_mhz_prover: None,
                    best_peak_prove_mhz_request_id: None,
                    best_effective_prove_mhz: 0,
                    best_effective_prove_mhz_prover: None,
                    best_effective_prove_mhz_request_id: None,
                }
            }
        } else {
            // No previous aggregate, start from zeros
            AllTimeMarketSummary {
                period_timestamp: start_hour,
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
                locked_orders_fulfillment_rate: 0.0,
                total_program_cycles: U256::ZERO,
                total_cycles: U256::ZERO,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            }
        };

        let mut processed = 0;
        for (hour_ts, _hour_end) in periods {
            // Get hourly summaries up to this hour (from earliest to current hour)
            let hourly_summaries: Vec<_> = hourly_map
                .range(earliest_hourly_ts..=hour_ts)
                .map(|(_, summary)| summary.clone())
                .collect();

            // If we have a previous aggregate, only sum new hours
            let hours_to_sum = if current_all_time.period_timestamp < hour_ts {
                // Sum only hours after the previous aggregate
                hourly_map
                    .range((current_all_time.period_timestamp + crate::market::service::SECONDS_PER_HOUR)..=hour_ts)
                    .map(|(_, summary)| summary.clone())
                    .collect()
            } else {
                // Sum all hours up to this hour
                hourly_summaries
            };

            // Update period_timestamp
            current_all_time.period_timestamp = hour_ts;

            // Sum hourly aggregates into base
            sum_hourly_aggregates_into_base(&mut current_all_time, &hours_to_sum);

            // Query unique counts from DB for this hour
            current_all_time.unique_provers_locking_requests = self.indexer.db
                .get_all_time_unique_provers(hour_ts)
                .await?;
            current_all_time.unique_requesters_submitting_requests = self.indexer.db
                .get_all_time_unique_requesters(hour_ts)
                .await?;

            // Upsert all-time aggregate for this hour
            self.indexer.db.upsert_all_time_market_summary(current_all_time.clone()).await?;

            processed += 1;
            if processed % 100 == 0 {
                tracing::info!("Processed {} / {} all-time aggregate periods", processed, total_hours);
            }
        }

        tracing::info!(
            "All-time aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    // Per-Requestor Backfill Methods

    async fn backfill_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting per-requestor aggregate backfill...");

        // Get all unique requestor addresses
        let requestors = self.indexer.db.get_all_requestor_addresses().await?;
        tracing::info!("Found {} unique requestors to process", requestors.len());

        // Recompute hourly aggregates for all requestors
        self.backfill_hourly_requestor_aggregates(start_ts, end_ts, &requestors).await?;

        // Recompute daily aggregates for all requestors
        self.backfill_daily_requestor_aggregates(start_ts, end_ts, &requestors).await?;

        // Recompute weekly aggregates for all requestors
        self.backfill_weekly_requestor_aggregates(start_ts, end_ts, &requestors).await?;

        // Recompute monthly aggregates for all requestors
        self.backfill_monthly_requestor_aggregates(start_ts, end_ts, &requestors).await?;

        // Recompute all-time aggregates for all requestors
        self.backfill_all_time_requestor_aggregates(start_ts, end_ts, &requestors).await?;

        tracing::info!("Per-requestor aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_hourly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
        requestors: &[alloy::primitives::Address],
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        let periods: Vec<_> = iter_hourly_periods(start_ts, end_ts).collect();
        let total_hours = periods.len();

        tracing::info!(
            "Recomputing hourly requestor aggregates: {} requestors × {} hours = {} total",
            requestors.len(),
            total_hours,
            requestors.len() * total_hours
        );

        let mut processed = 0;
        for requestor in requestors {
            for (hour_ts, hour_end) in &periods {
                let summary = self.indexer
                    .compute_period_requestor_summary(*hour_ts, *hour_end, *requestor)
                    .await?;
                
                // Only insert if there's activity
                if summary.total_requests_submitted > 0 {
                    self.indexer.db.upsert_hourly_requestor_summary(summary).await?;
                }

                processed += 1;
                if processed % 1000 == 0 {
                    tracing::info!(
                        "Processed {} / {} hourly requestor periods",
                        processed,
                        requestors.len() * total_hours
                    );
                }
            }
        }

        tracing::info!(
            "Hourly requestor aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_daily_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
        requestors: &[alloy::primitives::Address],
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        let periods: Vec<_> = iter_daily_periods(start_ts, end_ts).collect();
        let total_days = periods.len();

        tracing::info!(
            "Recomputing daily requestor aggregates: {} requestors × {} days = {} total",
            requestors.len(),
            total_days,
            requestors.len() * total_days
        );

        let mut processed = 0;
        for requestor in requestors {
            for (day_ts, day_end) in &periods {
                let summary = self.indexer
                    .compute_period_requestor_summary(*day_ts, *day_end, *requestor)
                    .await?;
                
                if summary.total_requests_submitted > 0 {
                    self.indexer.db.upsert_daily_requestor_summary(summary).await?;
                }

                processed += 1;
                if processed % 100 == 0 {
                    tracing::info!(
                        "Processed {} / {} daily requestor periods",
                        processed,
                        requestors.len() * total_days
                    );
                }
            }
        }

        tracing::info!(
            "Daily requestor aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_weekly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
        requestors: &[alloy::primitives::Address],
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        let periods: Vec<_> = iter_weekly_periods(start_ts, end_ts).collect();
        let total_weeks = periods.len();

        tracing::info!(
            "Recomputing weekly requestor aggregates: {} requestors × {} weeks = {} total",
            requestors.len(),
            total_weeks,
            requestors.len() * total_weeks
        );

        let mut processed = 0;
        for requestor in requestors {
            for (week_ts, week_end) in &periods {
                let summary = self.indexer
                    .compute_period_requestor_summary(*week_ts, *week_end, *requestor)
                    .await?;
                
                if summary.total_requests_submitted > 0 {
                    self.indexer.db.upsert_weekly_requestor_summary(summary).await?;
                }

                processed += 1;
                if processed % 50 == 0 {
                    tracing::info!(
                        "Processed {} / {} weekly requestor periods",
                        processed,
                        requestors.len() * total_weeks
                    );
                }
            }
        }

        tracing::info!(
            "Weekly requestor aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_monthly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
        requestors: &[alloy::primitives::Address],
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        let periods: Vec<_> = iter_monthly_periods(start_ts, end_ts).collect();
        let total_months = periods.len();

        tracing::info!(
            "Recomputing monthly requestor aggregates: {} requestors × {} months = {} total",
            requestors.len(),
            total_months,
            requestors.len() * total_months
        );

        let mut processed = 0;
        for requestor in requestors {
            for (month_ts, month_end) in &periods {
                let summary = self.indexer
                    .compute_period_requestor_summary(*month_ts, *month_end, *requestor)
                    .await?;
                
                if summary.total_requests_submitted > 0 {
                    self.indexer.db.upsert_monthly_requestor_summary(summary).await?;
                }

                processed += 1;
                if processed % 20 == 0 {
                    tracing::info!(
                        "Processed {} / {} monthly requestor periods",
                        processed,
                        requestors.len() * total_months
                    );
                }
            }
        }

        tracing::info!(
            "Monthly requestor aggregate backfill completed: {} periods in {:?}",
            processed,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_all_time_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
        requestors: &[alloy::primitives::Address],
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        let periods: Vec<_> = iter_hourly_periods(start_ts, end_ts).collect();
        let _total_hours = periods.len();

        tracing::info!(
            "Recomputing all-time requestor aggregates: {} requestors",
            requestors.len()
        );

        let mut processed_requestors = 0;
        for requestor in requestors {
            // Initialize with zeros
            let mut current_all_time = crate::db::market::AllTimeRequestorSummary {
                period_timestamp: 0,
                requestor_address: *requestor,
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
                locked_orders_fulfillment_rate: 0.0,
                total_program_cycles: alloy::primitives::U256::ZERO,
                total_cycles: alloy::primitives::U256::ZERO,
                best_peak_prove_mhz: 0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };

            let mut has_activity = false;
            for (hour_ts, _hour_end) in &periods {
                // Try to get the hourly summary for this requestor
                if let Ok(hourly_summaries) = self.indexer.db
                    .get_hourly_requestor_summaries_by_range(*requestor, *hour_ts, hour_ts + 1)
                    .await
                {
                    if let Some(hour_summary) = hourly_summaries.first() {
                        has_activity = true;
                        
                        // Add this hour's data
                        current_all_time.total_fulfilled += hour_summary.total_fulfilled;
                        current_all_time.total_requests_submitted += hour_summary.total_requests_submitted;
                        current_all_time.total_requests_submitted_onchain += hour_summary.total_requests_submitted_onchain;
                        current_all_time.total_requests_submitted_offchain += hour_summary.total_requests_submitted_offchain;
                        current_all_time.total_requests_locked += hour_summary.total_requests_locked;
                        current_all_time.total_requests_slashed += hour_summary.total_requests_slashed;
                        current_all_time.total_expired += hour_summary.total_expired;
                        current_all_time.total_locked_and_expired += hour_summary.total_locked_and_expired;
                        current_all_time.total_locked_and_fulfilled += hour_summary.total_locked_and_fulfilled;
                        current_all_time.total_program_cycles += hour_summary.total_program_cycles;
                        current_all_time.total_cycles += hour_summary.total_cycles;
                        current_all_time.total_fees_locked += hour_summary.total_fees_locked;
                        current_all_time.total_collateral_locked += hour_summary.total_collateral_locked;
                        current_all_time.total_locked_and_expired_collateral += hour_summary.total_locked_and_expired_collateral;

                        // Update best metrics
                        if hour_summary.best_peak_prove_mhz > current_all_time.best_peak_prove_mhz {
                            current_all_time.best_peak_prove_mhz = hour_summary.best_peak_prove_mhz;
                            current_all_time.best_peak_prove_mhz_prover = hour_summary.best_peak_prove_mhz_prover.clone();
                            current_all_time.best_peak_prove_mhz_request_id = hour_summary.best_peak_prove_mhz_request_id;
                        }
                        if hour_summary.best_effective_prove_mhz > current_all_time.best_effective_prove_mhz {
                            current_all_time.best_effective_prove_mhz = hour_summary.best_effective_prove_mhz;
                            current_all_time.best_effective_prove_mhz_prover = hour_summary.best_effective_prove_mhz_prover.clone();
                            current_all_time.best_effective_prove_mhz_request_id = hour_summary.best_effective_prove_mhz_request_id;
                        }
                    }
                }

                current_all_time.period_timestamp = *hour_ts;

                // Query unique provers for this requestor up to this hour
                current_all_time.unique_provers_locking_requests = self.indexer.db
                    .get_all_time_requestor_unique_provers(hour_ts + 1, *requestor)
                    .await?;

                // Recalculate fulfillment rate
                let total_locked_outcomes = current_all_time.total_locked_and_fulfilled + current_all_time.total_locked_and_expired;
                current_all_time.locked_orders_fulfillment_rate = if total_locked_outcomes > 0 {
                    (current_all_time.total_locked_and_fulfilled as f32 / total_locked_outcomes as f32) * 100.0
                } else {
                    0.0
                };

                // Upsert all-time aggregate for this hour only if there's activity
                if has_activity {
                    self.indexer.db.upsert_all_time_requestor_summary(current_all_time.clone()).await?;
                }
            }

            processed_requestors += 1;
            if processed_requestors % 10 == 0 {
                tracing::info!(
                    "Processed {} / {} requestors for all-time aggregates",
                    processed_requestors,
                    requestors.len()
                );
            }
        }

        tracing::info!(
            "All-time requestor aggregate backfill completed: {} requestors in {:?}",
            processed_requestors,
            start_time.elapsed()
        );
        Ok(())
    }
}
