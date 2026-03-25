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

//! Telemetry types shared between the broker (sender) and order-stream (receiver).

use alloy_primitives::Address;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// API path for broker identity heartbeats.
pub const HEARTBEAT_PATH: &str = "/api/v2/heartbeats";

/// API path for request evaluation/completion heartbeats.
pub const HEARTBEAT_REQUESTS_PATH: &str = "/api/v2/heartbeats/requests";

/// Periodic broker identity heartbeat (sent hourly).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct BrokerHeartbeat {
    /// Ethereum address of the broker.
    pub broker_address: Address,
    /// Broker config serialized as opaque JSON.
    pub config: serde_json::Value,
    /// Number of orders currently committed (in the proving pipeline).
    pub committed_orders_count: u32,
    /// Number of orders waiting for a preflight slot.
    pub pending_preflight_count: u32,
    /// Broker software version.
    pub version: String,
    /// Seconds since the broker process started.
    pub uptime_secs: u64,
    /// Number of telemetry events dropped due to channel backpressure.
    pub events_dropped: u64,
    /// Timestamp when this heartbeat was generated.
    pub timestamp: DateTime<Utc>,
}

/// Batch of request evaluation and completion events (sent every minute).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestHeartbeat {
    /// Ethereum address of the broker.
    pub broker_address: Address,
    /// Request evaluations performed since the last heartbeat.
    pub evaluated: Vec<RequestEvaluated>,
    /// Requests that reached a terminal state since the last heartbeat.
    pub completed: Vec<RequestCompleted>,
    /// Number of telemetry events dropped due to channel backpressure.
    pub events_dropped: u64,
    /// Timestamp when this heartbeat was generated.
    pub timestamp: DateTime<Utc>,
}

/// Outcome of order evaluation in the OrderPricer.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalOutcome {
    /// Order was locked for proving.
    Locked,
    /// Order will be fulfilled after another prover's lock expires.
    FulfillAfterLockExpire,
    /// Order was skipped during pricing.
    Skipped,
}

/// Outcome of the commitment decision after pricing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitmentOutcome {
    /// Order was committed to the proving pipeline.
    Committed,
    /// Order was dropped before entering the proving pipeline.
    Dropped,
}

