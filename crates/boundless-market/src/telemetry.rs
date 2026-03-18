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
    /// Order was skipped.
    Skipped,
}

/// Emitted when the OrderPricer evaluates a request.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestEvaluated {
    /// Ethereum address of the broker that evaluated this request.
    pub broker_address: Address,
    /// Hex-encoded request ID (on-chain sequential ID).
    pub request_id: String,
    /// Hex-encoded request digest (content hash). The true unique identifier for orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    /// Address of the requestor who submitted the proof request.
    pub requestor: Address,
    /// Evaluation outcome.
    pub outcome: EvalOutcome,
    /// Reason the order was skipped, if applicable.
    pub skip_reason: Option<String>,
    /// Total execution cycles from preflight.
    pub total_cycles: Option<u64>,
    /// Estimated proving time in seconds.
    pub estimated_proving_time_secs: Option<u64>,
    /// Fulfillment type (e.g. "LockAndFulfill", "FulfillAfterLockExpire").
    pub fulfillment_type: Option<String>,
    /// Whether the preflight result was served from cache (no actual execution).
    pub preflight_cache_hit: bool,
    /// Time spent in the pending queue before a preflight slot was available (ms).
    pub queue_duration_ms: Option<u64>,
    /// Time spent running preflight (upload + execution), in milliseconds.
    pub preflight_duration_ms: Option<u64>,
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
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct RequestCompleted {
    /// Ethereum address of the broker that processed this request.
    pub broker_address: Address,
    /// Hex-encoded request ID (on-chain sequential ID).
    pub request_id: String,
    /// Hex-encoded request digest (content hash). The true unique identifier for orders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_digest: Option<String>,
    /// Terminal outcome.
    pub outcome: CompletionOutcome,
    /// Broker error code (e.g. "[B-SUB-001]"), if applicable.
    pub error_code: Option<String>,

    /// Duration of the lock phase in seconds.
    pub lock_duration_secs: Option<u64>,
    /// Duration of proving in seconds.
    pub proving_duration_secs: Option<u64>,
    /// Duration of aggregation in seconds.
    pub aggregation_duration_secs: Option<u64>,
    /// Duration of submission in seconds.
    pub submission_duration_secs: Option<u64>,
    /// Total wall-clock duration from when the order was first received (entered the
    /// pending queue) to when it reached a terminal state, in seconds.
    pub total_duration_secs: u64,

    /// Estimated proving time at evaluation.
    pub estimated_proving_time_secs: Option<u64>,
    /// Actual proving time observed.
    pub actual_proving_time_secs: Option<u64>,
    /// Number of other orders being proved concurrently.
    pub concurrent_proving_jobs: u32,
    /// Total execution cycles.
    pub total_cycles: Option<u64>,
    /// Fulfillment type (e.g. "LockAndFulfill", "FulfillAfterLockExpire").
    pub fulfillment_type: String,

    /// When the request reached its terminal state.
    pub completed_at: DateTime<Utc>,
}
