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

use super::IndexerService;
use crate::db::market::IndexerDb;
use crate::market::ServiceError;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::B256;
use alloy::providers::Provider;
use std::cmp::min;
use std::collections::HashSet;

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
                    tracing::info!("process_blocks completed in {:?}", start.elapsed());
                    attempt = 0;
                    from_block = batch_end + 1;

                    // If we've reached or passed the end_block, exit
                    if let Some(end) = end_block {
                        if from_block > end {
                            tracing::info!("Reached end block {}, exiting", end);
                            return Ok(());
                        }
                    }
                }
                Err(e) => match e {
                    // Irrecoverable errors
                    ServiceError::DatabaseError(_)
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
        tracing::info!("Blocks being processed timestamp range: {} [{}] to {} [{}]", from, self.block_timestamp(from).await?, to, self.block_timestamp(to).await?);

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
        let cycle_count_updated_digests = self.process_cycle_counts(from, to).await?;

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

        // Aggregate market data.
        // Note: Aggregations use the result of update_request_statuses, so we need to run them after.
        self.aggregate_hourly_market_data(to).await?;
        self.aggregate_daily_market_data(to).await?;
        self.aggregate_weekly_market_data(to).await?;
        self.aggregate_monthly_market_data(to).await?;
        self.aggregate_all_time_market_data(to).await?;

        // Aggregate per-requestor data.
        self.aggregate_hourly_requestor_data(to).await?;
        self.aggregate_daily_requestor_data(to).await?;
        self.aggregate_weekly_requestor_data(to).await?;
        // TODO: Debug speed issues.
        // self.aggregate_monthly_requestor_data(to).await?;
        self.aggregate_all_time_requestor_data(to).await?;

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

    async fn process_cycle_counts(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<HashSet<B256>, ServiceError> {
        let from_timestamp = self.block_timestamp(from_block).await?;
        let to_timestamp = self.block_timestamp(to_block).await?;
        let request_digests = self
            .db
            .get_cycle_counts_by_updated_at_range(from_timestamp, to_timestamp)
            .await?;
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
}

fn find_starting_block(
    starting_block: Option<u64>,
    last_processed: Option<u64>,
    current_block: u64,
) -> u64 {
    if let Some(last) = last_processed.filter(|&b| b > 0) {
        tracing::info!("Using last processed block {} as starting block", last);
        return last;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_find_starting_block() {
        let starting_block = Some(100);
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = Some(50);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 50);

        let starting_block = None;
        let last_processed = None;
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = None;
        let last_processed = Some(0);
        let current_block = 200;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 200);

        let starting_block = Some(200);
        let last_processed = None;
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 100);

        let starting_block = Some(200);
        let last_processed = Some(10);
        let current_block = 100;
        let block = find_starting_block(starting_block, last_processed, current_block);
        assert_eq!(block, 10);
    }
}
