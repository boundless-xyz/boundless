// Copyright 2025 Boundless Foundation, Inc.
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
use crate::db::market::{IndexerDb, RequestStatusType, SlashedStatus};
use crate::market::ServiceError;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::{B256, U256};
use alloy::providers::Provider;
use boundless_market::contracts::pricing::price_at_time;
use std::collections::HashSet;
use std::str::FromStr;

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub fn compute_request_status(
        &self,
        req: crate::db::market::RequestComprehensive,
        current_timestamp: u64,
    ) -> crate::db::market::RequestStatus {
        use crate::db::market::RequestStatus;

        // Set overall status
        // Note: fulfilled events are only emitted if a proof is delivered before overall request timeout.
        // Fulfilled event is also emitted in the case where lock timesout and the proof is delivered by a secondary prover.
        let request_status = if req.fulfilled_at.is_some() {
            RequestStatusType::Fulfilled
        } else if current_timestamp > req.expires_at
            || (current_timestamp > req.lock_end && req.locked_at.is_none())
        {
            // Note: if the lock has expired and no-one locked, there is no incentive for a prover to deliver a proof, so we just mark as expired.
            RequestStatusType::Expired
        } else if req.locked_at.is_some() {
            RequestStatusType::Locked
        } else {
            RequestStatusType::Submitted
        };

        // Set slashed status
        let slashed_status = if req.slashed_at.is_some() {
            SlashedStatus::Slashed
        } else if req.locked_at.is_some()
            && current_timestamp > req.lock_end
            && (req.lock_prover_delivered_proof_at.is_none()
                || req.lock_prover_delivered_proof_at.unwrap() > req.lock_end)
        {
            SlashedStatus::Pending
        } else {
            SlashedStatus::NotApplicable
        };

        // Set updated_at field to the current timestamp
        let updated_at = current_timestamp;

        // Compute lock price and lock price per cycle if request was locked
        let (lock_price, lock_price_per_cycle) = if let Some(locked_at) = req.locked_at {
            let min_price = U256::from_str(&req.min_price).ok();
            let max_price = U256::from_str(&req.max_price).ok();

            if let (Some(min_price), Some(max_price)) = (min_price, max_price) {
                let lock_timeout = req.lock_end.saturating_sub(req.ramp_up_start);
                let lock_price_u256 = price_at_time(
                    min_price,
                    max_price,
                    req.ramp_up_start,
                    req.ramp_up_period as u32,
                    lock_timeout as u32,
                    locked_at,
                );

                let lock_price_str = format!("{:0>78}", lock_price_u256.to_string());

                let lock_price_per_cycle_str = if let Some(program_cycles) = req.program_cycles {
                    if program_cycles > 0 {
                        let price_per_cycle = lock_price_u256 / U256::from(program_cycles);
                        Some(format!("{:0>78}", price_per_cycle.to_string()))
                    } else {
                        None
                    }
                } else {
                    None
                };

                (Some(lock_price_str), lock_price_per_cycle_str)
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        RequestStatus {
            request_digest: req.request_digest,
            request_id: req.request_id,
            request_status,
            slashed_status,
            source: req.source,
            client_address: req.client_address,
            lock_prover_address: req.lock_prover_address,
            fulfill_prover_address: req.fulfill_prover_address,
            created_at: req.created_at,
            updated_at,
            locked_at: req.locked_at,
            fulfilled_at: req.fulfilled_at,
            slashed_at: req.slashed_at,
            lock_prover_delivered_proof_at: req.lock_prover_delivered_proof_at,
            submit_block: req.submit_block,
            lock_block: req.lock_block,
            fulfill_block: req.fulfill_block,
            slashed_block: req.slashed_block,
            min_price: req.min_price,
            max_price: req.max_price,
            lock_collateral: req.lock_collateral,
            ramp_up_start: req.ramp_up_start,
            ramp_up_period: req.ramp_up_period,
            expires_at: req.expires_at,
            lock_end: req.lock_end,
            slash_recipient: req.slash_recipient,
            slash_transferred_amount: req.slash_transferred_amount,
            slash_burned_amount: req.slash_burned_amount,
            program_cycles: req.program_cycles,
            total_cycles: req.total_cycles,
            peak_prove_mhz: req.peak_prove_mhz,
            effective_prove_mhz: req.effective_prove_mhz,
            cycle_status: req.cycle_status,
            lock_price,
            lock_price_per_cycle,
            submit_tx_hash: req.submit_tx_hash,
            lock_tx_hash: req.lock_tx_hash,
            fulfill_tx_hash: req.fulfill_tx_hash,
            slash_tx_hash: req.slash_tx_hash,
            image_id: req.image_id,
            image_url: req.image_url,
            selector: req.selector,
            predicate_type: req.predicate_type,
            predicate_data: req.predicate_data,
            input_type: req.input_type,
            input_data: req.input_data,
            fulfill_journal: req.fulfill_journal,
            fulfill_seal: req.fulfill_seal,
        }
    }

    pub(super) async fn update_request_statuses(
        &mut self,
        request_digests: HashSet<B256>,
        block_number: u64,
    ) -> Result<(), ServiceError> {
        tracing::debug!(
            "Request digests to update: {:?}",
            request_digests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>()
        );

        let current_timestamp = self.block_timestamp(block_number).await?;

        let start = std::time::Instant::now();

        tracing::debug!(
            "Updating statuses for {} requests based on block {} timestamp {}",
            request_digests.len(),
            block_number,
            current_timestamp
        );

        if request_digests.is_empty() {
            tracing::info!(
                "update_request_statuses completed in {:?} [0 statuses updated]",
                start.elapsed()
            );
            return Ok(());
        }

        let start_get_requests_comprehensive = std::time::Instant::now();
        let requests_with_events = self.db.get_requests_comprehensive(&request_digests).await?;
        tracing::info!("get_requests_comprehensive completed in {:?} [queried with {} digests, {} requests found]", start_get_requests_comprehensive.elapsed(), request_digests.len(), requests_with_events.len());

        let start_compute_request_status = std::time::Instant::now();
        let request_statuses: Vec<_> = requests_with_events
            .into_iter()
            .map(|req| self.compute_request_status(req, current_timestamp))
            .collect();

        tracing::info!(
            "compute_request_status completed in {:?} [{} statuses computed]",
            start_compute_request_status.elapsed(),
            request_statuses.len()
        );

        let start_upsert_request_statuses = std::time::Instant::now();
        self.db.upsert_request_statuses(&request_statuses).await?;
        tracing::info!(
            "upsert_request_statuses completed in {:?} [{} statuses upserted]",
            start_upsert_request_statuses.elapsed(),
            request_statuses.len()
        );
        for request_status in request_statuses.clone() {
            tracing::debug!("Updated request status for request_id: 0x{:x}, digest: 0x{:x}. New status: {:?}. Locked at: {:?}. Fulfilled at: {:?}. Slashed at: {:?}.", request_status.request_id, request_status.request_digest, request_status.request_status, request_status.locked_at, request_status.fulfilled_at, request_status.slashed_at);
        }

        tracing::info!(
            "update_request_statuses completed in {:?} [{} statuses updated]",
            start.elapsed(),
            request_statuses.len()
        );

        Ok(())
    }

    pub(super) async fn get_newly_expired_requests(
        &mut self,
        from_block: u64,
        to_block: u64,
    ) -> Result<HashSet<B256>, ServiceError> {
        let start = std::time::Instant::now();
        let from_timestamp = self.block_timestamp(from_block).await?;
        let to_timestamp = self.block_timestamp(to_block).await?;

        // Note: to_timestamp is typically "now", so we use a half-open range to avoid including requests that
        // with timeout == now, as they are not expired yet.
        let expired_requests =
            self.db.find_newly_expired_requests(from_timestamp, to_timestamp).await?;

        tracing::info!(
            "find_newly_expired_requests completed in {:?} [{} expired requests found]",
            start.elapsed(),
            expired_requests.len()
        );
        tracing::debug!(
            "Expired requests digests: {:?}",
            expired_requests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>()
        );
        Ok(expired_requests)
    }
}
