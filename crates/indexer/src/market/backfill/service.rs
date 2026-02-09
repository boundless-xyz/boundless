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

use crate::{
    db::market::IndexerDb,
    market::{
        time_boundaries::{
            get_day_start, get_hour_start, get_month_start, get_next_day, get_next_hour,
            get_next_month, get_next_week, get_week_start,
        },
        IndexerService, ServiceError,
    },
};
use alloy::{
    network::{AnyNetwork, Ethereum},
    primitives::B256,
    providers::Provider,
};
use std::collections::HashSet;
use std::time::Duration;

const DIGEST_BATCH_SIZE: i64 = 5000;
const STATUS_BATCH_SIZE: usize = 2500;

// Chunk sizes for backfill progress reporting
// Note: All chunk sizes must be > 1 to satisfy from_time < to_time validation
const HOURLY_CHUNK_SIZE_HOURS: u64 = 48; // Process 2 days at a time
const DAILY_CHUNK_SIZE_DAYS: u64 = 7; // Process 1 week at a time
const WEEKLY_CHUNK_SIZE_WEEKS: u64 = 2; // Process 2 weeks at a time
const MONTHLY_CHUNK_SIZE_MONTHS: u64 = 2; // Process 2 months at a time
const ALL_TIME_CHUNK_SIZE_HOURS: u64 = 48; // Process 2 days at a time (same as hourly)
const EPOCH_CHUNK_SIZE_EPOCHS: u64 = 2; // Process 2 epochs at a time (4 days)

/// Generic helper function to chunk a time range into smaller chunks
/// Returns an iterator of (chunk_start, chunk_end) tuples where both are inclusive period boundaries
///
/// # Arguments
/// * `start` - Start timestamp (must be aligned to period boundary)
/// * `end` - End timestamp (must be aligned to period boundary, inclusive)
/// * `chunk_size` - Number of periods per chunk (must be > 1)
/// * `get_next` - Function to advance to the next period boundary
fn chunk_time_range<F>(
    start: u64,
    end: u64,
    chunk_size: u64,
    get_next: F,
) -> impl Iterator<Item = (u64, u64)>
where
    F: Fn(u64) -> u64,
{
    // Require chunk_size > 1 to ensure from_time < to_time validation passes
    assert!(chunk_size > 1, "chunk_size must be greater than 1, got {}", chunk_size);

    let mut chunks: Vec<(u64, u64)> = Vec::new();
    let mut current = start;

    while current <= end {
        // Try to advance chunk_size - 1 times to create a chunk of chunk_size periods
        let mut chunk_end = current;
        let mut advanced = false;

        for _ in 1..chunk_size {
            let next = get_next(chunk_end);
            if next > end {
                // Hit the end boundary - clamp to end
                chunk_end = end;
                break;
            }
            chunk_end = next;
            advanced = true;
        }

        // If we couldn't advance at all, handle the final period
        if !advanced {
            // current <= end but couldn't advance - clamp to end
            chunk_end = end;
        }

        // Ensure chunk_end doesn't exceed end (safety check)
        if chunk_end > end {
            chunk_end = end;
        }

        // Add chunk (aggregation functions handle from_time == to_time by processing single period)
        chunks.push((current, chunk_end));

        // If we've reached or exceeded end, we're done
        if chunk_end >= end {
            break;
        }

        // Move to the next chunk starting point
        current = get_next(chunk_end);
    }

    chunks.into_iter()
}

/// Helper function to chunk hourly range into smaller chunks
/// Returns an iterator of (chunk_start, chunk_end) tuples where both are inclusive hour boundaries
/// Automatically aligns start and end to hour boundaries
fn chunk_hourly_range(
    start_ts: u64,
    end_ts: u64,
    chunk_size_hours: u64,
) -> impl Iterator<Item = (u64, u64)> {
    let start_hour = get_hour_start(start_ts);
    let end_hour = get_hour_start(end_ts);
    chunk_time_range(start_hour, end_hour, chunk_size_hours, get_next_hour)
}

/// Helper function to chunk daily range into smaller chunks
/// Returns an iterator of (chunk_start, chunk_end) tuples where both are inclusive day boundaries
/// Automatically aligns start and end to day boundaries
fn chunk_daily_range(
    start_ts: u64,
    end_ts: u64,
    chunk_size_days: u64,
) -> impl Iterator<Item = (u64, u64)> {
    let start_day = get_day_start(start_ts);
    let end_day = get_day_start(end_ts);
    chunk_time_range(start_day, end_day, chunk_size_days, get_next_day)
}

/// Helper function to chunk weekly range into smaller chunks
/// Returns an iterator of (chunk_start, chunk_end) tuples where both are inclusive week boundaries
/// Automatically aligns start and end to week boundaries
fn chunk_weekly_range(
    start_ts: u64,
    end_ts: u64,
    chunk_size_weeks: u64,
) -> impl Iterator<Item = (u64, u64)> {
    let start_week = get_week_start(start_ts);
    let end_week = get_week_start(end_ts);
    chunk_time_range(start_week, end_week, chunk_size_weeks, get_next_week)
}

