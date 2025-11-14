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
}
