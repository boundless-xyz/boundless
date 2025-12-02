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
use crate::market::{
    time_boundaries::{get_day_start, get_hour_start, get_month_start, get_week_start},
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
    pub start_block: u64,
    pub end_block: u64,
}

impl<P, ANP> BackfillService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub fn new(
        indexer: IndexerService<P, ANP>,
        mode: BackfillMode,
        start_block: u64,
        end_block: u64,
    ) -> Self {
        Self { indexer, mode, start_block, end_block }
    }

    pub async fn run(&mut self) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();

        tracing::info!(
            "Starting backfill with mode: {:?}, start_block: {}, end_block: {}",
            self.mode,
            self.start_block,
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

        // Get the timestamp range from start block to end block
        let start_timestamp = self.indexer.block_timestamp(self.start_block).await.unwrap_or(0);
        let end_timestamp = self.indexer.block_timestamp(self.end_block).await?;

        tracing::info!(
            "Recomputing aggregates for timestamp range {} to {} (blocks {} to {})",
            start_timestamp,
            end_timestamp,
            self.start_block,
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
        // Align timestamps to hour boundaries
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);

        self.indexer.aggregate_hourly_market_data_from(start_hour, end_hour).await
    }

    async fn backfill_daily_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to day boundaries
        let start_day = get_day_start(start_ts);
        let end_day = get_day_start(end_ts);

        self.indexer.aggregate_daily_market_data_from(start_day, end_day).await
    }

    async fn backfill_weekly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to week boundaries
        let start_week = get_week_start(start_ts);
        let end_week = get_week_start(end_ts);

        self.indexer.aggregate_weekly_market_data_from(start_week, end_week).await
    }

    async fn backfill_monthly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to month boundaries
        let start_month = get_month_start(start_ts);
        let end_month = get_month_start(end_ts);

        self.indexer.aggregate_monthly_market_data_from(start_month, end_month).await
    }

    async fn backfill_all_time_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to hour boundaries
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);

        self.indexer.aggregate_all_time_market_data_from(start_hour, end_hour).await
    }

    // Per-Requestor Backfill Methods

    async fn backfill_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting per-requestor aggregate backfill...");

        // Recompute hourly aggregates
        self.backfill_hourly_requestor_aggregates(start_ts, end_ts).await?;

        // Recompute daily aggregates
        self.backfill_daily_requestor_aggregates(start_ts, end_ts).await?;

        // Recompute weekly aggregates
        self.backfill_weekly_requestor_aggregates(start_ts, end_ts).await?;

        // Recompute monthly aggregates
        self.backfill_monthly_requestor_aggregates(start_ts, end_ts).await?;

        // Recompute all-time aggregates
        self.backfill_all_time_requestor_aggregates(start_ts, end_ts).await?;

        tracing::info!("Per-requestor aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_hourly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to hour boundaries
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);

        self.indexer.aggregate_hourly_requestor_data_from(start_hour, end_hour).await
    }

    async fn backfill_daily_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to day boundaries
        let start_day = get_day_start(start_ts);
        let end_day = get_day_start(end_ts);

        self.indexer.aggregate_daily_requestor_data_from(start_day, end_day).await
    }

    async fn backfill_weekly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to week boundaries
        let start_week = get_week_start(start_ts);
        let end_week = get_week_start(end_ts);

        self.indexer.aggregate_weekly_requestor_data_from(start_week, end_week).await
    }

    async fn backfill_monthly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to month boundaries
        let start_month = get_month_start(start_ts);
        let end_month = get_month_start(end_ts);

        self.indexer.aggregate_monthly_requestor_data_from(start_month, end_month).await
    }

    async fn backfill_all_time_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Align timestamps to hour boundaries
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);

        self.indexer.aggregate_all_time_requestor_data_from(start_hour, end_hour).await
    }
}