/// Produced by the TelemetryService by merging an OrderPricing event (from the OrderPricer)
/// with an OrderCommitment event (from the OrderCommitter). For orders skipped during pricing,
/// only the pricing fields are populated; commitment fields remain None.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestEvaluated {
    /// Ethereum address of the broker that evaluated this request.
    pub broker_address: Address,
    /// Composite order ID: "0x{request_id}-{request_digest}-{fulfillment_type}".
    pub order_id: String,
    /// Hex-encoded on-chain request ID (e.g. "0x2a").
    pub request_id: String,
    /// Signing hash (digest) of the proof request, hex-encoded.
    pub request_digest: String,
    /// Address of the requestor who submitted the proof request.
    pub requestor: Address,
    /// Pricing outcome from the OrderPicker: Locked, FulfillAfterLockExpire, or Skipped.
    pub outcome: EvalOutcome,
    /// Structured skip code from pricing (e.g. "[B-OP-001]"). Set when outcome is Skipped.
    pub skip_code: Option<String>,
    /// Human-readable reason the order was skipped during pricing.
    pub skip_reason: Option<String>,
    /// Total execution cycles from preflight. None if preflight was skipped.
    pub total_cycles: Option<u64>,
    /// "LockAndFulfill", "FulfillAfterLockExpire", or "FulfillWithoutLocking".
    pub fulfillment_type: String,
    /// Time spent in the pending queue before a preflight slot was available (ms).
    /// Calculated in OrderPicker as: (now - received_at_timestamp - preflight_duration) * 1000.
    pub queue_duration_ms: Option<u64>,
    /// Time spent running preflight (upload + execution), in milliseconds.
    /// Calculated in OrderPicker as: (now - received_at_timestamp - queue_duration) * 1000.
    pub preflight_duration_ms: Option<u64>,
    /// Unix timestamp (seconds) when the broker first received this request from the network.
    pub received_at_timestamp: u64,
    /// When the TelemetryService produced this evaluation record.
    pub evaluated_at: DateTime<Utc>,
    /// Outcome of the commitment phase (Committed or Dropped). None for orders skipped
    /// during pricing (they never reach the OrderMonitor).
    pub commitment_outcome: Option<CommitmentOutcome>,
    /// Structured skip code from the commitment phase (e.g. "[B-OM-001]").
    /// Set when the order is dropped at commitment.
    pub commitment_skip_code: Option<String>,
    /// Human-readable reason the order was dropped at commitment.
    pub commitment_skip_reason: Option<String>,
    /// Estimated proving time in seconds with current load factored in. Accounts for all
    /// currently committed orders ahead in the queue. Recorded at the moment the OrderMonitor
    /// makes its final commit/drop decision. None when peak_prove_khz is not configured.
    pub estimated_proving_time_secs: Option<u64>,
    /// Estimated proving time in seconds ignoring current load (as if no other orders were
    /// queued). Calculated as:
    /// (total_cycles + additional_proof_cycles).div_ceil(1000).div_ceil(peak_prove_khz).
    /// Recorded at the moment the OrderMonitor makes its final commit/drop decision.
    /// None when peak_prove_khz is not configured or total_cycles is unavailable.
    pub estimated_proving_time_no_load_secs: Option<u64>,
    /// Time from when the order was priced (entered monitor cache) to when the OrderMonitor
    /// made its final commit/drop decision, in milliseconds. Calculated as:
    /// (now_timestamp() - order.priced_at_timestamp) * 1000.
    pub monitor_wait_duration_ms: Option<u64>,
    /// Peak proving speed from broker config (kHz). Passed through from
    /// config.market.peak_prove_khz at the time of the commitment decision.
    pub peak_prove_khz: Option<u64>,
    /// Max concurrent proofs from broker config. Passed through from
    /// config.market.max_concurrent_proofs at the time of the commitment decision.
    pub max_capacity: Option<u32>,
    /// Number of orders in the monitor caches waiting to be committed (excluding the
    /// current order). Recorded at the start of lock_and_prove_orders.
    pub pending_commitment_count: Option<u32>,
    /// Number of orders already committed in the DB at the moment the commit/drop decision
    /// is made. Queried via db.get_committed_orders().len().
    pub concurrent_proving_jobs: Option<u32>,
    /// Wall-clock seconds from when the lock transaction was submitted to when the lock was
    /// confirmed. None if no lock was needed (FulfillAfterLockExpire / FulfillWithoutLocking).
    pub lock_duration_secs: Option<u64>,
}

/// Terminal outcome of a request that has finished processing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// Successfully fulfilled on-chain.
    Fulfilled,
    /// Order expired while proofs were being generated.
    ExpiredWhileProving,
    /// Order expired before the fulfill transaction was submitted.
    ExpiredBeforeSubmission,
    /// Order was fulfilled before submission, e.g. another prover submitted secondary fulfillment first.
    FulfilledBeforeSubmission,
    /// The fulfill transaction failed on-chain.
    TxFailed,
    /// Proving failed.
    ProvingFailed,
    /// Aggregation (proof compression) failed.
    AggregationFailed,
    /// The lock transaction failed.
    LockFailed,
    /// Order was cancelled (e.g. by the reaper).
    Cancelled,
    /// Outcome does not fit other categories.
    Unknown,
}

