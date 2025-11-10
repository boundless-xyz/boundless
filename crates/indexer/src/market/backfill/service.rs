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

use crate::market::{IndexerService, ServiceError};
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
        use super::super::service::SECONDS_PER_HOUR;

        let start_time = std::time::Instant::now();

        // Calculate hour boundaries
        let start_hour = (start_ts / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;
        let end_hour = (end_ts / SECONDS_PER_HOUR) * SECONDS_PER_HOUR;

        let total_hours = ((end_hour - start_hour) / SECONDS_PER_HOUR) + 1;
        tracing::info!(
            "Recomputing {} hourly periods from {} to {}",
            total_hours,
            start_hour,
            end_hour
        );

        let mut processed = 0;
        for hour_ts in (start_hour..=end_hour).step_by(SECONDS_PER_HOUR as usize) {
            let hour_end = hour_ts.saturating_add(SECONDS_PER_HOUR);
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
        use super::super::service::SECONDS_PER_DAY;

        let start_time = std::time::Instant::now();

        // Use helper functions from aggregation module
        let start_day = self.get_day_start(start_ts);
        let end_day = self.get_day_start(end_ts);

        let total_days = ((end_day - start_day) / SECONDS_PER_DAY) + 1;
        tracing::info!(
            "Recomputing {} daily periods from {} to {}",
            total_days,
            start_day,
            end_day
        );

        let mut processed = 0;
        for day_ts in (start_day..=end_day).step_by(SECONDS_PER_DAY as usize) {
            let day_end = self.get_next_day(day_ts);
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
        use super::super::service::SECONDS_PER_WEEK;

        let start_time = std::time::Instant::now();

        let start_week = self.get_week_start(start_ts);
        let end_week = self.get_week_start(end_ts);

        let mut periods = Vec::new();
        let mut week_ts = start_week;
        while week_ts <= end_week {
            periods.push(week_ts);
            week_ts += SECONDS_PER_WEEK;
        }

        tracing::info!(
            "Recomputing {} weekly periods from {} to {}",
            periods.len(),
            start_week,
            end_week
        );

        for (idx, week_ts) in periods.iter().enumerate() {
            let week_end = self.get_next_week(*week_ts);
            let summary = self.indexer.compute_period_summary(*week_ts, week_end).await?;
            self.indexer.db.upsert_weekly_market_summary(summary).await?;

            if (idx + 1) % 10 == 0 {
                tracing::info!("Processed {} / {} weekly periods", idx + 1, periods.len());
            }
        }

        tracing::info!(
            "Weekly aggregate backfill completed: {} periods in {:?}",
            periods.len(),
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

        let start_month = self.get_month_start(start_ts);
        let end_month = self.get_month_start(end_ts);

        let mut periods = Vec::new();
        let mut month_ts = start_month;
        while month_ts <= end_month {
            periods.push(month_ts);
            month_ts = self.get_next_month(month_ts);
        }

        tracing::info!(
            "Recomputing {} monthly periods from {} to {}",
            periods.len(),
            start_month,
            end_month
        );

        for (idx, month_ts) in periods.iter().enumerate() {
            let month_end = self.get_next_month(*month_ts);
            let summary = self.indexer.compute_period_summary(*month_ts, month_end).await?;
            self.indexer.db.upsert_monthly_market_summary(summary).await?;

            tracing::info!("Processed {} / {} monthly periods", idx + 1, periods.len());
        }

        tracing::info!(
            "Monthly aggregate backfill completed: {} periods in {:?}",
            periods.len(),
            start_time.elapsed()
        );
        Ok(())
    }

    // Helper functions for period boundaries (copied from aggregation.rs)
    fn get_day_start(&self, timestamp: u64) -> u64 {
        use super::super::service::SECONDS_PER_DAY;
        (timestamp / SECONDS_PER_DAY) * SECONDS_PER_DAY
    }

    fn get_next_day(&self, timestamp: u64) -> u64 {
        use super::super::service::SECONDS_PER_DAY;
        self.get_day_start(timestamp) + SECONDS_PER_DAY
    }

    fn get_week_start(&self, timestamp: u64) -> u64 {
        use chrono::{Datelike, TimeZone, Utc, Weekday};

        let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
        let weekday = dt.weekday();

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

    fn get_next_week(&self, timestamp: u64) -> u64 {
        use super::super::service::SECONDS_PER_WEEK;
        self.get_week_start(timestamp) + SECONDS_PER_WEEK
    }

    fn get_month_start(&self, timestamp: u64) -> u64 {
        use chrono::{Datelike, TimeZone, Utc};

        let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();
        let month_start = Utc.with_ymd_and_hms(dt.year(), dt.month(), 1, 0, 0, 0).unwrap();
        month_start.timestamp() as u64
    }

    fn get_next_month(&self, timestamp: u64) -> u64 {
        use chrono::{Datelike, TimeZone, Utc};

        let dt = Utc.timestamp_opt(timestamp as i64, 0).unwrap();

        let next_month = if dt.month() == 12 {
            Utc.with_ymd_and_hms(dt.year() + 1, 1, 1, 0, 0, 0).unwrap()
        } else {
            Utc.with_ymd_and_hms(dt.year(), dt.month() + 1, 1, 0, 0, 0).unwrap()
        };

        next_month.timestamp() as u64
    }
}
