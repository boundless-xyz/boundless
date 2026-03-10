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

/// Outcome of order evaluation in the OrderPicker.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalOutcome {
    /// Order was locked for proving.
    Locked,
    /// Order will be fulfilled after another prover's lock expires.
    FulfillAfterLockExpire,
    /// Order was skipped.
    Skipped,
}

/// Emitted when the OrderPicker evaluates a request.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestEvaluated {
    /// Ethereum address of the broker that evaluated this request.
    pub broker_address: Address,
    /// Composite order ID: "0x{request_id}-{request_digest}-{fulfillment_type}".
    pub order_id: String,
    /// Hex-encoded on-chain request ID.
    pub request_id: String,
    /// Signing hash (digest) of the proof request, hex-encoded.
    pub request_digest: String,
    /// Address of the requestor who submitted the proof request.
    pub requestor: Address,
    /// Evaluation outcome.
    pub outcome: EvalOutcome,
    /// Structured skip code (e.g. "[B-OP-001]"), if applicable.
    pub skip_code: Option<String>,
    /// Human-readable reason the order was skipped, if applicable.
    pub skip_reason: Option<String>,
    /// Total execution cycles from preflight.
    pub total_cycles: Option<u64>,
    /// Estimated total proving time at evaluation, covering the full pipeline:
    /// STARK proving + compression/aggregation + submission.
    pub estimated_total_proving_time_secs: Option<u64>,
    /// Fulfillment type (e.g. "LockAndFulfill", "FulfillAfterLockExpire").
    pub fulfillment_type: String,
    /// Whether the preflight result was served from cache (no actual execution).
    pub preflight_cache_hit: bool,
    /// Time spent in the pending queue before a preflight slot was available (ms).
    pub queue_duration_ms: Option<u64>,
    /// Time spent running preflight (upload + execution), in milliseconds.
    pub preflight_duration_ms: Option<u64>,
    /// Unix timestamp (seconds since epoch) when the broker first received this
    /// request from the network, before any processing.
    pub received_at_timestamp: u64,
    /// When the evaluation occurred.
    pub evaluated_at: DateTime<Utc>,
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

/// Emitted when a request reaches a terminal state.
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
    /// Hex-encoded on-chain request ID.
    pub request_id: String,
    /// Signing hash (digest) of the proof request, hex-encoded.
    pub request_digest: String,
    /// Proof type: "Groth16", "Blake3Groth16", or "Merkle".
    pub proof_type: String,
    /// Terminal outcome.
    pub outcome: CompletionOutcome,
    /// Structured error code (e.g. "[B-SUB-001]"), if applicable.
    pub error_code: Option<String>,
    /// Human-readable error reason, if applicable.
    pub error_reason: Option<String>,

    /// Wall-clock seconds from when the lock transaction was submitted to when
    /// the order was committed to the DB as locked. None if no lock was needed.
    pub lock_duration_secs: Option<u64>,
    /// Wall-clock seconds from when the order was committed (entered proving
    /// pipeline) to when proving completed (STARK + optional compression).
    pub proving_duration_secs: Option<u64>,
    /// Wall-clock seconds from when proving completed to when aggregation
    /// completed (set builder + assessor + aggregation Groth16 compression).
    /// None for orders on Path A (individually compressed, skip aggregation).
    pub aggregation_duration_secs: Option<u64>,
    /// Wall-clock seconds from when aggregation (or per-order compression)
    /// completed to when the fulfill transaction was confirmed on-chain.
    pub submission_duration_secs: Option<u64>,
    /// Total wall-clock duration from when the order was first received (entered the
    /// pending queue) to when it reached a terminal state, in seconds.
    pub total_duration_secs: u64,

    /// Estimated total proving time at evaluation, covering the full pipeline:
    /// STARK proving + compression/aggregation + submission.
    pub estimated_total_proving_time_secs: Option<u64>,
    /// Actual total proving time observed, covering the full pipeline:
    /// STARK proving + compression/aggregation + submission.
    pub actual_total_proving_time_secs: Option<u64>,
    /// Number of orders being proved concurrently when this order entered the proving pipeline.
    pub concurrent_proving_jobs_start: Option<u32>,
    /// Number of orders being proved concurrently when this order completed.
    pub concurrent_proving_jobs_end: Option<u32>,
    /// Total execution cycles.
    pub total_cycles: Option<u64>,
    /// Fulfillment type (e.g. "LockAndFulfill", "FulfillAfterLockExpire").
    pub fulfillment_type: String,

    /// Wall-clock seconds for the customer STARK proof (from session creation
    /// through proof monitoring to completion).
    pub stark_proving_secs: Option<f64>,
    /// Wall-clock seconds for Groth16/Blake3Groth16 compression of the individual
    /// customer STARK proof. Only set when the request's selector requires Groth16
    /// or Blake3Groth16; None for merkle inclusion orders.
    pub proof_compression_secs: Option<f64>,
    /// Wall-clock seconds for the set-builder STARK proof that merges multiple
    /// order claim digests into an aggregation tree (Path B only).
    pub set_builder_proving_secs: Option<f64>,
    /// Wall-clock seconds for the assessor STARK proof that validates the batch
    /// fulfillments (Path B only).
    pub assessor_proving_secs: Option<f64>,
    /// Wall-clock seconds for compressing the aggregation STARK proof (produced
    /// by the set builder) into a Groth16 proof for on-chain verification.
    /// Shared across all orders in the same batch (Path B only).
    pub assessor_compression_proof_secs: Option<f64>,

    /// Unix timestamp (seconds since epoch) when the broker first received this
    /// request from the network, before any processing.
    pub received_at_timestamp: u64,
    /// When the request reached its terminal state.
    pub completed_at: DateTime<Utc>,
}
