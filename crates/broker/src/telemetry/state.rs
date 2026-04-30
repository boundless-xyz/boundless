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

//! In-memory tracking state for requests as they move through the broker
//! pipeline. Owned by [`TelemetryService`](super::TelemetryService) and
//! merged into `RequestEvaluated` events when the per-request lifecycle
//! crosses key milestones.

use std::time::Instant;

use alloy::primitives::{Address, U256};
use boundless_market::telemetry::{CommitmentOutcome, EvalOutcome, RequestEvaluated};
use chrono::Utc;

use super::now_unix;

/// In-memory tracking for a request as it moves through the pipeline.
pub(super) struct InFlightRequest {
    pub(super) received_at_timestamp: u64,
    pub(super) request_id: Option<U256>,
    pub(super) request_digest: String,
    pub(super) fulfillment_type: String,
    pub(super) proof_type: String,
    pub(super) total_cycles: Option<u64>,
    // Pricing fields stored for deferred RequestEvaluated construction.
    pub(super) requestor: Option<Address>,
    pub(super) pricing_outcome: Option<EvalOutcome>,
    pub(super) pricing_skip_code: Option<String>,
    pub(super) pricing_skip_reason: Option<String>,
    pub(super) queue_duration_ms: Option<u64>,
    pub(super) preflight_duration_ms: Option<u64>,
    // Commitment + completion tracking fields.
    pub(super) estimated_proving_time_secs: Option<u64>,
    pub(super) lock_duration_secs: Option<u64>,
    pub(super) committed_at: Option<Instant>,
    pub(super) concurrent_proving_jobs_start: Option<u32>,
    pub(super) stark_proving_secs: Option<f64>,
    pub(super) proof_compression_secs: Option<f64>,
    pub(super) proving_completed_at: Option<Instant>,
    pub(super) set_builder_proving_secs: Option<f64>,
    pub(super) assessor_proving_secs: Option<f64>,
    pub(super) assessor_compression_proof_secs: Option<f64>,
    pub(super) aggregation_completed_at: Option<Instant>,
}

impl InFlightRequest {
    pub(super) fn new() -> Self {
        Self {
            received_at_timestamp: now_unix(),
            request_id: None,
            request_digest: String::new(),
            fulfillment_type: String::new(),
            proof_type: "Unknown".to_string(),
            total_cycles: None,
            requestor: None,
            pricing_outcome: None,
            pricing_skip_code: None,
            pricing_skip_reason: None,
            queue_duration_ms: None,
            preflight_duration_ms: None,
            estimated_proving_time_secs: None,
            lock_duration_secs: None,
            committed_at: None,
            concurrent_proving_jobs_start: None,
            stark_proving_secs: None,
            proof_compression_secs: None,
            proving_completed_at: None,
            set_builder_proving_secs: None,
            assessor_proving_secs: None,
            assessor_compression_proof_secs: None,
            aggregation_completed_at: None,
        }
    }
}

pub(super) struct CommitmentInfo {
    pub(super) outcome: Option<CommitmentOutcome>,
    pub(super) skip_commit_code: Option<String>,
    pub(super) skip_commit_reason: Option<String>,
    pub(super) estimated_proving_time_secs: Option<u64>,
    pub(super) estimated_proving_time_no_load_secs: Option<u64>,
    pub(super) monitor_wait_duration_ms: Option<u64>,
    pub(super) peak_prove_khz: Option<u64>,
    pub(super) max_capacity: Option<u32>,
    pub(super) pending_commitment_count: Option<u32>,
    pub(super) concurrent_proving_jobs: Option<u32>,
    pub(super) lock_duration_secs: Option<u64>,
}

pub(super) fn build_request_evaluated(
    broker_address: Address,
    order_id: String,
    entry: &InFlightRequest,
    commitment: CommitmentInfo,
) -> RequestEvaluated {
    let request_id =
        entry.request_id.map(|id| format!("0x{id:x}")).unwrap_or_else(|| "unknown".to_string());
    RequestEvaluated {
        broker_address,
        order_id,
        request_id,
        request_digest: entry.request_digest.clone(),
        requestor: entry.requestor.unwrap_or(Address::ZERO),
        outcome: entry.pricing_outcome.unwrap_or(EvalOutcome::Skipped),
        skip_code: entry.pricing_skip_code.clone(),
        skip_reason: entry.pricing_skip_reason.clone(),
        total_cycles: entry.total_cycles,
        fulfillment_type: entry.fulfillment_type.clone(),
        queue_duration_ms: entry.queue_duration_ms,
        preflight_duration_ms: entry.preflight_duration_ms,
        received_at_timestamp: entry.received_at_timestamp,
        evaluated_at: Utc::now(),
        commitment_outcome: commitment.outcome,
        commitment_skip_code: commitment.skip_commit_code,
        commitment_skip_reason: commitment.skip_commit_reason,
        estimated_proving_time_secs: commitment.estimated_proving_time_secs,
        estimated_proving_time_no_load_secs: commitment.estimated_proving_time_no_load_secs,
        monitor_wait_duration_ms: commitment.monitor_wait_duration_ms,
        peak_prove_khz: commitment.peak_prove_khz,
        max_capacity: commitment.max_capacity,
        pending_commitment_count: commitment.pending_commitment_count,
        concurrent_proving_jobs: commitment.concurrent_proving_jobs,
        lock_duration_secs: commitment.lock_duration_secs,
    }
}