/// Helper function to chunk monthly range into smaller chunks
/// Returns an iterator of (chunk_start, chunk_end) tuples where both are inclusive month boundaries
/// Automatically aligns start and end to month boundaries
fn chunk_monthly_range(
    start_ts: u64,
    end_ts: u64,
    chunk_size_months: u64,
) -> impl Iterator<Item = (u64, u64)> {
    let start_month = get_month_start(start_ts);
    let end_month = get_month_start(end_ts);
    chunk_time_range(start_month, end_month, chunk_size_months, get_next_month)
}

/// Chunk epochs within a time range into smaller groups
/// Returns an iterator of (chunk_start_time, chunk_end_time) tuples
fn chunk_epoch_range(
    epoch_calc: &crate::market::epoch_calculator::EpochCalculator,
    start_ts: u64,
    end_ts: u64,
    chunk_size_epochs: u64,
) -> impl Iterator<Item = (u64, u64)> {
    let epoch0_start = epoch_calc.epoch0_start_time();

    // Skip if entire range is before epoch 0
    if end_ts < epoch0_start {
        return Vec::new().into_iter();
    }

    let effective_start = start_ts.max(epoch0_start);
    let start_epoch = epoch_calc.get_epoch_for_timestamp(effective_start).unwrap_or(0);
    let end_epoch = epoch_calc.get_epoch_for_timestamp(end_ts).unwrap_or(start_epoch);

    let mut chunks = Vec::new();
    let mut current_epoch = start_epoch;

    while current_epoch <= end_epoch {
        let chunk_end_epoch = std::cmp::min(current_epoch + chunk_size_epochs - 1, end_epoch);

        let chunk_start_time = epoch_calc.get_epoch_start_time(current_epoch);
        let chunk_end_time = epoch_calc.get_epoch_end_time(chunk_end_epoch);

        chunks.push((chunk_start_time, chunk_end_time));
        current_epoch = chunk_end_epoch + 1;
    }

    chunks.into_iter()
}

#[derive(Debug, Clone, Copy)]
pub enum BackfillMode {
    StatusesAndAggregates,
    Aggregates,
    ChainData,
}

