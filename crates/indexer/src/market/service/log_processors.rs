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
use crate::db::events::EventsDb;
use crate::db::market::{IndexerDb, TxMetadata};
use crate::market::ServiceError;
use ::boundless_market::contracts::{IBoundlessMarket, RequestId};
use ::boundless_market::order_stream_client::SortDirection;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::B256;
use alloy::providers::Provider;
use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use anyhow::{anyhow, Context};
use std::collections::HashSet;
use std::time::Duration;

async fn list_orders_v2_with_retry(
    order_stream_client: &::boundless_market::order_stream_client::OrderStreamClient,
    cursor: Option<String>,
    limit: Option<u64>,
    sort: Option<SortDirection>,
    before: Option<chrono::DateTime<chrono::Utc>>,
    after: Option<chrono::DateTime<chrono::Utc>>,
) -> Result<::boundless_market::order_stream_client::ListOrdersV2Response, anyhow::Error> {
    const MAX_RETRIES: u32 = 3;
    const INITIAL_DELAY_MS: u64 = 1000;
    const BACKOFF_MULTIPLIER: u64 = 2;
    const MAX_DELAY_MS: u64 = 30000;

    let mut last_error = None;

    for attempt in 0..=MAX_RETRIES {
        match order_stream_client.list_orders_v2(cursor.clone(), limit, sort, before, after).await {
            Ok(resp) => {
                if attempt > 0 {
                    tracing::info!("Successfully fetched orders after {} retries", attempt);
                }
                return Ok(resp);
            }
            Err(e) => {
                last_error = Some(e);
                if attempt < MAX_RETRIES {
                    let delay_ms = std::cmp::min(
                        INITIAL_DELAY_MS * BACKOFF_MULTIPLIER.pow(attempt),
                        MAX_DELAY_MS,
                    );
                    tracing::warn!(
                        "Failed to fetch orders (attempt {}/{}): {}. Retrying in {}ms",
                        attempt + 1,
                        MAX_RETRIES + 1,
                        last_error.as_ref().unwrap(),
                        delay_ms
                    );
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    }

    Err(anyhow!("Failed to fetch orders after {} retries: {}", MAX_RETRIES, last_error.unwrap()))
}

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) async fn process_request_submitted_events(
        &self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestSubmitted events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} request submitted events", logs_len);

        // Collect events and proof requests for batch insert
        let mut submitted_events = Vec::new();
        let mut proof_requests = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestSubmitted>()
                .context("Failed to decode RequestSubmitted log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;

            let request = event.request.clone();

            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            tracing::debug!(
                "Processing request submitted event for request: 0x{:x}, digest: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                request_digest,
                metadata.block_number,
                metadata.block_timestamp,
            );

            // Collect proof request for batch insert
            proof_requests.push((
                request_digest,
                request,
                metadata,
                "onchain".to_string(),
                metadata.block_timestamp,
            ));

            // Collect event for batch insert
            submitted_events.push((request_digest, event.requestId, metadata));

            tracing::debug!(
                "Adding request_digest to touched_requests: 0x{:x} for request: 0x{:x}",
                request_digest,
                event.requestId
            );
            touched_requests.insert(request_digest);
        }

        // Batch insert all collected proof requests
        if !proof_requests.is_empty() {
            self.db.add_proof_requests(&proof_requests).await?;
        }

        // Batch insert all collected events
        if !submitted_events.is_empty() {
            self.db.add_request_submitted_events(&submitted_events).await?;
        }

        tracing::info!(
            "process_request_submitted_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        tracing::debug!(
            "Touched requests: {:?}",
            touched_requests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>()
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_request_submitted_offchain(
        &self,
        from_block: u64,
        to_block: u64,
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Get the block timestamp for the max block to use as upper bound for order filtering
        let from_block_timestamp = self.block_timestamp(from_block).await?;
        let to_block_timestamp = self.block_timestamp(to_block).await?;
        tracing::debug!(
            "Processing offchain orders from block {} (timestamp: {}) to block {} (timestamp: {})",
            from_block,
            from_block_timestamp,
            to_block,
            to_block_timestamp
        );

        let Some(order_stream_client) = &self.order_stream_client else {
            return Ok(touched_requests);
        };

        let last_processed = self.db.get_last_order_stream_timestamp().await?;

        tracing::debug!(
            "Processing offchain orders up to block {} (timestamp: {}). Last processed timestamp: {:?}",
            to_block,
            to_block_timestamp,
            last_processed
        );

        const MAX_ORDERS_PER_BATCH: u64 = 1000;
        let mut cursor: Option<String> = None;
        let mut latest_timestamp = last_processed;
        let mut total_orders = 0;

        // Convert end block timestamp to DateTime and add 1 second to include orders at block_timestamp
        // (before parameter is exclusive, uses <)
        let before_timestamp =
            chrono::DateTime::from_timestamp(to_block_timestamp as i64 + 1, 0)
                .ok_or_else(|| ServiceError::Error(anyhow!("Invalid block timestamp")))?;

        // Convert start block timestamp to DateTime and subtract 1 second to include orders at block_timestamp
        let after_timestamp = chrono::DateTime::from_timestamp(from_block_timestamp as i64 - 1, 0)
            .ok_or_else(|| ServiceError::Error(anyhow!("Invalid block timestamp")))?;

        // Collect all proof requests for batch insert
        let mut all_proof_requests = Vec::new();

        loop {
            let response = list_orders_v2_with_retry(
                order_stream_client,
                cursor.clone(),
                Some(MAX_ORDERS_PER_BATCH),
                Some(SortDirection::Asc),
                Some(before_timestamp),
                Some(after_timestamp),
            )
            .await
            .map_err(ServiceError::Error)?;

            if response.orders.is_empty() {
                break;
            }

            tracing::debug!("Fetched {} orders from order stream", response.orders.len());

            // Collect all request digests to check in batch
            let request_digests: Vec<B256> =
                response.orders.iter().map(|order_data| order_data.order.request_digest).collect();

            // Batch check which requests already exist
            let existing_digests = self.db.has_proof_requests(&request_digests).await?;

            // Collect proof requests from this batch, skipping existing ones
            let mut batch_proof_requests = Vec::new();

            for order_data in &response.orders {
                let request = &order_data.order.request;
                let request_digest = order_data.order.request_digest;

                if existing_digests.contains(&request_digest) {
                    tracing::warn!(
                        "Skipping order 0x{:x} - already exists in database",
                        request.id
                    );
                    continue;
                }

                let request_id = RequestId::from_lossy(request.id);
                let created_at = order_data.created_at;
                let submission_timestamp = created_at.timestamp() as u64;
                // Off-chain orders have no associated on-chain transaction, so use sentinel values:
                // tx_hash = B256::ZERO, block_number = 0, block_timestamp = 0, transaction_index = 0
                let metadata = TxMetadata::new(B256::ZERO, request_id.addr, 0, 0, 0);

                tracing::debug!("Processing request submitted offchain event for request: 0x{:x}, digest: 0x{:x} [block: {}, timestamp: {}]", request.id, request_digest, metadata.block_number, metadata.block_timestamp);

                // Collect for batch insert
                batch_proof_requests.push((
                    request_digest,
                    request.clone(),
                    metadata,
                    "offchain".to_string(),
                    submission_timestamp,
                ));

                touched_requests.insert(request_digest);
                if latest_timestamp.is_none() || latest_timestamp.unwrap() < created_at {
                    latest_timestamp = Some(created_at);
                }

                total_orders += 1;
            }

            // Add this batch to the overall collection
            all_proof_requests.extend(batch_proof_requests);

            // Check if there are more pages to fetch
            if !response.has_more {
                break;
            }

            // Update cursor for next page
            cursor = response.next_cursor;
        }

        // Batch insert all collected proof requests
        if !all_proof_requests.is_empty() {
            tracing::info!("Batch inserting {} offchain proof requests", all_proof_requests.len());
            self.db.add_proof_requests(&all_proof_requests).await?;
        }

        if total_orders > 0 {
            tracing::info!("Processed {} offchain orders from order stream between block {} (timestamp: {}) and block {} (timestamp: {})", total_orders, from_block, from_block_timestamp, to_block, to_block_timestamp);
        }

        if let Some(ts) = latest_timestamp {
            self.db.set_last_order_stream_timestamp(ts).await?;
        }

        tracing::info!(
            "process_request_submitted_offchain completed in {:?} [found: {}]",
            start.elapsed(),
            total_orders
        );
        tracing::debug!(
            "Touched requests (offchain): {:?}",
            touched_requests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>()
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_locked_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestLocked events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestLocked::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} locked events", logs_len);

        // Collect events for batch insert
        let mut locked_events = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestLocked>()
                .context("Failed to decode RequestLocked log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;

            // Get the request and calculate its digest
            let request = event.request.clone();
            let request_digest = request
                .signing_hash(self.domain.verifying_contract, self.domain.chain_id)
                .context(anyhow!(
                    "Failed to compute request digest for request: 0x{:x}",
                    event.requestId
                ))?;

            tracing::debug!(
                "Processing request locked event for request: 0x{:x}, digest: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                request_digest,
                metadata.block_number,
                metadata.block_timestamp,
            );

            locked_events.push((request_digest, event.requestId, event.prover, metadata));
            touched_requests.insert(request_digest);
        }

        // Batch insert all locked events
        if !locked_events.is_empty() {
            self.db.add_request_locked_events(&locked_events).await?;
        }

        tracing::info!(
            "process_locked_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_proof_delivered_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for ProofDelivered events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::ProofDelivered::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} proof delivered events", logs_len);

        // Collect events and proofs for batch insert
        let mut proof_delivered_events = Vec::new();
        let mut proofs = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::ProofDelivered>()
                .context("Failed to decode ProofDelivered log")?;
            let event = decoded.inner.data;
            let request_digest = event.fulfillment.requestDigest;

            let metadata = self.get_tx_metadata(log.clone()).await?;

            tracing::debug!(
                "Processing proof delivered event for request: 0x{:x}, digest: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                request_digest,
                metadata.block_number,
                metadata.block_timestamp
            );

            proof_delivered_events.push((request_digest, event.requestId, event.prover, metadata));

            // Collect proof for batch insert
            proofs.push((event.fulfillment, event.prover, metadata));

            touched_requests.insert(request_digest);
        }

        // Batch insert all proofs
        if !proofs.is_empty() {
            self.db.add_proofs(&proofs).await?;
        }

        // Batch insert all proof delivered events
        if !proof_delivered_events.is_empty() {
            self.db.add_proof_delivered_events(&proof_delivered_events).await?;
        }

        tracing::info!(
            "process_proof_delivered_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_fulfilled_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for RequestFulfilled events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::RequestFulfilled::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} fulfilled events", logs_len);

        // Collect events for batch insert
        let mut fulfilled_events = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::RequestFulfilled>()
                .context("Failed to decode RequestFulfilled log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;

            tracing::debug!(
                "Processing fulfilled event for request: 0x{:x}, digest: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                event.requestDigest,
                metadata.block_number,
                metadata.block_timestamp
            );

            fulfilled_events.push((event.requestDigest, event.requestId, event.prover, metadata));
            touched_requests.insert(event.requestDigest);
        }

        // Batch insert all fulfilled events
        if !fulfilled_events.is_empty() {
            self.db.add_request_fulfilled_events(&fulfilled_events).await?;
        }

        tracing::info!(
            "process_fulfilled_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_slashed_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for ProverSlashed events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::ProverSlashed::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} slashed events", logs_len);

        // Collect events and request_ids for batch insert and batch query
        let mut slashed_events = Vec::new();
        let mut request_ids = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::ProverSlashed>()
                .context("Failed to decode ProverSlashed log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing slashed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            // Collect event for batch insert
            slashed_events.push((
                event.requestId,
                event.collateralBurned,
                event.collateralTransferred,
                event.collateralRecipient,
                metadata,
            ));

            // Collect request_id for batch query
            request_ids.push(event.requestId);
        }

        // Batch insert all slashed events
        if !slashed_events.is_empty() {
            self.db.add_prover_slashed_events(&slashed_events).await?;
        }

        // Batch query all request digests
        if !request_ids.is_empty() {
            let request_digests_map =
                self.db.get_request_digests_by_request_ids(&request_ids).await?;
            for digests in request_digests_map.values() {
                for digest in digests {
                    touched_requests.insert(*digest);
                }
            }
        }

        tracing::info!(
            "process_slashed_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(touched_requests)
    }

    pub(super) async fn process_deposit_events(
        &self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Filter logs for Deposit events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::Deposit::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} deposit events", logs_len);

        // Collect deposits for batch insert
        let mut deposits = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::Deposit>()
                .context("Failed to decode Deposit log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );

            deposits.push((event.account, event.value, metadata));
        }

        // Batch insert all deposit events
        if !deposits.is_empty() {
            self.db.add_deposit_events(&deposits).await?;
        }

        tracing::info!(
            "process_deposit_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(())
    }

    pub(super) async fn process_withdrawal_events(
        &self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Filter logs for Withdrawal events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::Withdrawal::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} withdrawal events", logs_len);

        // Collect withdrawals for batch insert
        let mut withdrawals = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::Withdrawal>()
                .context("Failed to decode Withdrawal log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );

            withdrawals.push((event.account, event.value, metadata));
        }

        // Batch insert all withdrawal events
        if !withdrawals.is_empty() {
            self.db.add_withdrawal_events(&withdrawals).await?;
        }

        tracing::info!(
            "process_withdrawal_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(())
    }

    pub(super) async fn process_collateral_deposit_events(
        &self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Filter logs for CollateralDeposit events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CollateralDeposit::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} collateral deposit events", logs_len);

        // Collect collateral deposits for batch insert
        let mut collateral_deposits = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CollateralDeposit>()
                .context("Failed to decode CollateralDeposit log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing collateral deposit event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );

            collateral_deposits.push((event.account, event.value, metadata));
        }

        // Batch insert all collateral deposit events
        if !collateral_deposits.is_empty() {
            self.db.add_collateral_deposit_events(&collateral_deposits).await?;
        }

        tracing::info!(
            "process_collateral_deposit_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(())
    }

    pub(super) async fn process_collateral_withdrawal_events(
        &self,
        all_logs: &[Log],
    ) -> Result<(), ServiceError> {
        let start = std::time::Instant::now();

        // Filter logs for CollateralWithdrawal events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CollateralWithdrawal::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} collateral withdrawal events", logs_len);

        // Collect collateral withdrawals for batch insert
        let mut collateral_withdrawals = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CollateralWithdrawal>()
                .context("Failed to decode CollateralWithdrawal log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing collateral withdrawal event for account: 0x{:x} [block: {}, timestamp: {}]",
                event.account,
                metadata.block_number,
                metadata.block_timestamp
            );

            collateral_withdrawals.push((event.account, event.value, metadata));
        }

        // Batch insert all collateral withdrawal events
        if !collateral_withdrawals.is_empty() {
            self.db.add_collateral_withdrawal_events(&collateral_withdrawals).await?;
        }

        tracing::info!(
            "process_collateral_withdrawal_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(())
    }

    pub(super) async fn process_callback_failed_events(
        &mut self,
        all_logs: &[Log],
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let mut touched_requests = HashSet::new();

        // Filter logs for CallbackFailed events
        let logs: Vec<_> = all_logs
            .iter()
            .filter(|log| {
                log.topic0()
                    .map(|t| t == &IBoundlessMarket::CallbackFailed::SIGNATURE_HASH)
                    .unwrap_or(false)
            })
            .collect();

        let logs_len = logs.len();

        tracing::debug!("Found {} callback failed events", logs_len);

        // Collect events and request_ids for batch insert and batch query
        let mut callback_failed_events = Vec::new();
        let mut request_ids = Vec::new();

        for log in logs {
            let decoded = log
                .log_decode::<IBoundlessMarket::CallbackFailed>()
                .context("Failed to decode CallbackFailed log")?;
            let event = decoded.inner.data;

            let metadata = self.get_tx_metadata(log.clone()).await?;
            tracing::debug!(
                "Processing callback failed event for request: 0x{:x} [block: {}, timestamp: {}]",
                event.requestId,
                metadata.block_number,
                metadata.block_timestamp
            );

            // Collect event for batch insert
            callback_failed_events.push((
                event.requestId,
                event.callback,
                event.error.to_vec(),
                metadata,
            ));

            // Collect request_id for batch query
            request_ids.push(event.requestId);
        }

        // Batch insert all callback failed events
        if !callback_failed_events.is_empty() {
            self.db.add_callback_failed_events(&callback_failed_events).await?;
        }

        // Batch query all request digests
        if !request_ids.is_empty() {
            let request_digests_map =
                self.db.get_request_digests_by_request_ids(&request_ids).await?;
            for digests in request_digests_map.values() {
                for digest in digests {
                    touched_requests.insert(*digest);
                }
            }
        }

        tracing::info!(
            "process_callback_failed_events completed in {:?} [found: {}]",
            start.elapsed(),
            logs_len
        );
        Ok(touched_requests)
    }
}
