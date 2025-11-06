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
use crate::db::market::{RequestStatusType, SlashedStatus};
use crate::market::ServiceError;
use alloy::network::{AnyNetwork, Ethereum};
use alloy::primitives::B256;
use alloy::providers::Provider;
use std::collections::HashSet;

impl<P, ANP> IndexerService<P, ANP>
where
    P: Provider<Ethereum> + 'static + Clone,
    ANP: Provider<AnyNetwork> + 'static + Clone,
{
    pub(super) fn compute_request_status(
        &self,
        req: crate::db::market::RequestComprehensive,
        current_timestamp: u64,
    ) -> crate::db::market::RequestStatus {
        use crate::db::market::RequestStatus;

        let request_status = if req.fulfilled_at.is_some() {
            RequestStatusType::Fulfilled
        } else if current_timestamp > req.expires_at {
            RequestStatusType::Expired
        } else if req.locked_at.is_some() {
            RequestStatusType::Locked
        } else {
            RequestStatusType::Submitted
        };

        let slashed_status = if req.slashed_at.is_some() {
            SlashedStatus::Slashed
        } else if req.locked_at.is_some() && current_timestamp > req.expires_at && req.fulfilled_at.is_none() {
            SlashedStatus::Pending
        } else {
            SlashedStatus::NotApplicable
        };

        let updated_at = [
            req.submitted_at,
            req.locked_at,
            req.fulfilled_at,
            req.slashed_at,
        ]
        .iter()
        .filter_map(|&t| t)
        .max()
        .unwrap_or(req.created_at);

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
            cycles: req.cycles,
            peak_prove_mhz: req.peak_prove_mhz,
            effective_prove_mhz: req.effective_prove_mhz,
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
        tracing::info!("update_request_statuses called with {} request digests for block {}", request_digests.len(), block_number);
        tracing::debug!("Request digests to update: {:?}", request_digests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>());

        let current_timestamp = self.block_timestamp(block_number).await?;

        let start = std::time::Instant::now();

        tracing::debug!("Updating statuses for {} requests based on block {} timestamp {}", request_digests.len(), block_number, current_timestamp);

        if request_digests.is_empty() {
            tracing::info!(
                "update_request_statuses completed in {:?} [0 statuses updated]",
                start.elapsed()
            );
            return Ok(());
        }

        let requests_with_events = self.db.get_requests_comprehensive(&request_digests).await?;
        tracing::info!("get_requests_comprehensive completed in {:?} [{} requests found]", start.elapsed(), requests_with_events.len());

        let request_statuses: Vec<_> = requests_with_events
            .into_iter()
            .map(|req| self.compute_request_status(req, current_timestamp))
            .collect();

        tracing::info!("compute_request_status completed in {:?} [{} statuses computed]", start.elapsed(), request_statuses.len());

        self.db.upsert_request_statuses(&request_statuses).await?;
        tracing::info!("upsert_request_statuses completed in {:?} [{} statuses upserted]", start.elapsed(), request_statuses.len());

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
        tracing::info!("get_newly_expired_requests called with from_timestamp {} to_timestamp {}", from_timestamp, to_timestamp);
        let expired_requests = self.db.find_newly_expired_requests(from_timestamp, to_timestamp).await?;
        tracing::info!("find_newly_expired_requests completed in {:?} [{} expired requests found]", start.elapsed(), expired_requests.len());
        tracing::debug!("Expired requests: {:?}", expired_requests.iter().map(|d| format!("0x{:x}", d)).collect::<Vec<_>>());
        Ok(expired_requests)
    }
}
