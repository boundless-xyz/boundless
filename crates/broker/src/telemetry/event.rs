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

//! Internal telemetry events emitted by broker services to the
//! [`TelemetryHandle`](super::TelemetryHandle). These events are then
//! processed/merged by the [`TelemetryService`](super::TelemetryService)
//! into `RequestEvaluated` and `RequestCompleted` events. Those shared
//! types live in the boundless-market crate.

use std::time::Instant;

use alloy::primitives::{Address, U256};
use boundless_market::telemetry::{CompletionOutcome, EvalOutcome};

#[derive(Debug)]
pub(crate) enum TelemetryEvent {
    // Emitted by OrderPicker immediately after pricing an order (preflight + price evaluation).
    OrderPricing {
        /// Composite order ID: "0x{request_id}-{request_digest}-{fulfillment_type}".
        order_id: String,
        /// On-chain request ID.
        request_id: U256,
        /// Signing hash (digest) of the proof request, hex-encoded.
        request_digest: String,
        /// Ethereum address of the requestor who submitted the proof request.
        requestor: Address,
        /// Pricing outcome: Locked, FulfillAfterLockExpire, or Skipped.
        outcome: EvalOutcome,
        /// Structured skip code (e.g. "[B-OP-001]"), set when outcome is Skipped.
        skip_code: Option<String>,
        /// Human-readable skip reason, set when outcome is Skipped.
        skip_reason: Option<String>,
        /// Total execution cycles from preflight. None if preflight was skipped.
        total_cycles: Option<u64>,
        /// "LockAndFulfill", "FulfillAfterLockExpire", or "FulfillWithoutLocking".
        fulfillment_type: String,
        /// "Groth16", "Blake3Groth16", or "Merkle". Derived from the request's selector.
        proof_type: String,
        /// Time spent in the pending queue before a preflight slot was available (ms).
        /// Calculated as: (now - received_at_timestamp - preflight_duration) * 1000.
        queue_duration_ms: Option<u64>,
        /// Time spent running preflight (upload + execution), in milliseconds.
        /// Calculated as: (now - received_at_timestamp - queue_duration) * 1000.
        preflight_duration_ms: Option<u64>,
        /// Unix timestamp (seconds) when the broker first received this request.
        received_at_timestamp: u64,
    },
    // Emitted by OrderLocker when it makes its final commit/drop decision for an order.
    // For LockAndFulfill orders: emitted after the lock tx succeeds or fails.
    // For FulfillAfterLockExpire orders: emitted immediately when the order enters the pipeline.
    OrderCommitment {
        /// Composite order ID.
        order_id: String,
        /// Whether the order was committed to the proving pipeline.
        committed: bool,
        /// Wall-clock instant when the commitment was recorded. Used to compute proving
        /// duration later. Set only when committed=true.
        committed_at: Option<Instant>,
        /// Number of orders already committed in the DB at the moment the commit/drop
        /// decision is made. Queried via db.get_committed_orders().len().
        concurrent_proving_jobs: u32,
        /// Estimated proving time in seconds with current load factored in. Accounts for
        /// all currently committed orders ahead in the queue.
        estimated_proving_time_secs: Option<u64>,
        /// Estimated proving time in seconds ignoring current load (as if no other orders
        /// were queued).
        estimated_proving_time_no_load_secs: Option<u64>,
        /// Time from when the order was priced (entered monitor cache) to when the
        /// commit/drop decision was made, in milliseconds. Calculated as:
        /// (now_timestamp() - order.priced_at_timestamp) * 1000.
        /// None if priced_at_timestamp was not set.
        monitor_wait_duration_ms: Option<u64>,
        /// Peak proving speed from broker config (kHz). Passed through from
        /// config.market.peak_prove_khz.
        peak_prove_khz: Option<u64>,
        /// Max concurrent proofs from broker config. Passed through from
        /// config.market.max_concurrent_proofs.
        max_capacity: Option<u32>,
        /// Number of orders in the monitor caches (lock_and_prove_cache + prove_cache)
        /// waiting to be committed, excluding the current order. Calculated as:
        /// (lock_and_prove_cache.entry_count() + prove_cache.entry_count()).saturating_sub(1).
        pending_commitment_count: u32,
        /// Structured skip code (e.g. "[B-OL-001]"), set when the order is dropped.
        skip_commit_code: Option<String>,
        /// Human-readable reason the order was dropped at commitment.
        skip_commit_reason: Option<String>,
        /// Wall-clock instant when the lock transaction was submitted. Set only for
        /// LockAndFulfill orders that attempt a lock. Used with committed_at to
        /// compute lock_duration_secs.
        lock_submitted_at: Option<Instant>,
    },
    // Emitted by the proving pipeline when STARK proving (and optional Groth16 compression)
    // completes for an order.
    ApplicationProvingCompleted {
        /// Composite order ID.
        order_id: String,
        /// Total execution cycles reported by the prover. May differ from the preflight
        /// estimate if the guest behaved differently.
        total_cycles: Option<u64>,
        /// Wall-clock seconds for the STARK proof (session creation through proof completion).
        stark_proving_secs: Option<f64>,
        /// Wall-clock seconds for Groth16/Blake3Groth16 compression of the individual proof.
        /// None for merkle inclusion orders (they skip per-order compression).
        proof_compression_secs: Option<f64>,
    },
    // Emitted by the batching pipeline when aggregation completes (set builder + assessor
    // + aggregation Groth16 compression).
    AggregationCompleted {
        /// Composite order ID.
        order_id: String,
        /// Wall-clock seconds for the set-builder STARK proof that merges claim digests.
        set_builder_proving_secs: Option<f64>,
        /// Wall-clock seconds for the assessor STARK proof that validates batch fulfillments.
        assessor_proving_secs: Option<f64>,
        /// Wall-clock seconds for compressing the aggregation STARK proof into Groth16.
        assessor_compression_proof_secs: Option<f64>,
    },
    // Emitted when the fulfill transaction is confirmed on-chain.
    Fulfilled {
        /// Composite order ID.
        order_id: String,
    },
    // Emitted when the order fails at any stage after commitment.
    Failed {
        /// Composite order ID.
        order_id: String,
        /// Structured error code (e.g. "[B-PRO-501]").
        error_code: String,
        /// Human-readable error description.
        error_reason: String,
        /// Terminal outcome for this failure.
        outcome: CompletionOutcome,
    },
}