/// Produced by the TelemetryService when an order reaches a terminal state (fulfilled,
/// failed, expired, etc.). Carries the full lifecycle timing breakdown.
///
/// Proving flow (two paths):
///
///   Path A (Groth16/Blake3Groth16 selector):
///     STARK proof -> Groth16 compression -> submit individually
///     Fields: stark_proving_secs, proof_compression_secs
///
///   Path B (inclusion / no selector):
///     STARK proof -> aggregation (set builder + assessor) -> Groth16 compression
///       of aggregation proof -> submit as batch
///     Fields: stark_proving_secs, set_builder_proving_secs, assessor_proving_secs,
///       assessor_compression_proof_secs
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestCompleted {
    /// Ethereum address of the broker that processed this request.
    pub broker_address: Address,
    /// Composite order ID: "0x{request_id}-{request_digest}-{fulfillment_type}".
    pub order_id: String,
    /// Hex-encoded on-chain request ID (e.g. "0x2a").
    pub request_id: String,
    /// Signing hash (digest) of the proof request, hex-encoded.
    pub request_digest: String,
    /// "Groth16", "Blake3Groth16", or "Merkle". Derived from the request's selector.
    pub proof_type: String,
    /// Terminal outcome (Fulfilled, ProvingFailed, TxFailed, etc.).
    pub outcome: CompletionOutcome,
    /// Structured error code (e.g. "[B-SUB-001]"). The prefix indicates the stage:
    /// B-REAP = expired while proving, B-OM = lock failed, B-SUB = tx failed,
    /// B-PRO = proving failed, B-AGG = aggregation failed.
    pub error_code: Option<String>,
    /// Human-readable error description.
    pub error_reason: Option<String>,

    /// Wall-clock seconds from when the lock transaction was submitted to when
    /// the order was committed to the DB as locked. None if no lock was needed.
    pub lock_duration_secs: Option<u64>,
    /// Wall-clock seconds from commitment to when the application proof completed
    /// (includes optional per-order Groth16 compression for Groth16/Blake3Groth16 proof types).
    /// Calculated as: proving_completed_at - committed_at.
    pub committed_to_application_proof_duration_secs: Option<u64>,
    /// Wall-clock seconds from ApplicationProvingCompleted to AggregationCompleted.
    /// None for orders on Path A (individually compressed, skip aggregation).
    pub aggregation_duration_secs: Option<u64>,
    /// Wall-clock seconds from AggregationCompleted (or ApplicationProvingCompleted for Path A)
    /// to when the Fulfilled/Failed event was received.
    pub submission_duration_secs: Option<u64>,
    /// Total wall-clock duration from when the order was first received to when it
    /// reached a terminal state. Calculated as: now_unix() - received_at_timestamp.
    pub total_duration_secs: u64,

    /// Estimated proving time (with load) recorded at commitment time. Copied from
    /// the InFlightRequest entry that was populated by the OrderCommitment event.
    pub estimated_proving_time_secs: Option<u64>,
    /// Wall-clock seconds from commitment to when all proofs completed.
    /// Path A (Groth16): committed_at to proving_completed_at (STARK + compression).
    /// Path B (Merkle/batch): committed_at to aggregation_completed_at (STARK + set builder + assessor + batch compression).
    pub actual_total_proving_time_secs: Option<u64>,
    /// Number of orders committed in the DB when this order entered the proving pipeline.
    /// Recorded by the OrderCommitment event at commit time.
    pub concurrent_proving_jobs_start: Option<u32>,
    /// Number of orders committed in the DB when this order completed.
    /// Queried via db.get_committed_orders().len() at finalization time.
    pub concurrent_proving_jobs_end: Option<u32>,
    /// Total execution cycles reported by the prover. May differ from the preflight
    /// estimate; updated by ApplicationProvingCompleted if available.
    pub total_cycles: Option<u64>,
    /// "LockAndFulfill", "FulfillAfterLockExpire", or "FulfillWithoutLocking".
    pub fulfillment_type: String,

    /// Wall-clock seconds for the customer STARK proof (session creation through
    /// proof monitoring to completion). Reported by ApplicationProvingCompleted.
    pub stark_proving_secs: Option<f64>,
    /// Wall-clock seconds for Groth16/Blake3Groth16 compression of the individual
    /// customer STARK proof. Only set on Path A; None for merkle inclusion orders.
    /// Reported by ApplicationProvingCompleted.
    pub proof_compression_secs: Option<f64>,
    /// Wall-clock seconds for the set-builder STARK proof that merges multiple
    /// order claim digests into an aggregation tree. Path B only.
    /// Reported by AggregationCompleted.
    pub set_builder_proving_secs: Option<f64>,
    /// Wall-clock seconds for the assessor STARK proof that validates the batch
    /// fulfillments. Path B only. Reported by AggregationCompleted.
    pub assessor_proving_secs: Option<f64>,
    /// Wall-clock seconds for compressing the aggregation STARK proof into Groth16
    /// for on-chain verification. Shared across all orders in the same batch.
    /// Path B only. Reported by AggregationCompleted.
    pub assessor_compression_proof_secs: Option<f64>,

    /// Unix timestamp (seconds) when the broker first received this request from the network.
    pub received_at_timestamp: u64,
    /// When the TelemetryService produced this completion record.
    pub completed_at: DateTime<Utc>,
}