pub struct BackfillService<P, ANP> {
    pub indexer: IndexerService<P, ANP>,
    pub mode: BackfillMode,
    pub start_block: u64,
    pub end_block: u64,
    pub chain_data_batch_delay_ms: u64,
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
        chain_data_batch_delay_ms: u64,
    ) -> Self {
        Self { indexer, mode, start_block, end_block, chain_data_batch_delay_ms }
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
            BackfillMode::ChainData => {
                self.backfill_chain_data().await?;
            }
        }

        tracing::info!("Backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_chain_data(&mut self) -> Result<(), ServiceError> {
        use std::cmp::min;

        let start_time = std::time::Instant::now();
        tracing::info!(
            "Starting chain data backfill from block {} to {} (batch_size: {}, delay_ms: {})...",
            self.start_block,
            self.end_block,
            self.indexer.config.batch_size,
            self.chain_data_batch_delay_ms
        );

        let batch_size = self.indexer.config.batch_size;
        let mut from_block = self.start_block;
        let mut batch_num = 0;

        // Calculate total number of batches for progress reporting
        let total_blocks = self.end_block.saturating_sub(self.start_block) + 1;
        let total_batches = total_blocks.div_ceil(batch_size);

        while from_block <= self.end_block {
            batch_num += 1;
            let to_block = min(from_block + batch_size - 1, self.end_block);
            let batch_start = std::time::Instant::now();

            tracing::info!(
                "=== Chain data backfill batch {}/{}: processing blocks {} to {} ===",
                batch_num,
                total_batches,
                from_block,
                to_block
            );

            // Fetch logs from RPC
            let logs = self.indexer.fetch_logs(from_block, to_block).await?;
            tracing::info!(
                "Batch {}: fetched {} logs from blocks {} to {}",
                batch_num,
                logs.len(),
                from_block,
                to_block
            );

            // Apply rate limiting delay between batches if configured
            if self.chain_data_batch_delay_ms > 0 {
                tracing::info!(
                    "Applying delay: {}ms before next batch",
                    self.chain_data_batch_delay_ms
                );
                tokio::time::sleep(Duration::from_millis(self.chain_data_batch_delay_ms)).await;
            }

            if !logs.is_empty() {
                // Fetch tx metadata for the logs
                self.indexer.fetch_tx_metadata(&logs, from_block, to_block).await?;

                // Process all events
                let (submitted_digests, _offchain_digests) = tokio::try_join!(
                    self.indexer.process_request_submitted_events(&logs),
                    self.indexer.process_request_submitted_offchain(from_block, to_block)
                )?;

                let locked_digests = self.indexer.process_locked_events(&logs).await?;
                let proof_delivered_digests =
                    self.indexer.process_proof_delivered_events(&logs).await?;
                let fulfilled_digests = self.indexer.process_fulfilled_events(&logs).await?;
                let callback_failed_digests =
                    self.indexer.process_callback_failed_events(&logs).await?;
                let slashed_digests = self.indexer.process_slashed_events(&logs).await?;

                // Process deposit/withdrawal events
                tokio::try_join!(
                    self.indexer.process_deposit_events(&logs),
                    self.indexer.process_withdrawal_events(&logs),
                    self.indexer.process_collateral_deposit_events(&logs),
                    self.indexer.process_collateral_withdrawal_events(&logs)
                )?;

                let total_touched = submitted_digests.len()
                    + locked_digests.len()
                    + proof_delivered_digests.len()
                    + fulfilled_digests.len()
                    + callback_failed_digests.len()
                    + slashed_digests.len();

                tracing::info!(
                    "Completed chain data backfill batch {}/{} [blocks {} to {}] : processed {} events, touched {} request digests in {:?}",
                    batch_num, total_batches, from_block, to_block, logs.len(), total_touched, batch_start.elapsed()
                );
            } else {
                tracing::info!(
                    "Completed chain data backfill batch {}/{} [blocks {} to {}] : no logs found in blocks {} to {} ({:?})",
                    batch_num, total_batches, from_block, to_block, from_block, to_block, batch_start.elapsed()
                );
            }

            // Clear in-memory cache to free memory
            self.indexer.clear_in_memory_cache();

            from_block = to_block + 1;
        }

        tracing::info!(
            "Chain data backfill completed: processed {} batches (blocks {} to {}) in {:?}",
            batch_num,
            self.start_block,
            self.end_block,
            start_time.elapsed()
        );
        Ok(())
    }

    async fn backfill_statuses(&mut self) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting status backfill...");

        let current_timestamp = self.indexer.block_timestamp(self.end_block).await?;
        let end_timestamp = current_timestamp;
        tracing::info!(
            "Using end block {} with timestamp {} as 'current time' for status computation and filtering",
            self.end_block,
            end_timestamp
        );

        // Get total count of digests to process for logging
        let total_count = self.indexer.db.count_request_digests_by_timestamp(end_timestamp).await?;
        tracing::info!("Total statuses (digests) to recompute: {}", total_count);

        let mut cursor: Option<(u64, B256)> = None;
        let mut total_processed = 0;
        let mut batch_num = 0;

        loop {
            batch_num += 1;
            let batch_start = std::time::Instant::now();

            // Fetch next batch of digests with timestamp filtering
            let digest_results = self
                .indexer
                .db
                .get_all_request_digests(cursor, end_timestamp, DIGEST_BATCH_SIZE)
                .await?;

            if digest_results.is_empty() {
                tracing::info!("No more digests to process");
                break;
            }

            // Extract just the digests (created_at is used for cursor only)
            let digests: Vec<B256> = digest_results.iter().map(|(digest, _)| *digest).collect();
            let digest_count = digests.len();

            tracing::info!(
                "Batch {}: Fetched {} digests in {:?} (total processed: {})",
                batch_num,
                digest_count,
                batch_start.elapsed(),
                total_processed + digest_count
            );

            // Update cursor for next iteration (use last item's timestamp and digest)
            // Note: digest_results is Vec<(B256, u64)> but cursor is (u64, B256)
            if let Some((last_digest, last_ts)) = digest_results.last() {
                cursor = Some((*last_ts, *last_digest));
            }

            // Process in smaller sub-batches to avoid overwhelming the DB
            for (chunk_idx, chunk) in digests.chunks(STATUS_BATCH_SIZE).enumerate() {
                let chunk_start = std::time::Instant::now();

                let digest_set: HashSet<B256> = chunk.iter().copied().collect();

                // Fetch comprehensive data for these requests
                let requests_comprehensive =
                    self.indexer.db.get_requests_comprehensive(&digest_set).await?;

                // Fetch base fees for lock blocks
                let lock_blocks: std::collections::HashSet<u64> = requests_comprehensive
                    .iter()
                    .filter_map(|req| req.lock_block)
                    .collect();
                let mut base_fee_map: std::collections::HashMap<u64, Option<u128>> =
                    std::collections::HashMap::new();
                for &block_num in &lock_blocks {
                    let base_fee = self.indexer.db.get_block_base_fee(block_num).await?;
                    base_fee_map.insert(block_num, base_fee);
                }

                // Compute statuses
                let request_statuses: Vec<_> = requests_comprehensive
                    .into_iter()
                    .map(|req| {
                        self.indexer
                            .compute_request_status(req, current_timestamp, &base_fee_map)
                    })
                    .collect();

                // Upsert statuses
                self.indexer.db.upsert_request_statuses(&request_statuses).await?;

                tracing::debug!(
                    "Batch {} chunk {}: Processed {} statuses in {:?}",
                    batch_num,
                    chunk_idx + 1,
                    request_statuses.len(),
                    chunk_start.elapsed()
                );
            }

            // Count unique digests processed
            total_processed += digest_count;

            tracing::info!(
                "Batch {} completed in {:?}. Total unique digests processed: {}/{}",
                batch_num,
                batch_start.elapsed(),
                total_processed,
                total_count
            );
        }

        tracing::info!(
            "Status backfill completed: {} unique digests processed in {:?}",
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

        // Recompute market aggregates
        self.backfill_market_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute per-requestor aggregates
        self.backfill_requestor_aggregates(start_timestamp, end_timestamp).await?;

        // Recompute per-prover aggregates
        self.backfill_prover_aggregates(start_timestamp, end_timestamp).await?;

        tracing::info!("Aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_market_aggregates(
        &mut self,
        start_timestamp: u64,
        end_timestamp: u64,
    ) -> Result<(), ServiceError> {
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

        // Recompute epoch aggregates
        self.backfill_epoch_market_aggregates(start_timestamp, end_timestamp).await?;

        Ok(())
    }

    async fn backfill_hourly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;

        // Calculate chunks and process with progress messages
        // chunk_hourly_range automatically aligns timestamps to hour boundaries
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, HOURLY_CHUNK_SIZE_HOURS).collect();
        let total_chunks = chunks.len();

        // Compute aligned values for logging
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        tracing::info!(
            "Processing hourly aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing hourly chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_hourly_market_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_daily_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_daily_range automatically aligns timestamps to day boundaries
        let chunks: Vec<_> = chunk_daily_range(start_ts, end_ts, DAILY_CHUNK_SIZE_DAYS).collect();

        // Compute aligned values for logging
        let start_day = get_day_start(start_ts);
        let end_day = get_day_start(end_ts);
        let total_chunks = chunks.len();

        let total_days = {
            let mut count = 0;
            let mut current = start_day;
            while current <= end_day {
                count += 1;
                current = get_next_day(current);
            }
            count
        };

        tracing::info!(
            "Processing daily aggregates: {} days in {} chunks",
            total_days,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_day(current);
                }
                count
            };

            tracing::info!(
                "Processing daily chunk {}/{}: {} to {} ({} days)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_daily_market_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_weekly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_weekly_range automatically aligns timestamps to week boundaries
        let chunks: Vec<_> =
            chunk_weekly_range(start_ts, end_ts, WEEKLY_CHUNK_SIZE_WEEKS).collect();

        // Compute aligned values for logging
        let start_week = get_week_start(start_ts);
        let end_week = get_week_start(end_ts);
        let total_chunks = chunks.len();

        let total_weeks = {
            let mut count = 0;
            let mut current = start_week;
            while current <= end_week {
                count += 1;
                current = get_next_week(current);
            }
            count
        };

        tracing::info!(
            "Processing weekly aggregates: {} weeks in {} chunks",
            total_weeks,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_week(current);
                }
                count
            };

            tracing::info!(
                "Processing weekly chunk {}/{}: {} to {} ({} weeks)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_weekly_market_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_monthly_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_monthly_range automatically aligns timestamps to month boundaries
        let chunks: Vec<_> =
            chunk_monthly_range(start_ts, end_ts, MONTHLY_CHUNK_SIZE_MONTHS).collect();

        // Compute aligned values for logging
        let start_month = get_month_start(start_ts);
        let end_month = get_month_start(end_ts);
        let total_chunks = chunks.len();

        let total_months = {
            let mut count = 0;
            let mut current = start_month;
            while current <= end_month {
                count += 1;
                current = get_next_month(current);
            }
            count
        };

        tracing::info!(
            "Processing monthly aggregates: {} months in {} chunks",
            total_months,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_month(current);
                }
                count
            };

            tracing::info!(
                "Processing monthly chunk {}/{}: {} to {} ({} months)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_monthly_market_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_all_time_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;

        // Calculate chunks and process with progress messages
        // chunk_hourly_range automatically aligns timestamps to hour boundaries
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, ALL_TIME_CHUNK_SIZE_HOURS).collect();

        // Compute aligned values for logging
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        let total_chunks = chunks.len();

        tracing::info!(
            "Processing all-time aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing all-time chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_all_time_market_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_epoch_market_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let epoch_calc = &self.indexer.epoch_calculator;

        // Skip if entire range is before epoch 0
        let epoch0_start = epoch_calc.epoch0_start_time();
        if end_ts < epoch0_start {
            tracing::info!(
                "Skipping epoch market aggregates: end_ts {} is before epoch 0 start {}",
                end_ts,
                epoch0_start
            );
            return Ok(());
        }

        // Adjust start to epoch 0 start if needed
        let effective_start = start_ts.max(epoch0_start);

        let start_epoch = epoch_calc.get_epoch_for_timestamp(effective_start).unwrap_or(0);
        let end_epoch = epoch_calc.get_epoch_for_timestamp(end_ts).unwrap_or(start_epoch);

        if start_epoch > end_epoch {
            tracing::info!(
                "No epochs to process: start_epoch {} > end_epoch {}",
                start_epoch,
                end_epoch
            );
            return Ok(());
        }

        let total_epochs = end_epoch - start_epoch + 1;

        // Calculate chunks
        let chunks: Vec<_> =
            chunk_epoch_range(epoch_calc, effective_start, end_ts, EPOCH_CHUNK_SIZE_EPOCHS)
                .collect();

        let total_chunks = chunks.len();

        tracing::info!(
            "Processing epoch market aggregates: {} epochs in {} chunks",
            total_epochs,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let chunk_start_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_start).unwrap_or(0);
            let chunk_end_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_end).unwrap_or(0);
            let epoch_count = chunk_end_epoch - chunk_start_epoch + 1;

            tracing::info!(
                "Processing epoch market chunk {}/{}: epochs {} [{}] to {} [{}] ({} epochs)",
                chunk_idx + 1,
                total_chunks,
                chunk_start_epoch,
                *chunk_start,
                chunk_end_epoch,
                *chunk_end,
                epoch_count
            );

            self.indexer.aggregate_epoch_market_data_from(*chunk_start, *chunk_end).await?;
        }

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

        // Recompute epoch aggregates
        self.backfill_epoch_requestor_aggregates(start_ts, end_ts).await?;

        tracing::info!("Per-requestor aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_hourly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;

        // Calculate chunks and process with progress messages
        // chunk_hourly_range automatically aligns timestamps to hour boundaries
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, HOURLY_CHUNK_SIZE_HOURS).collect();

        // Compute aligned values for logging
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        let total_chunks = chunks.len();

        tracing::info!(
            "Processing hourly requestor aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing hourly requestor chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_hourly_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_daily_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_daily_range automatically aligns timestamps to day boundaries
        let chunks: Vec<_> = chunk_daily_range(start_ts, end_ts, DAILY_CHUNK_SIZE_DAYS).collect();

        // Compute aligned values for logging
        let start_day = get_day_start(start_ts);
        let end_day = get_day_start(end_ts);
        let total_chunks = chunks.len();

        let total_days = {
            let mut count = 0;
            let mut current = start_day;
            while current <= end_day {
                count += 1;
                current = get_next_day(current);
            }
            count
        };

        tracing::info!(
            "Processing daily requestor aggregates: {} days in {} chunks",
            total_days,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_day(current);
                }
                count
            };

            tracing::info!(
                "Processing daily requestor chunk {}/{}: {} to {} ({} days)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_daily_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_weekly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_weekly_range automatically aligns timestamps to week boundaries
        let chunks: Vec<_> =
            chunk_weekly_range(start_ts, end_ts, WEEKLY_CHUNK_SIZE_WEEKS).collect();

        // Compute aligned values for logging
        let start_week = get_week_start(start_ts);
        let end_week = get_week_start(end_ts);
        let total_chunks = chunks.len();

        let total_weeks = {
            let mut count = 0;
            let mut current = start_week;
            while current <= end_week {
                count += 1;
                current = get_next_week(current);
            }
            count
        };

        tracing::info!(
            "Processing weekly requestor aggregates: {} weeks in {} chunks",
            total_weeks,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_week(current);
                }
                count
            };

            tracing::info!(
                "Processing weekly requestor chunk {}/{}: {} to {} ({} weeks)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_weekly_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_monthly_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        // Calculate chunks and process with progress messages
        // chunk_monthly_range automatically aligns timestamps to month boundaries
        let chunks: Vec<_> =
            chunk_monthly_range(start_ts, end_ts, MONTHLY_CHUNK_SIZE_MONTHS).collect();

        // Compute aligned values for logging
        let start_month = get_month_start(start_ts);
        let end_month = get_month_start(end_ts);
        let total_chunks = chunks.len();

        let total_months = {
            let mut count = 0;
            let mut current = start_month;
            while current <= end_month {
                count += 1;
                current = get_next_month(current);
            }
            count
        };

        tracing::info!(
            "Processing monthly requestor aggregates: {} months in {} chunks",
            total_months,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_month(current);
                }
                count
            };

            tracing::info!(
                "Processing monthly requestor chunk {}/{}: {} to {} ({} months)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_monthly_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_all_time_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;

        // Calculate chunks and process with progress messages
        // chunk_hourly_range automatically aligns timestamps to hour boundaries
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, ALL_TIME_CHUNK_SIZE_HOURS).collect();

        // Compute aligned values for logging
        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        let total_chunks = chunks.len();

        tracing::info!(
            "Processing all-time requestor aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing all-time requestor chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_all_time_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_epoch_requestor_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let epoch_calc = &self.indexer.epoch_calculator;

        // Skip if entire range is before epoch 0
        let epoch0_start = epoch_calc.epoch0_start_time();
        if end_ts < epoch0_start {
            tracing::info!(
                "Skipping epoch requestor aggregates: end_ts {} is before epoch 0 start {}",
                end_ts,
                epoch0_start
            );
            return Ok(());
        }

        // Adjust start to epoch 0 start if needed
        let effective_start = start_ts.max(epoch0_start);

        let start_epoch = epoch_calc.get_epoch_for_timestamp(effective_start).unwrap_or(0);
        let end_epoch = epoch_calc.get_epoch_for_timestamp(end_ts).unwrap_or(start_epoch);

        if start_epoch > end_epoch {
            tracing::info!(
                "No epochs to process for requestors: start_epoch {} > end_epoch {}",
                start_epoch,
                end_epoch
            );
            return Ok(());
        }

        let total_epochs = end_epoch - start_epoch + 1;

        // Calculate chunks
        let chunks: Vec<_> =
            chunk_epoch_range(epoch_calc, effective_start, end_ts, EPOCH_CHUNK_SIZE_EPOCHS)
                .collect();

        let total_chunks = chunks.len();

        tracing::info!(
            "Processing epoch requestor aggregates: {} epochs in {} chunks",
            total_epochs,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            // Timestamps here should be guaranteed to be on epoch boundaries by the chunking function
            let chunk_start_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_start).unwrap();
            let chunk_end_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_end).unwrap();
            let epoch_count = chunk_end_epoch - chunk_start_epoch + 1;

            tracing::info!(
                "Processing epoch requestor chunk {}/{}: epochs {} to {} ({} epochs)",
                chunk_idx + 1,
                total_chunks,
                chunk_start_epoch,
                chunk_end_epoch,
                epoch_count
            );

            self.indexer.aggregate_epoch_requestor_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let start_time = std::time::Instant::now();
        tracing::info!("Starting per-prover aggregate backfill...");

        self.backfill_hourly_prover_aggregates(start_ts, end_ts).await?;

        self.backfill_daily_prover_aggregates(start_ts, end_ts).await?;

        self.backfill_weekly_prover_aggregates(start_ts, end_ts).await?;

        self.backfill_monthly_prover_aggregates(start_ts, end_ts).await?;

        self.backfill_all_time_prover_aggregates(start_ts, end_ts).await?;

        // Recompute epoch aggregates
        self.backfill_epoch_prover_aggregates(start_ts, end_ts).await?;

        tracing::info!("Per-prover aggregate backfill completed in {:?}", start_time.elapsed());
        Ok(())
    }

    async fn backfill_hourly_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, HOURLY_CHUNK_SIZE_HOURS).collect();

        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        let total_chunks = chunks.len();

        tracing::info!(
            "Processing hourly prover aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing hourly prover chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_hourly_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_daily_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let chunks: Vec<_> = chunk_daily_range(start_ts, end_ts, DAILY_CHUNK_SIZE_DAYS).collect();

        let start_day = get_day_start(start_ts);
        let end_day = get_day_start(end_ts);
        let total_chunks = chunks.len();

        let total_days = {
            let mut count = 0;
            let mut current = start_day;
            while current <= end_day {
                count += 1;
                current = get_next_day(current);
            }
            count
        };

        tracing::info!(
            "Processing daily prover aggregates: {} days in {} chunks",
            total_days,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_day(current);
                }
                count
            };

            tracing::info!(
                "Processing daily prover chunk {}/{}: {} to {} ({} days)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_daily_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_weekly_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let chunks: Vec<_> =
            chunk_weekly_range(start_ts, end_ts, WEEKLY_CHUNK_SIZE_WEEKS).collect();

        let start_week = get_week_start(start_ts);
        let end_week = get_week_start(end_ts);
        let total_chunks = chunks.len();

        let total_weeks = {
            let mut count = 0;
            let mut current = start_week;
            while current <= end_week {
                count += 1;
                current = get_next_week(current);
            }
            count
        };

        tracing::info!(
            "Processing weekly prover aggregates: {} weeks in {} chunks",
            total_weeks,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_week(current);
                }
                count
            };

            tracing::info!(
                "Processing weekly prover chunk {}/{}: {} to {} ({} weeks)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_weekly_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_monthly_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let chunks: Vec<_> =
            chunk_monthly_range(start_ts, end_ts, MONTHLY_CHUNK_SIZE_MONTHS).collect();

        let start_month = get_month_start(start_ts);
        let end_month = get_month_start(end_ts);
        let total_chunks = chunks.len();

        let total_months = {
            let mut count = 0;
            let mut current = start_month;
            while current <= end_month {
                count += 1;
                current = get_next_month(current);
            }
            count
        };

        tracing::info!(
            "Processing monthly prover aggregates: {} months in {} chunks",
            total_months,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = {
                let mut count = 0;
                let mut current = *chunk_start;
                while current <= *chunk_end {
                    count += 1;
                    current = get_next_month(current);
                }
                count
            };

            tracing::info!(
                "Processing monthly prover chunk {}/{}: {} to {} ({} months)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_monthly_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_all_time_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        use crate::market::service::SECONDS_PER_HOUR;
        let chunks: Vec<_> =
            chunk_hourly_range(start_ts, end_ts, ALL_TIME_CHUNK_SIZE_HOURS).collect();

        let start_hour = get_hour_start(start_ts);
        let end_hour = get_hour_start(end_ts);
        let total_chunks = chunks.len();

        tracing::info!(
            "Processing all-time prover aggregates: {} hours in {} chunks",
            (end_hour - start_hour) / SECONDS_PER_HOUR + 1,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let period_count = if *chunk_start == *chunk_end {
                1
            } else {
                (*chunk_end - chunk_start) / SECONDS_PER_HOUR + 1
            };
            tracing::info!(
                "Processing all-time prover chunk {}/{}: {} to {} ({} hours)",
                chunk_idx + 1,
                total_chunks,
                chunk_start,
                chunk_end,
                period_count
            );

            self.indexer.aggregate_all_time_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }

    async fn backfill_epoch_prover_aggregates(
        &mut self,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<(), ServiceError> {
        let epoch_calc = &self.indexer.epoch_calculator;

        // Skip if entire range is before epoch 0
        let epoch0_start = epoch_calc.epoch0_start_time();
        if end_ts < epoch0_start {
            tracing::info!(
                "Skipping epoch prover aggregates: end_ts {} is before epoch 0 start {}",
                end_ts,
                epoch0_start
            );
            return Ok(());
        }

        // Adjust start to epoch 0 start if needed
        let effective_start = start_ts.max(epoch0_start);

        let start_epoch = epoch_calc.get_epoch_for_timestamp(effective_start).unwrap_or(0);
        let end_epoch = epoch_calc.get_epoch_for_timestamp(end_ts).unwrap_or(start_epoch);

        if start_epoch > end_epoch {
            tracing::info!(
                "No epochs to process for provers: start_epoch {} > end_epoch {}",
                start_epoch,
                end_epoch
            );
            return Ok(());
        }

        let total_epochs = end_epoch - start_epoch + 1;

        // Calculate chunks
        let chunks: Vec<_> =
            chunk_epoch_range(epoch_calc, effective_start, end_ts, EPOCH_CHUNK_SIZE_EPOCHS)
                .collect();

        let total_chunks = chunks.len();

        tracing::info!(
            "Processing epoch prover aggregates: {} epochs in {} chunks",
            total_epochs,
            total_chunks
        );

        for (chunk_idx, (chunk_start, chunk_end)) in chunks.iter().enumerate() {
            let chunk_start_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_start).unwrap_or(0);
            let chunk_end_epoch = epoch_calc.get_epoch_for_timestamp(*chunk_end).unwrap_or(0);
            let epoch_count = chunk_end_epoch - chunk_start_epoch + 1;

            tracing::info!(
                "Processing epoch prover chunk {}/{}: epochs {} to {} ({} epochs)",
                chunk_idx + 1,
                total_chunks,
                chunk_start_epoch,
                chunk_end_epoch,
                epoch_count
            );

            self.indexer.aggregate_epoch_prover_data_from(*chunk_start, *chunk_end).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_hourly_range_multiple_chunks() {
        // Test: start=0, end=10800 (4 hours), chunk_size=2
        // Expected: (0, 3600), then (7200, 10800)
        let chunks: Vec<_> = chunk_hourly_range(0, 10800, 2).collect();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], (0, 3600));
        assert_eq!(chunks[1], (7200, 10800));
    }

    #[test]
    fn test_chunk_hourly_range_single_period() {
        // Test: start=0, end=0 (single period)
        // Should create one chunk with start == end (aggregation functions handle this)
        let chunks: Vec<_> = chunk_hourly_range(0, 0, 2).collect();
        assert_eq!(chunks.len(), 1, "Should create one chunk for single period");
        assert_eq!(chunks[0].0, 0, "Chunk should start at 0");
        assert_eq!(
            chunks[0].1, 0,
            "Chunk should end at 0 (aggregation functions handle from_time == to_time)"
        );
    }

    #[test]
    fn test_chunk_hourly_range_non_boundary_start() {
        use crate::market::time_boundaries::{get_hour_start, get_next_hour};

        // Test: start=1800 (30 minutes into hour 0), end=7200 (aligned to hour 2)
        // The function should align start_ts to hour boundary (0) and end_ts to hour boundary (7200)
        // get_hour_start(1800) = 0, get_hour_start(7200) = 7200
        // So it should process from hour 0 to hour 2
        let start_ts = 1800; // 30 minutes into hour 0
        let end_ts = 7200; // Hour 2 boundary

        let chunks: Vec<_> = chunk_hourly_range(start_ts, end_ts, 2).collect();

        // The function should align the start to the hour boundary
        let expected_start = get_hour_start(start_ts);
        let expected_end = get_hour_start(end_ts);
        assert!(!chunks.is_empty(), "Should produce at least one chunk");

        // First chunk should start at the aligned hour boundary
        assert_eq!(
            chunks[0].0, expected_start,
            "First chunk should start at aligned hour boundary {} (from start_ts {})",
            expected_start, start_ts
        );

        // Verify chunks are valid
        for (chunk_start, chunk_end) in &chunks {
            assert!(
                chunk_start <= chunk_end,
                "chunk_start {} must be <= chunk_end {}",
                chunk_start,
                chunk_end
            );
            if expected_start == expected_end {
                assert_eq!(
                    *chunk_end,
                    get_next_hour(expected_end),
                    "Single period chunk should extend to next hour boundary {}",
                    get_next_hour(expected_end)
                );
            } else {
                assert!(
                    *chunk_end <= get_next_hour(expected_end),
                    "chunk_end {} should not exceed next hour boundary {}",
                    chunk_end,
                    get_next_hour(expected_end)
                );
            }
        }
    }

    #[test]
    fn test_chunk_hourly_range_non_boundary_end() {
        use crate::market::time_boundaries::{get_hour_start, get_next_hour};

        // Test: start=0 (aligned), end=9000 (30 minutes into hour 2)
        // The function should align end_ts to hour boundary
        // get_hour_start(9000) = 7200, so it should process from hour 0 to hour 2
        let start_ts = 0;
        let end_ts = 9000; // 30 minutes into hour 2

        let chunks: Vec<_> = chunk_hourly_range(start_ts, end_ts, 2).collect();

        // Should still work without panicking
        assert!(!chunks.is_empty(), "Should produce at least one chunk");

        // The end should be aligned to the hour boundary containing end_ts
        let expected_end = get_hour_start(end_ts);
        let last_chunk_end = chunks.last().unwrap().1;

        // If there's only one period (start == end), chunk_end will extend to get_next(end)
        // to satisfy from_time < to_time validation
        let aligned_start = get_hour_start(start_ts);
        if aligned_start == expected_end {
            // Single period case: chunk_end extends beyond end to satisfy validation
            assert_eq!(
                last_chunk_end,
                get_next_hour(expected_end),
                "Single period chunk should extend to next hour boundary {}",
                get_next_hour(expected_end)
            );
        } else {
            // Multiple periods: chunk_end should be at or before expected_end
            assert!(
                last_chunk_end <= get_next_hour(expected_end),
                "Last chunk end {} should be at or before next hour boundary {}",
                last_chunk_end,
                get_next_hour(expected_end)
            );
        }

        // Verify chunks are valid
        for (chunk_start, chunk_end) in &chunks {
            assert!(
                chunk_start <= chunk_end,
                "chunk_start {} must be <= chunk_end {}",
                chunk_start,
                chunk_end
            );
        }
    }

    #[test]
    fn test_chunk_hourly_range_both_non_boundary() {
        use crate::market::time_boundaries::{get_hour_start, get_next_hour};

        // Test: start=1800 (30 min into hour 0), end=9000 (30 min into hour 2)
        // Both are non-boundary values
        // The function should align both: get_hour_start(1800) = 0, get_hour_start(9000) = 7200
        // So it should process from hour 0 to hour 2
        let start_ts = 1800;
        let end_ts = 9000;

        let chunks: Vec<_> = chunk_hourly_range(start_ts, end_ts, 2).collect();

        // Should still work without panicking
        assert!(!chunks.is_empty(), "Should produce at least one chunk");

        // First chunk should start at the aligned hour boundary
        let expected_start = get_hour_start(start_ts);
        assert_eq!(
            chunks[0].0, expected_start,
            "First chunk should start at aligned hour boundary {} (from start_ts {})",
            expected_start, start_ts
        );

        // Last chunk should end at the aligned hour boundary or extend beyond if single period
        let expected_end = get_hour_start(end_ts);
        let last_chunk_end = chunks.last().unwrap().1;
        if expected_start == expected_end {
            // Single period case: chunk_end extends beyond end to satisfy validation
            assert_eq!(
                last_chunk_end,
                get_next_hour(expected_end),
                "Single period chunk should extend to next hour boundary {}",
                get_next_hour(expected_end)
            );
        } else {
            // Multiple periods: chunk_end should be at or before expected_end
            assert!(
                last_chunk_end <= get_next_hour(expected_end),
                "Last chunk end {} should be at or before next hour boundary {}",
                last_chunk_end,
                get_next_hour(expected_end)
            );
        }

        // Verify chunks are valid
        for (chunk_start, chunk_end) in &chunks {
            assert!(
                chunk_start <= chunk_end,
                "chunk_start {} must be <= chunk_end {}",
                chunk_start,
                chunk_end
            );
        }
    }
}
