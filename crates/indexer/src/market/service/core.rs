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

use super::IndexerService;
use crate::db::market::IndexerDb;
use crate::db::DbObj;
use crate::market::service::execution::execute_requests;
use crate::market::service::IndexerServiceExecutionConfig;
use crate::market::ServiceError;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::B256;
use alloy::providers::Provider;
use std::cmp::min;
use std::collections::HashSet;
use std::time::Duration;

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub async fn run(
        &mut self,
        starting_block: Option<u64>,
        end_block: Option<u64>,
    ) -> Result<(), ServiceError> {
        let mut interval = tokio::time::interval(self.config.interval);

        let mut from_block: u64 = self.starting_block(starting_block).await?;

        // Validate end_block if provided
        if let Some(end) = end_block {
            if end < from_block {
                return Err(ServiceError::Error(anyhow::anyhow!(
                    "End block {} is less than starting block {}",
                    end,
                    from_block
                )));
            }
            let current_block = self.current_block().await?;
            if end > current_block {
                return Err(ServiceError::Error(anyhow::anyhow!(
                    "End block {} is greater than current block {}",
                    end,
                    current_block
                )));
            }
            tracing::info!("Starting indexer at block {} (will stop at block {})", from_block, end);
        } else {
            tracing::info!("Starting indexer at block {}", from_block);
        }

        // Spawn a supervisor task for the cycle count executor
        // if the corresponding configuration is present
        let db_clone = self.db.clone();
        let config_clone = self.config.clone();
        if let Some(execution_config) = config_clone.execution_config {
            // TODO: Currently we assume running the execution task supervisor itself won't panic, so don't keep the handle.
            tokio::spawn(run_execution_task_supervisor(db_clone, execution_config));
        } else {
            tracing::info!("Execution configuration not found, not starting executor task");
        }

        // Spawn the aggregation task supervisor
        let service_clone = self.clone();
        // TODO: Currently we assume running the aggregation task supervisor itself won't panic, so don't keep the handle.
        tokio::spawn(run_aggregation_task_supervisor(service_clone));

        let mut attempt = 0;
        loop {
            interval.tick().await;

            // Determine the maximum block we can process up to
            let max_block = match self.current_block().await {
                Ok(to_block) => {
                    // If end_block is specified, use the minimum of current block and end_block
                    if let Some(end) = end_block {
                        min(to_block, end)
                    } else {
                        to_block
                    }
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!(
                        "Failed to fetch current block: {:?}, attempt number {}",
                        e,
                        attempt
                    );
                    if attempt > self.config.retries {
                        tracing::error!("Aborting after {} consecutive attempts", attempt);
                        return Err(ServiceError::MaxRetries);
                    }
                    continue;
                }
            };

            if max_block < from_block {
                // If we've reached the end_block and processed everything, exit
                if let Some(end) = end_block {
                    if from_block > end {
                        tracing::info!("Reached end block {}, waiting for aggregation to catch up before exiting", end);
                        self.wait_for_aggregation_to_catch_up().await?;
                        tracing::info!("Reached end block {}, exiting", end);
                        return Ok(());
                    }
                }
                continue;
            }

            // cap to at most batch_size blocks per batch, but also respect end_block
            let batch_end = min(max_block, from_block.saturating_add(self.config.batch_size));

            let start = std::time::Instant::now();
            match self.process_blocks(from_block, batch_end).await {
                Ok(_) => {
                    tracing::info!(
                        "process_blocks completed in {:?} [num_blocks={}]",
                        start.elapsed(),
                        batch_end - from_block + 1
                    );
                    attempt = 0;
                    from_block = batch_end + 1;

                    // If we've reached or passed the end_block, exit
                    if let Some(end) = end_block {
                        if from_block > end {
                            tracing::info!("Reached end block {}, waiting for aggregation to catch up before exiting", end);
                            self.wait_for_aggregation_to_catch_up().await?;
                            tracing::info!("Reached end block {}, exiting", end);
                            return Ok(());
                        }
                    }
                }
                Err(e) => match e {
                    // Irrecoverable errors
                    ServiceError::DatabaseError(_)
                    | ServiceError::DatabaseQueryError(_, _)
                    | ServiceError::MaxRetries
                    | ServiceError::RequestNotExpired
                    | ServiceError::Error(_) => {
                        tracing::error!(
                            "Failed to process blocks from {} to {}: {:?}",
                            from_block,
                            batch_end,
                            e
                        );
                        return Err(e);
                    }
                    // Recoverable errors
                    ServiceError::BoundlessMarketError(_)
                    | ServiceError::EventQueryError(_)
                    | ServiceError::RpcError(_) => {
                        attempt += 1;
                        // exponential backoff with a maximum delay of 120 seconds
                        let delay = std::time::Duration::from_secs(2u64.pow(attempt - 1).min(120));
                        tracing::warn!(
                            "Failed to process blocks from {} to {}: {:?}, attempt number {}, retrying in {}s",
                            from_block,
                            batch_end,
                            e,
                            attempt,
                            delay.as_secs()
                        );
                        tokio::time::sleep(delay).await;
                    }
                },
            }

            if attempt > self.config.retries {
                tracing::error!("Aborting after {} consecutive attempts", attempt);
                return Err(ServiceError::MaxRetries);
            }
        }
    }

    async fn process_blocks(&mut self, from: u64, to: u64) -> Result<(), ServiceError> {
        tracing::info!("=== Processing blocks from {} to {} ===", from, to);
        // Fetch all relevant logs once with a single filter
        let logs = self.fetch_logs(from, to).await?;

        // Batch fetch all transactions and blocks in parallel and populate cache
        self.fetch_tx_metadata(&logs, from, to).await?;
        tracing::info!(
            "Blocks being processed timestamp range: {} [{}] to {} [{}]",
            from,
            self.block_timestamp(from).await?,
            to,
            self.block_timestamp(to).await?
        );

        // Process all events
        // Collect touched requests from each process_* call
        // Parallelize onchain and offchain request submission processing
        let (submitted_events_digests, submitted_offchain_digests) = tokio::try_join!(
            self.process_request_submitted_events(&logs),
            self.process_request_submitted_offchain(from, to)
        )?;
        let locked_events_digests = self.process_locked_events(&logs).await?;
        let proof_delivered_digests = self.process_proof_delivered_events(&logs).await?;
        let fulfilled_events_digests = self.process_fulfilled_events(&logs).await?;
        let callback_failed_digests = self.process_callback_failed_events(&logs).await?;
        let slashed_events_digests = self.process_slashed_events(&logs).await?;

        // Request cycle counts for locked and proof fulfilled requests
        // We include fulfilled as it possible to fulfill a request without first locking it.
        let mut cycle_count_requests = HashSet::new();
        cycle_count_requests.extend(locked_events_digests.clone());
        cycle_count_requests.extend(fulfilled_events_digests.clone());
        self.request_cycle_counts(cycle_count_requests).await?;

        // Collect request digests that have been updated in the current block range with cycle count data.
        let cycle_count_updated_digests = self.process_cycle_counts(to).await?;

        // Process deposit/withdrawal events. These don't cause request statuses to be updated, so we don't
        // need to track them as touched requests.
        // Parallelize all deposit/withdrawal event processing
        tokio::try_join!(
            self.process_deposit_events(&logs),
            self.process_withdrawal_events(&logs),
            self.process_collateral_deposit_events(&logs),
            self.process_collateral_withdrawal_events(&logs)
        )?;

        // Find requests that expired during this block range.
        // Note we process the previous block also, to ensure we capture requests that expired during the previous block.
        let expired_requests_from = if from > 0 { from.saturating_sub(1) } else { 0 };
        let expired_requests = self.get_newly_expired_requests(expired_requests_from, to).await?;

        // Merge all touched requests
        let mut touched_requests = HashSet::new();
        touched_requests.extend(submitted_events_digests);
        touched_requests.extend(submitted_offchain_digests);
        touched_requests.extend(locked_events_digests);
        touched_requests.extend(proof_delivered_digests);
        touched_requests.extend(fulfilled_events_digests);
        touched_requests.extend(callback_failed_digests);
        touched_requests.extend(slashed_events_digests);
        touched_requests.extend(expired_requests);
        touched_requests.extend(cycle_count_updated_digests);

        // Update request statuses for touched requests
        self.update_request_statuses(touched_requests, to).await?;

        // Update the last processed block.
        self.update_last_processed_block(to).await?;

        self.clear_in_memory_cache();

        Ok(())
    }

    async fn get_last_processed_block(&self) -> Result<Option<u64>, ServiceError> {
        Ok(self.db.get_last_block().await?)
    }

    async fn update_last_processed_block(&self, block_number: u64) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();
        self.db.set_last_block(block_number).await?;
        tracing::info!("update_last_processed_block completed in {:?}", start.elapsed());
        Ok(())
    }

    async fn current_block(&self) -> Result<u64, ServiceError> {
        Ok(self.boundless_market.instance().provider().get_block_number().await?)
    }

    fn clear_in_memory_cache(&mut self) {
        self.tx_hash_to_metadata.clear();
        self.block_num_to_timestamp.clear();
    }

    /// Process cycle count updates that occurred between the last processed block and the current block timestamp.
    /// Note, we fetch last processed block from the DB, rather than using the "from" block. This is because cycle
    /// counts may have been updated between the last processed block's timestamp and the from block timestamp, that could be missed.
    async fn process_cycle_counts(&mut self, to_block: u64) -> Result<HashSet<B256>, ServiceError> {
        // Get the last processed block to determine the lower bound for cycle count queries.
        // We use last_processed_block's timestamp + 1 as the lower bound to ensure we don't miss
        // cycle counts updated between block timestamps when processing blocks individually.
        let from_timestamp = match self.get_last_processed_block().await? {
            Some(last_block) => {
                // Use last processed block's timestamp + 1 as lower bound
                // This ensures we don't miss cycle counts updated between the last processed block
                // and the current block range
                self.block_timestamp(last_block).await? + 1
            }
            None => {
                // No previous block processed, start from timestamp 0
                0
            }
        };
        let to_timestamp = self.block_timestamp(to_block).await?;
        let request_digests =
            self.db.get_cycle_counts_by_updated_at_range(from_timestamp, to_timestamp).await?;
        tracing::debug!("Found {} cycle counts that were updated in the timestamp range [{}, {}]. Will recompute statuses for these requests.", request_digests.len(), from_timestamp, to_timestamp);
        Ok(request_digests)
    }

    // Return the last processed block from the DB is > 0;
    // otherwise, return the starting_block if set and <= current_block;
    // otherwise, return the current_block.
    async fn starting_block(&self, starting_block: Option<u64>) -> Result<u64, ServiceError> {
        let last_processed = self.get_last_processed_block().await?;
        let current_block = self.current_block().await?;
        Ok(find_starting_block(starting_block, last_processed, current_block))
    }

    // Wait for aggregation task to catch up to the last processed block before exiting.
    // This is only called when end_block is specified and we're about to exit.
    async fn wait_for_aggregation_to_catch_up(&self) -> Result<(), ServiceError> {
        tracing::info!("Waiting for aggregation to catch up before exiting...");

        let poll_interval = self.config.aggregation_interval;

        loop {
            let main_last_block = self.db.get_last_block().await?;
            let aggregation_last_block = self.db.get_last_aggregation_block().await?;

            match (main_last_block, aggregation_last_block) {
                (Some(main_block), Some(agg_block)) => {
                    if agg_block >= main_block {
                        tracing::info!(
                            "Aggregation caught up (main: {}, aggregation: {}), proceeding to exit",
                            main_block,
                            agg_block
                        );
                        return Ok(());
                    }
                }
                (Some(_), None) => {
                    tracing::info!("Aggregation hasn't started yet, continuing to wait");
                }
                (None, _) => {
                    tracing::warn!("No main processing block found, skipping aggregation wait");
                    return Ok(());
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }
}

fn find_starting_block(
    starting_block: Option<u64>,
    last_processed: Option<u64>,
    current_block: u64,
) -> u64 {
    if let Some(last) = last_processed.filter(|&b| b > 0) {
        // Start from last + 1 to avoid reprocessing the last block
        // Cap to current_block if last + 1 exceeds it
        let next_block = last + 1;
        let start = if next_block > current_block { current_block } else { next_block };
        tracing::info!("Using last processed block {} as starting block (next: {})", last, start);
        return start;
    }

    let from = starting_block.unwrap_or(current_block);
    if from > current_block {
        tracing::warn!(
            "Starting block {} is greater than current block {}, defaulting to current block",
            from,
            current_block
        );
        current_block
    } else {
        tracing::info!("Using {} as starting block", from);
        from
    }
}

/// Aggregation task that runs concurrently with event processing.
/// Processes aggregations up to the last processed block from the main event processing task.
async fn compute_aggregates<P, ANP>(service: IndexerService<P, ANP>)
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    let mut interval = tokio::time::interval(service.config.aggregation_interval);

    loop {
        interval.tick().await;

        process_aggregations(&service).await.unwrap();
    }
}

async fn process_aggregations<P, ANP>(service: &IndexerService<P, ANP>) -> Result<(), ServiceError>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    let main_last_block = service.db.get_last_block().await?;
    let aggregation_last_block = service.db.get_last_aggregation_block().await?;

    let from_block = aggregation_last_block.map(|b| b + 1).unwrap_or(0);
    let to_block = match main_last_block {
        Some(block) => block,
        None => {
            tracing::debug!("No main processing block yet, skipping aggregation");
            return Ok(());
        }
    };

    if from_block > to_block {
        tracing::debug!(
            "Aggregation is up to date (aggregation: {}, main: {})",
            aggregation_last_block.unwrap_or(0),
            to_block
        );
        return Ok(());
    }
    tracing::info!("=== Aggregating blocks from {} to {} ===", from_block, to_block);
    let elapsed = std::time::Instant::now();

    let to_timestamp = service.block_timestamp(to_block).await?;

    service.aggregate_hourly_market_data(to_block).await?;
    service.aggregate_daily_market_data(to_block).await?;
    service.aggregate_weekly_market_data(to_block).await?;
    service.aggregate_monthly_market_data(to_block).await?;
    service.aggregate_all_time_market_data(to_block).await?;

    service.aggregate_hourly_requestor_data(to_block).await?;
    service.aggregate_daily_requestor_data(to_block).await?;
    service.aggregate_weekly_requestor_data(to_block).await?;
    service.aggregate_all_time_requestor_data(to_block).await?;

    service.aggregate_hourly_prover_data(to_block).await?;
    service.aggregate_daily_prover_data(to_block).await?;
    service.aggregate_weekly_prover_data(to_block).await?;
    service.aggregate_all_time_prover_data(to_block).await?;

    service.db.set_last_aggregation_block(to_block).await?;
    tracing::info!("Aggregation completed up to block {} (timestamp: {})", to_block, to_timestamp);
    tracing::info!(
        "process_aggregations completed in {:?} [num_blocks={}]",
        elapsed.elapsed(),
        to_block - from_block + 1
    );

    Ok(())
}

