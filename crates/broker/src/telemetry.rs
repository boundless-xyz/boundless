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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Duration, Instant};

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::signers::local::PrivateKeySigner;
use anyhow::Result;
use boundless_market::order_stream_client::OrderStreamClient;
use boundless_market::selector::{is_blake3_groth16_selector, is_groth16_selector};
use boundless_market::telemetry::{
    BrokerHeartbeat, CommitmentOutcome, CompletionOutcome, EvalOutcome, RequestCompleted,
    RequestEvaluated, RequestHeartbeat,
};
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use boundless_market::prover_utils::OrderRequest;

use crate::config::ConfigLock;
use crate::db::DbObj;
use crate::errors::BrokerFailure;
use crate::futures_retry::retry;
use crate::order_monitor::{OrderCommitmentMeta, OrderMonitorConfig};

const HEARTBEAT_RETRY_COUNT: u64 = 2;
const HEARTBEAT_RETRY_SLEEP_MS: u64 = 1000;

static GLOBAL: Mutex<Option<TelemetryHandle>> = Mutex::new(None);
static NOOP_HANDLE: OnceLock<TelemetryHandle> = OnceLock::new();
static NOOP_HANDLE_WARNED: Once = Once::new();

/// Initializes the process-global telemetry singleton on first use and returns its handle.
///
/// The closure is only run for the first caller, so it is expected to create both the
/// `TelemetryHandle` and any underlying telemetry service/runtime that consumes it.
/// Later callers reuse the existing singleton handle without rebuilding the service.
pub(crate) fn init_with<F>(build_handle: F) -> TelemetryHandle
where
    F: FnOnce() -> TelemetryHandle,
{
    let mut global = GLOBAL.lock().unwrap();
    if let Some(handle) = global.clone() {
        tracing::warn!("Telemetry handle already initialized; refusing to replace existing handle");
        return handle;
    }

    let handle = build_handle();
    tracing::debug!("Telemetry handle initialized");
    *global = Some(handle.clone());
    handle
}

pub(crate) fn telemetry() -> TelemetryHandle {
    GLOBAL.lock().unwrap().clone().unwrap_or_else(noop_handle)
}

fn noop_handle() -> TelemetryHandle {
    NOOP_HANDLE_WARNED.call_once(|| {
        tracing::warn!(
            "Telemetry handle used before initialization; falling back to internal drop handle"
        );
    });

    NOOP_HANDLE
        .get_or_init(|| {
            let (tx, rx) = mpsc::channel(1);
            drop(rx);
            TelemetryHandle::new(tx)
        })
        .clone()
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const REQUEST_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const BROKER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const EVICTION_INTERVAL_SECS: u64 = 300;
const STALE_ENTRY_SECS: u64 = 7200;

// Internal telemetry events emitted by broker services to the TelemetryHandle.
// These events are then processed/merged by the TelemetryService into RequestEvaluated
// and RequestCompleted events. Those shared types live in the boundless-market crate.
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
    // Emitted by OrderMonitor when it makes its final commit/drop decision for an order.
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
        /// Structured skip code (e.g. "[B-OM-001]"), set when the order is dropped.
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
    ProvingCompleted {
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

// Lightweight, cheaply cloneable handle for recording telemetry events.
#[derive(Clone)]
pub(crate) struct TelemetryHandle {
    tx: mpsc::Sender<TelemetryEvent>,
    drop_count: Arc<AtomicU64>,
    pending_preflight_count: Arc<AtomicU32>,
}

impl TelemetryHandle {
    pub(crate) fn new(tx: mpsc::Sender<TelemetryEvent>) -> Self {
        Self {
            tx,
            drop_count: Arc::new(AtomicU64::new(0)),
            pending_preflight_count: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Send a telemetry event without blocking. Increments the drop counter on backpressure.
    pub(crate) fn record(&self, event: TelemetryEvent) {
        if self.tx.try_send(event).is_err() {
            self.drop_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn set_pending_preflight(&self, count: u32) {
        self.pending_preflight_count.store(count, Ordering::Relaxed);
    }

    pub(crate) fn drop_count(&self) -> u64 {
        self.drop_count.load(Ordering::Relaxed)
    }

    pub(crate) fn pending_preflight(&self) -> u32 {
        self.pending_preflight_count.load(Ordering::Relaxed)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_order_pricing(
        &self,
        order: &OrderRequest,
        outcome: EvalOutcome,
        skip_code: Option<&str>,
        skip_reason: Option<String>,
        total_cycles: Option<u64>,
        queue_start_secs: u64,
        preflight_duration_secs: u64,
    ) {
        self.record(TelemetryEvent::OrderPricing {
            order_id: order.id(),
            request_id: order.request.id,
            request_digest: order.request_digest(),
            requestor: order.request.client_address(),
            outcome,
            skip_code: skip_code.map(|s| s.to_string()),
            skip_reason,
            total_cycles,
            fulfillment_type: order.fulfillment_type.to_string(),
            proof_type: proof_type_label(order.request.requirements.selector).to_string(),
            queue_duration_ms: Some(queue_start_secs * 1000),
            preflight_duration_ms: Some(preflight_duration_secs * 1000),
            received_at_timestamp: order.received_at_timestamp,
        });
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn record_order_commitment(
        &self,
        order_id: &str,
        committed: bool,
        meta: Option<&OrderCommitmentMeta>,
        monitor_wait_duration_ms: Option<u64>,
        config: &OrderMonitorConfig,
        pending_commitment_count: u32,
        skip_code: Option<&str>,
        skip_reason: Option<&str>,
        lock_submitted_at: Option<Instant>,
    ) {
        self.record(TelemetryEvent::OrderCommitment {
            order_id: order_id.to_string(),
            committed,
            committed_at: if committed { Some(Instant::now()) } else { None },
            concurrent_proving_jobs: meta.map(|m| m.concurrent_proving_jobs).unwrap_or(0),
            estimated_proving_time_secs: meta.and_then(|m| m.estimated_proving_time_secs),
            estimated_proving_time_no_load_secs: meta
                .and_then(|m| m.estimated_proving_time_no_load_secs),
            monitor_wait_duration_ms,
            peak_prove_khz: config.peak_prove_khz,
            max_capacity: config.max_concurrent_proofs,
            pending_commitment_count,
            skip_commit_code: skip_code.map(|s| s.to_string()),
            skip_commit_reason: skip_reason.map(|s| s.to_string()),
            lock_submitted_at,
        });
    }

    pub(crate) fn record_proving_completed(
        &self,
        order_id: &str,
        total_cycles: Option<u64>,
        stark_proving_secs: Option<f64>,
        proof_compression_secs: Option<f64>,
    ) {
        self.record(TelemetryEvent::ProvingCompleted {
            order_id: order_id.to_string(),
            total_cycles,
            stark_proving_secs,
            proof_compression_secs,
        });
    }

    pub(crate) fn record_aggregation_completed(
        &self,
        order_id: &str,
        set_builder_proving_secs: Option<f64>,
        assessor_proving_secs: Option<f64>,
        assessor_compression_proof_secs: Option<f64>,
    ) {
        self.record(TelemetryEvent::AggregationCompleted {
            order_id: order_id.to_string(),
            set_builder_proving_secs,
            assessor_proving_secs,
            assessor_compression_proof_secs,
        });
    }

    pub(crate) fn record_fulfilled(&self, order_id: &str) {
        self.record(TelemetryEvent::Fulfilled { order_id: order_id.to_string() });
    }

    pub(crate) fn record_failed(&self, order_id: &str, failure: &BrokerFailure) {
        self.record(TelemetryEvent::Failed {
            order_id: order_id.to_string(),
            error_code: failure.code.clone(),
            error_reason: failure.reason.clone(),
            outcome: failure.outcome,
        });
    }
}

// In-memory tracking for a request as it moves through the pipeline.
struct InFlightRequest {
    received_at_timestamp: u64,
    request_id: Option<U256>,
    request_digest: String,
    fulfillment_type: String,
    proof_type: String,
    total_cycles: Option<u64>,
    // Pricing fields stored for deferred RequestEvaluated construction.
    requestor: Option<Address>,
    pricing_outcome: Option<EvalOutcome>,
    pricing_skip_code: Option<String>,
    pricing_skip_reason: Option<String>,
    queue_duration_ms: Option<u64>,
    preflight_duration_ms: Option<u64>,
    // Commitment + completion tracking fields.
    estimated_proving_time_secs: Option<u64>,
    lock_duration_secs: Option<u64>,
    committed_at: Option<Instant>,
    concurrent_proving_jobs_start: Option<u32>,
    stark_proving_secs: Option<f64>,
    proof_compression_secs: Option<f64>,
    proving_completed_at: Option<Instant>,
    set_builder_proving_secs: Option<f64>,
    assessor_proving_secs: Option<f64>,
    assessor_compression_proof_secs: Option<f64>,
    aggregation_completed_at: Option<Instant>,
}

impl InFlightRequest {
    fn new() -> Self {
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

struct CommitmentInfo {
    outcome: Option<CommitmentOutcome>,
    skip_commit_code: Option<String>,
    skip_commit_reason: Option<String>,
    estimated_proving_time_secs: Option<u64>,
    estimated_proving_time_no_load_secs: Option<u64>,
    monitor_wait_duration_ms: Option<u64>,
    peak_prove_khz: Option<u64>,
    max_capacity: Option<u32>,
    pending_commitment_count: Option<u32>,
    concurrent_proving_jobs: Option<u32>,
    lock_duration_secs: Option<u64>,
}

fn build_request_evaluated(
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

pub(crate) struct TelemetryService {
    rx: mpsc::Receiver<TelemetryEvent>,
    handle: TelemetryHandle,
    client: Option<OrderStreamClient>,
    signer: PrivateKeySigner,
    broker_address: Address,
    config: ConfigLock,
    db: DbObj,
    in_flight: HashMap<String, InFlightRequest>,
    eval_buffer: Vec<RequestEvaluated>,
    comp_buffer: Vec<RequestCompleted>,
    uptime_start: Instant,
    request_heartbeat_interval_secs: u64,
    broker_heartbeat_interval_secs: u64,
}

impl TelemetryService {
    pub(crate) fn new(
        rx: mpsc::Receiver<TelemetryEvent>,
        handle: TelemetryHandle,
        client: OrderStreamClient,
        signer: PrivateKeySigner,
        config: ConfigLock,
        db: DbObj,
    ) -> Self {
        let broker_address = signer.address();
        let (req_interval, broker_interval) = read_heartbeat_intervals(&config);
        Self {
            rx,
            handle,
            client: Some(client),
            signer,
            broker_address,
            config,
            db,
            in_flight: HashMap::new(),
            eval_buffer: Vec::new(),
            comp_buffer: Vec::new(),
            uptime_start: Instant::now(),
            request_heartbeat_interval_secs: req_interval,
            broker_heartbeat_interval_secs: broker_interval,
        }
    }

    pub(crate) fn new_debug(
        rx: mpsc::Receiver<TelemetryEvent>,
        handle: TelemetryHandle,
        signer: PrivateKeySigner,
        config: ConfigLock,
        db: DbObj,
    ) -> Self {
        let broker_address = signer.address();
        let (req_interval, broker_interval) = read_heartbeat_intervals(&config);
        Self {
            rx,
            handle,
            client: None,
            signer,
            broker_address,
            config,
            db,
            in_flight: HashMap::new(),
            eval_buffer: Vec::new(),
            comp_buffer: Vec::new(),
            uptime_start: Instant::now(),
            request_heartbeat_interval_secs: req_interval,
            broker_heartbeat_interval_secs: broker_interval,
        }
    }

    async fn process_event(&mut self, event: TelemetryEvent) {
        match event {
            TelemetryEvent::OrderPricing {
                order_id,
                request_id,
                request_digest,
                requestor,
                outcome,
                skip_code,
                skip_reason,
                total_cycles,
                fulfillment_type,
                proof_type,
                queue_duration_ms,
                preflight_duration_ms,
                received_at_timestamp,
            } => {
                if outcome == EvalOutcome::Skipped {
                    // Skipped at pricing: build RequestEvaluated immediately.
                    let mut entry = InFlightRequest::new();
                    entry.request_id = Some(request_id);
                    entry.request_digest = request_digest;
                    entry.requestor = Some(requestor);
                    entry.pricing_outcome = Some(outcome);
                    entry.pricing_skip_code = skip_code;
                    entry.pricing_skip_reason = skip_reason;
                    entry.total_cycles = total_cycles;
                    entry.fulfillment_type = fulfillment_type;
                    entry.proof_type = proof_type;
                    entry.queue_duration_ms = queue_duration_ms;
                    entry.preflight_duration_ms = preflight_duration_ms;
                    entry.received_at_timestamp = received_at_timestamp;

                    let evaluated = build_request_evaluated(
                        self.broker_address,
                        order_id.clone(),
                        &entry,
                        CommitmentInfo {
                            outcome: None,
                            skip_commit_code: None,
                            skip_commit_reason: None,
                            estimated_proving_time_secs: None,
                            estimated_proving_time_no_load_secs: None,
                            monitor_wait_duration_ms: None,
                            peak_prove_khz: None,
                            max_capacity: None,
                            pending_commitment_count: None,
                            concurrent_proving_jobs: None,
                            lock_duration_secs: None,
                        },
                    );

                    tracing::debug!(
                        payload = %serde_json::to_string(&evaluated).unwrap_or_default(),
                        "(Telemetry) Request Evaluated: {}",
                        order_id,
                    );

                    self.eval_buffer.push(evaluated);
                } else {
                    // Not skipped: store in in_flight, wait for OrderCommitment.
                    let entry = self.in_flight.entry(order_id).or_insert_with(InFlightRequest::new);
                    entry.request_id = Some(request_id);
                    entry.request_digest = request_digest;
                    entry.requestor = Some(requestor);
                    entry.pricing_outcome = Some(outcome);
                    entry.pricing_skip_code = skip_code;
                    entry.pricing_skip_reason = skip_reason;
                    entry.total_cycles = total_cycles;
                    entry.fulfillment_type = fulfillment_type;
                    entry.proof_type = proof_type;
                    entry.queue_duration_ms = queue_duration_ms;
                    entry.preflight_duration_ms = preflight_duration_ms;
                    entry.received_at_timestamp = received_at_timestamp;
                }
            }
            TelemetryEvent::OrderCommitment {
                order_id,
                committed,
                committed_at,
                concurrent_proving_jobs,
                estimated_proving_time_secs,
                estimated_proving_time_no_load_secs,
                monitor_wait_duration_ms,
                peak_prove_khz,
                max_capacity,
                pending_commitment_count,
                skip_commit_code,
                skip_commit_reason,
                lock_submitted_at,
            } => {
                let lock_duration_secs = match (lock_submitted_at, committed_at) {
                    (Some(l), Some(c)) => Some(c.duration_since(l).as_secs()),
                    _ => None,
                };

                if committed {
                    let entry =
                        self.in_flight.entry(order_id.clone()).or_insert_with(InFlightRequest::new);
                    entry.committed_at = committed_at;
                    entry.concurrent_proving_jobs_start = Some(concurrent_proving_jobs);
                    entry.estimated_proving_time_secs = estimated_proving_time_secs;
                    entry.lock_duration_secs = lock_duration_secs;

                    let evaluated = build_request_evaluated(
                        self.broker_address,
                        order_id.clone(),
                        entry,
                        CommitmentInfo {
                            outcome: Some(CommitmentOutcome::Committed),
                            skip_commit_code: None,
                            skip_commit_reason: None,
                            estimated_proving_time_secs,
                            estimated_proving_time_no_load_secs,
                            monitor_wait_duration_ms,
                            peak_prove_khz,
                            max_capacity,
                            pending_commitment_count: Some(pending_commitment_count),
                            concurrent_proving_jobs: Some(concurrent_proving_jobs),
                            lock_duration_secs,
                        },
                    );

                    tracing::debug!(
                        payload = %serde_json::to_string(&evaluated).unwrap_or_default(),
                        "(Telemetry) Request Evaluated: {}",
                        order_id,
                    );

                    self.eval_buffer.push(evaluated);
                } else {
                    let entry =
                        self.in_flight.remove(&order_id).unwrap_or_else(InFlightRequest::new);

                    let evaluated = build_request_evaluated(
                        self.broker_address,
                        order_id.clone(),
                        &entry,
                        CommitmentInfo {
                            outcome: Some(CommitmentOutcome::Dropped),
                            skip_commit_code,
                            skip_commit_reason,
                            estimated_proving_time_secs,
                            estimated_proving_time_no_load_secs,
                            monitor_wait_duration_ms,
                            peak_prove_khz,
                            max_capacity,
                            pending_commitment_count: Some(pending_commitment_count),
                            concurrent_proving_jobs: Some(concurrent_proving_jobs),
                            lock_duration_secs: None,
                        },
                    );

                    tracing::debug!(
                        payload = %serde_json::to_string(&evaluated).unwrap_or_default(),
                        "(Telemetry) Request Evaluated: {}",
                        order_id,
                    );

                    self.eval_buffer.push(evaluated);
                }
            }
            TelemetryEvent::ProvingCompleted {
                order_id,
                total_cycles,
                stark_proving_secs,
                proof_compression_secs,
            } => {
                if let Some(entry) = self.in_flight.get_mut(&order_id) {
                    entry.total_cycles = total_cycles.or(entry.total_cycles);
                    entry.stark_proving_secs = stark_proving_secs;
                    entry.proof_compression_secs = proof_compression_secs;
                    entry.proving_completed_at = Some(Instant::now());
                }
            }
            TelemetryEvent::AggregationCompleted {
                order_id,
                set_builder_proving_secs,
                assessor_proving_secs,
                assessor_compression_proof_secs,
            } => {
                if let Some(entry) = self.in_flight.get_mut(&order_id) {
                    entry.set_builder_proving_secs = set_builder_proving_secs;
                    entry.assessor_proving_secs = assessor_proving_secs;
                    entry.assessor_compression_proof_secs = assessor_compression_proof_secs;
                    entry.aggregation_completed_at = Some(Instant::now());
                }
            }
            TelemetryEvent::Fulfilled { order_id } => {
                self.finalize_request(order_id, CompletionOutcome::Fulfilled, None, None).await;
            }
            TelemetryEvent::Failed { order_id, error_code, error_reason, outcome } => {
                self.finalize_request(order_id, outcome, Some(error_code), Some(error_reason))
                    .await;
            }
        }
    }

    async fn finalize_request(
        &mut self,
        order_id: String,
        outcome: CompletionOutcome,
        error_code: Option<String>,
        error_reason: Option<String>,
    ) {
        let now = Instant::now();
        let entry = self.in_flight.remove(&order_id).unwrap_or_else(InFlightRequest::new);

        let total_duration_secs = now_unix().saturating_sub(entry.received_at_timestamp);

        let proving_duration_secs = match (entry.committed_at, entry.proving_completed_at) {
            (Some(c), Some(p)) => Some(p.duration_since(c).as_secs()),
            _ => None,
        };

        let aggregation_duration_secs =
            match (entry.proving_completed_at, entry.aggregation_completed_at) {
                (Some(p), Some(a)) => Some(a.duration_since(p).as_secs()),
                _ => None,
            };

        // Path B (Merkle/batch): submission starts after aggregation completes.
        // Path A (Groth16/non-batch): no aggregation step, so submission starts after proving completes.
        let submission_duration_secs = entry
            .aggregation_completed_at
            .or(entry.proving_completed_at)
            .map(|t| now.duration_since(t).as_secs());

        let actual_total_proving_time_secs = entry.stark_proving_secs.map(|s| s as u64);

        let concurrent_proving_jobs_end = match self.db.get_committed_orders().await {
            Ok(orders) => Some(orders.len() as u32),
            Err(e) => {
                tracing::warn!("Failed to query committed orders for completion: {e}");
                None
            }
        };

        let request_id_str =
            entry.request_id.map(|id| format!("0x{id:x}")).unwrap_or_else(|| "unknown".to_string());

        let completed = RequestCompleted {
            broker_address: self.broker_address,
            order_id: order_id.clone(),
            request_id: request_id_str,
            request_digest: entry.request_digest,
            proof_type: entry.proof_type,
            outcome,
            error_code,
            error_reason,
            lock_duration_secs: entry.lock_duration_secs,
            proving_duration_secs,
            aggregation_duration_secs,
            submission_duration_secs,
            total_duration_secs,
            estimated_proving_time_secs: entry.estimated_proving_time_secs,
            actual_total_proving_time_secs,
            concurrent_proving_jobs_start: entry.concurrent_proving_jobs_start,
            concurrent_proving_jobs_end,
            total_cycles: entry.total_cycles,
            fulfillment_type: entry.fulfillment_type,
            stark_proving_secs: entry.stark_proving_secs,
            proof_compression_secs: entry.proof_compression_secs,
            set_builder_proving_secs: entry.set_builder_proving_secs,
            assessor_proving_secs: entry.assessor_proving_secs,
            assessor_compression_proof_secs: entry.assessor_compression_proof_secs,
            received_at_timestamp: entry.received_at_timestamp,
            completed_at: Utc::now(),
        };

        let payload = serde_json::to_string(&completed).unwrap_or_default();
        tracing::debug!(payload = %payload, "(Telemetry) Request Completed: {}", order_id);

        self.comp_buffer.push(completed);
    }

    async fn send_broker_heartbeat(&self) {
        let committed_orders_count = match self.db.get_committed_orders().await {
            Ok(orders) => orders.len() as u32,
            Err(e) => {
                tracing::warn!("Failed to query committed orders for heartbeat: {e}");
                0
            }
        };

        let config_value = match self.config.lock_all() {
            Ok(c) => serde_json::to_value(&*c).unwrap_or(serde_json::Value::Null),
            Err(_) => serde_json::Value::Null,
        };

        let heartbeat = BrokerHeartbeat {
            broker_address: self.broker_address,
            config: config_value,
            committed_orders_count,
            pending_preflight_count: self.handle.pending_preflight(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_secs: self.uptime_start.elapsed().as_secs(),
            events_dropped: self.handle.drop_count(),
            timestamp: Utc::now(),
        };

        let submitted = if let Some(ref client) = self.client {
            retry(
                HEARTBEAT_RETRY_COUNT,
                HEARTBEAT_RETRY_SLEEP_MS,
                || client.submit_heartbeat(&heartbeat, &self.signer),
                "send_broker_heartbeat",
            )
            .await
            .is_ok()
        } else {
            false
        };

        let payload = serde_json::to_string(&heartbeat).unwrap_or_default();
        tracing::debug!(submitted, payload = %payload, "(Telemetry) Broker Heartbeat");
    }

    async fn send_request_heartbeat(&mut self) {
        if self.eval_buffer.is_empty() && self.comp_buffer.is_empty() {
            return;
        }

        let heartbeat = RequestHeartbeat {
            broker_address: self.broker_address,
            evaluated: std::mem::take(&mut self.eval_buffer),
            completed: std::mem::take(&mut self.comp_buffer),
            events_dropped: self.handle.drop_count(),
            timestamp: Utc::now(),
        };

        let submitted = if let Some(ref client) = self.client {
            retry(
                HEARTBEAT_RETRY_COUNT,
                HEARTBEAT_RETRY_SLEEP_MS,
                || client.submit_request_heartbeat(&heartbeat, &self.signer),
                "send_request_heartbeat",
            )
            .await
            .is_ok()
        } else {
            false
        };

        let payload = serde_json::to_string(&heartbeat).unwrap_or_default();
        tracing::debug!(submitted, payload = %payload, "(Telemetry) Request Heartbeat");
    }

    fn evict_stale_entries(&mut self) {
        let cutoff = now_unix().saturating_sub(STALE_ENTRY_SECS);
        self.in_flight.retain(|_, entry| entry.received_at_timestamp > cutoff);
    }
}

fn read_heartbeat_intervals(config: &ConfigLock) -> (u64, u64) {
    match config.lock_all() {
        Ok(c) => {
            (c.market.request_heartbeat_interval_secs, c.market.broker_heartbeat_interval_secs)
        }
        Err(_) => (REQUEST_HEARTBEAT_INTERVAL_SECS, BROKER_HEARTBEAT_INTERVAL_SECS),
    }
}

pub(crate) async fn run_telemetry_service(
    mut service: TelemetryService,
    cancel_token: CancellationToken,
) -> Result<()> {
    let mut request_interval =
        tokio::time::interval(Duration::from_secs(service.request_heartbeat_interval_secs));
    let mut broker_interval =
        tokio::time::interval(Duration::from_secs(service.broker_heartbeat_interval_secs));
    let mut eviction_interval = tokio::time::interval(Duration::from_secs(EVICTION_INTERVAL_SECS));

    // Don't fire immediately on startup
    request_interval.tick().await;
    broker_interval.tick().await;
    eviction_interval.tick().await;

    loop {
        tokio::select! {
            event = service.rx.recv() => {
                match event {
                    Some(e) => service.process_event(e).await,
                    None => break,
                }
            }
            _ = request_interval.tick() => {
                service.send_request_heartbeat().await;
            }
            _ = broker_interval.tick() => {
                service.send_broker_heartbeat().await;
            }
            _ = eviction_interval.tick() => {
                service.evict_stale_entries();
            }
            _ = cancel_token.cancelled() => {
                // Flush remaining buffers before exit
                service.send_request_heartbeat().await;
                service.send_broker_heartbeat().await;
                break;
            }
        }
    }

    Ok(())
}

pub(crate) fn proof_type_label(selector: FixedBytes<4>) -> &'static str {
    if is_groth16_selector(selector) {
        "Groth16"
    } else if is_blake3_groth16_selector(selector) {
        "Blake3Groth16"
    } else {
        "Merkle"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigLock;
    use crate::db::{DbObj, SqliteDb};

    async fn test_service() -> TelemetryService {
        let (tx, rx) = mpsc::channel(16);
        let handle = TelemetryHandle::new(tx);
        let client =
            OrderStreamClient::new("http://localhost:1234".parse().unwrap(), Address::ZERO, 1);
        let signer = PrivateKeySigner::random();
        let config = ConfigLock::default();
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        TelemetryService::new(rx, handle, client, signer, config, db)
    }

    #[test]
    fn test_handle_records_drop_when_receiver_closed() {
        let (tx, rx) = mpsc::channel(16);
        drop(rx);
        let handle = TelemetryHandle::new(tx);
        handle.record(TelemetryEvent::Fulfilled { order_id: "test-order-1".to_string() });
        assert_eq!(handle.drop_count(), 1);
    }

    #[test]
    fn test_handle_atomic_ops() {
        let (tx, _rx) = mpsc::channel(16);
        let handle = TelemetryHandle::new(tx);

        handle.set_pending_preflight(42);
        assert_eq!(handle.pending_preflight(), 42);
    }

    #[tokio::test]
    async fn test_handle_record_succeeds() {
        let (tx, mut rx) = mpsc::channel(16);
        let handle = TelemetryHandle::new(tx);

        handle.record(TelemetryEvent::Fulfilled { order_id: "test-order-99".to_string() });
        assert_eq!(handle.drop_count(), 0);

        let event = rx.recv().await.unwrap();
        assert!(
            matches!(event, TelemetryEvent::Fulfilled { order_id } if order_id == "test-order-99")
        );
    }

    fn test_order_id(n: u64) -> String {
        format!("0x{n:x}-0xdeadbeef-LockAndFulfill")
    }

    fn make_order_pricing(order_id: &str, outcome: EvalOutcome) -> TelemetryEvent {
        TelemetryEvent::OrderPricing {
            order_id: order_id.to_string(),
            request_id: U256::from(42),
            request_digest: "0xdeadbeef".to_string(),
            requestor: Address::ZERO,
            outcome,
            skip_code: None,
            skip_reason: None,
            total_cycles: Some(1_000_000),
            fulfillment_type: "LockAndFulfill".to_string(),
            proof_type: "Merkle".to_string(),
            queue_duration_ms: Some(500),
            preflight_duration_ms: Some(1200),
            received_at_timestamp: 1700000000,
        }
    }

    #[tokio::test]
    async fn test_order_pricing_skipped_creates_eval_immediately() {
        let mut service = test_service().await;
        let oid = test_order_id(42);

        service
            .process_event(TelemetryEvent::OrderPricing {
                order_id: oid.clone(),
                request_id: U256::from(42),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Skipped,
                skip_code: Some("[S-001]".to_string()),
                skip_reason: Some("Expired".to_string()),
                total_cycles: None,
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Merkle".to_string(),
                queue_duration_ms: Some(500),
                preflight_duration_ms: Some(1200),
                received_at_timestamp: 1700000000,
            })
            .await;

        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].outcome, EvalOutcome::Skipped);
        assert!(service.eval_buffer[0].commitment_outcome.is_none());
        assert!(!service.in_flight.contains_key(&oid));
    }

    #[tokio::test]
    async fn test_order_pricing_locked_defers_eval() {
        let mut service = test_service().await;
        let oid = test_order_id(42);

        service.process_event(make_order_pricing(&oid, EvalOutcome::Locked)).await;

        // Eval is deferred until OrderCommitment.
        assert_eq!(service.eval_buffer.len(), 0);
        assert!(service.in_flight.contains_key(&oid));
    }

    #[tokio::test]
    async fn test_commitment_committed_creates_eval() {
        let mut service = test_service().await;
        let oid = test_order_id(42);

        service.process_event(make_order_pricing(&oid, EvalOutcome::Locked)).await;
        let lock_start = Instant::now();
        service
            .process_event(TelemetryEvent::OrderCommitment {
                order_id: oid.clone(),
                committed: true,
                committed_at: Some(Instant::now()),
                concurrent_proving_jobs: 3,
                estimated_proving_time_secs: Some(30),
                estimated_proving_time_no_load_secs: Some(10),
                monitor_wait_duration_ms: Some(500),
                peak_prove_khz: Some(100),
                max_capacity: Some(4),
                pending_commitment_count: 2,
                skip_commit_code: None,
                skip_commit_reason: None,
                lock_submitted_at: Some(lock_start),
            })
            .await;

        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].outcome, EvalOutcome::Locked);
        assert_eq!(service.eval_buffer[0].commitment_outcome, Some(CommitmentOutcome::Committed));
        assert_eq!(service.eval_buffer[0].estimated_proving_time_secs, Some(30));
        assert_eq!(service.eval_buffer[0].concurrent_proving_jobs, Some(3));
        assert_eq!(service.eval_buffer[0].lock_duration_secs, Some(0));
        // Still in_flight for completion tracking.
        assert!(service.in_flight.contains_key(&oid));
    }

    #[tokio::test]
    async fn test_commitment_dropped_creates_eval_without_completion() {
        let mut service = test_service().await;
        let oid = test_order_id(42);

        service.process_event(make_order_pricing(&oid, EvalOutcome::Locked)).await;
        service
            .process_event(TelemetryEvent::OrderCommitment {
                order_id: oid.clone(),
                committed: false,
                committed_at: None,
                concurrent_proving_jobs: 0,
                estimated_proving_time_secs: None,
                estimated_proving_time_no_load_secs: None,
                monitor_wait_duration_ms: None,
                peak_prove_khz: None,
                max_capacity: None,
                pending_commitment_count: 0,
                skip_commit_code: Some("[B-OM-001]".to_string()),
                skip_commit_reason: Some("Lock failed".to_string()),
                lock_submitted_at: Some(Instant::now()),
            })
            .await;

        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].commitment_outcome, Some(CommitmentOutcome::Dropped));
        assert_eq!(service.eval_buffer[0].commitment_skip_code, Some("[B-OM-001]".to_string()));
        assert_eq!(service.eval_buffer[0].commitment_skip_reason, Some("Lock failed".to_string()));
        // Removed from in_flight, no completion expected.
        assert!(!service.in_flight.contains_key(&oid));
        assert!(service.comp_buffer.is_empty());
    }

    #[tokio::test]
    async fn test_full_lifecycle_produces_completed() {
        let mut service = test_service().await;
        let oid = test_order_id(100);

        service
            .process_event(TelemetryEvent::OrderPricing {
                order_id: oid.clone(),
                request_id: U256::from(100),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Locked,
                skip_code: None,
                skip_reason: None,
                total_cycles: Some(5_000_000),
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Merkle".to_string(),
                queue_duration_ms: Some(100),
                preflight_duration_ms: Some(0),
                received_at_timestamp: 1700000000,
            })
            .await;

        // No eval yet.
        assert_eq!(service.eval_buffer.len(), 0);

        let lock_start = Instant::now();
        service
            .process_event(TelemetryEvent::OrderCommitment {
                order_id: oid.clone(),
                committed: true,
                committed_at: Some(Instant::now()),
                concurrent_proving_jobs: 3,
                estimated_proving_time_secs: Some(60),
                estimated_proving_time_no_load_secs: Some(20),
                monitor_wait_duration_ms: Some(300),
                peak_prove_khz: Some(100),
                max_capacity: Some(4),
                pending_commitment_count: 1,
                skip_commit_code: None,
                skip_commit_reason: None,
                lock_submitted_at: Some(lock_start),
            })
            .await;

        // Eval created after commitment.
        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].commitment_outcome, Some(CommitmentOutcome::Committed));

        service
            .process_event(TelemetryEvent::ProvingCompleted {
                order_id: oid.clone(),
                total_cycles: Some(5_000_000),
                stark_proving_secs: Some(45.2),
                proof_compression_secs: None,
            })
            .await;

        service
            .process_event(TelemetryEvent::AggregationCompleted {
                order_id: oid.clone(),
                set_builder_proving_secs: Some(10.5),
                assessor_proving_secs: Some(3.2),
                assessor_compression_proof_secs: Some(8.1),
            })
            .await;

        service.process_event(TelemetryEvent::Fulfilled { order_id: oid.clone() }).await;

        assert_eq!(service.comp_buffer.len(), 1);
        let completed = &service.comp_buffer[0];
        assert_eq!(completed.outcome, CompletionOutcome::Fulfilled);
        assert_eq!(completed.total_cycles, Some(5_000_000));
        assert_eq!(completed.stark_proving_secs, Some(45.2));
        assert_eq!(completed.set_builder_proving_secs, Some(10.5));
        assert_eq!(completed.assessor_proving_secs, Some(3.2));
        assert_eq!(completed.assessor_compression_proof_secs, Some(8.1));
        assert_eq!(completed.fulfillment_type, "LockAndFulfill");
        assert_eq!(completed.proof_type, "Merkle");
        assert!(completed.error_code.is_none());
        assert!(!service.in_flight.contains_key(&oid));
    }

    #[tokio::test]
    async fn test_failed_after_commitment_produces_completed() {
        let mut service = test_service().await;
        let oid = test_order_id(200);

        service
            .process_event(TelemetryEvent::OrderPricing {
                order_id: oid.clone(),
                request_id: U256::from(200),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Locked,
                skip_code: None,
                skip_reason: None,
                total_cycles: Some(1_000_000),
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Groth16".to_string(),
                queue_duration_ms: None,
                preflight_duration_ms: None,
                received_at_timestamp: 1700000000,
            })
            .await;

        service
            .process_event(TelemetryEvent::OrderCommitment {
                order_id: oid.clone(),
                committed: true,
                committed_at: Some(Instant::now()),
                concurrent_proving_jobs: 1,
                estimated_proving_time_secs: None,
                estimated_proving_time_no_load_secs: None,
                monitor_wait_duration_ms: None,
                peak_prove_khz: None,
                max_capacity: None,
                pending_commitment_count: 0,
                skip_commit_code: None,
                skip_commit_reason: None,
                lock_submitted_at: None,
            })
            .await;

        service
            .process_event(TelemetryEvent::Failed {
                order_id: oid,
                error_code: "[B-PRO-501]".to_string(),
                error_reason: "Proving failed".to_string(),
                outcome: CompletionOutcome::ProvingFailed,
            })
            .await;

        assert_eq!(service.comp_buffer.len(), 1);
        let completed = &service.comp_buffer[0];
        assert_eq!(completed.outcome, CompletionOutcome::ProvingFailed);
        assert_eq!(completed.error_code, Some("[B-PRO-501]".to_string()));
        assert_eq!(completed.error_reason, Some("Proving failed".to_string()));
    }

    #[tokio::test]
    async fn test_evict_stale_entries() {
        let mut service = test_service().await;

        let stale_id = "stale-order".to_string();
        let mut stale_entry = InFlightRequest::new();
        stale_entry.received_at_timestamp = now_unix().saturating_sub(STALE_ENTRY_SECS + 100);
        service.in_flight.insert(stale_id.clone(), stale_entry);

        let fresh_id = "fresh-order".to_string();
        service.in_flight.insert(fresh_id.clone(), InFlightRequest::new());

        service.evict_stale_entries();

        assert!(!service.in_flight.contains_key(&stale_id));
        assert!(service.in_flight.contains_key(&fresh_id));
    }

    #[tokio::test]
    async fn test_fulfilled_without_prior_events() {
        let mut service = test_service().await;
        let oid = test_order_id(300);

        service.process_event(TelemetryEvent::Fulfilled { order_id: oid }).await;

        assert_eq!(service.comp_buffer.len(), 1);
        let completed = &service.comp_buffer[0];
        assert_eq!(completed.outcome, CompletionOutcome::Fulfilled);
        assert_eq!(completed.fulfillment_type, "");
    }

    #[tokio::test]
    async fn test_send_request_heartbeat_skips_when_empty() {
        let mut service = test_service().await;
        // No events - should not attempt to send
        service.send_request_heartbeat().await;
        // No panic = success (the client URL is fake, so sending would fail)
    }
}