/// Simple supervisor that runs the aggregation task and restarts it on failure.
async fn run_aggregation_task_supervisor<P, ANP>(service: IndexerService<P, ANP>)
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    const RESTART_DELAY_SECS: u64 = 5;

    loop {
        tracing::info!("Starting aggregation task");
        let service_clone = service.clone();

        let handle = tokio::spawn(async move {
            compute_aggregates(service_clone).await;
        });

        match handle.await {
            Ok(()) => {
                tracing::error!(
                    "Aggregation task returned unexpectedly, restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
            Err(e) if e.is_panic() => {
                tracing::error!(
                    "Aggregation task panicked, restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
            Err(e) => {
                tracing::error!(
                    "Aggregation task cancelled ({e}), restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(RESTART_DELAY_SECS)).await;
    }
}

/// Simple supervisor that runs the execution task and restarts it on failure.
async fn run_execution_task_supervisor(db: DbObj, config: IndexerServiceExecutionConfig) {
    const RESTART_DELAY_SECS: u64 = 5;

    loop {
        tracing::info!("Starting cycle count execution task");
        let db_clone = db.clone();
        let config_clone = config.clone();

        let handle = tokio::spawn(async move {
            execute_requests(db_clone, config_clone).await;
        });

        match handle.await {
            Ok(()) => {
                tracing::error!(
                    "Cycle count execution task returned unexpectedly, restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
            Err(e) if e.is_panic() => {
                tracing::error!(
                    "Cycle count execution task panicked, restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
            Err(e) => {
                tracing::error!(
                    "Cycle count execution task cancelled ({e}), restarting in {} seconds",
                    RESTART_DELAY_SECS
                );
            }
        }

        tokio::time::sleep(Duration::from_secs(RESTART_DELAY_SECS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_starting_block() {
        // When last_processed is 50, should start from 51 (last + 1)
        let starting_block = Some(100);
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 51);

        // When last_processed is 50, should start from 51 (last + 1)
        let starting_block = None;
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 51);

        // When no last_processed, should use current_block
        let starting_block = None;
        let last_processed = None;
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        // When last_processed is 0 (filtered out), should use current_block
        let starting_block = None;
        let last_processed = Some(0);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        // When starting_block > current_block, should cap to current_block
        let starting_block = Some(200);
        let last_processed = None;
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100);

        // When last_processed is 10 and current_block is 100, should start from 11
        let starting_block = Some(200);
        let last_processed = Some(10);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 11);

        // Edge case: last_processed + 1 equals current_block
        let starting_block = None;
        let last_processed = Some(99);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100); // 100 > 100? No, so should be 100

        // Edge case: last_processed + 1 exceeds current_block
        let starting_block = None;
        let last_processed = Some(100);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100); // 101 > 100, so cap to 100
    }
}
