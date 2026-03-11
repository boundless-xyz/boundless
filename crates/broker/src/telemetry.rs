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
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, FixedBytes, U256};
use alloy::signers::local::PrivateKeySigner;
use anyhow::Result;
use boundless_market::order_stream_client::OrderStreamClient;
use boundless_market::selector::{is_blake3_groth16_selector, is_groth16_selector};
use boundless_market::telemetry::{
    BrokerHeartbeat, CompletionOutcome, EvalOutcome, RequestCompleted, RequestEvaluated,
    RequestHeartbeat,
};
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ConfigLock;
use crate::db::DbObj;

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

const REQUEST_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const BROKER_HEARTBEAT_INTERVAL_SECS: u64 = 3600;
const EVICTION_INTERVAL_SECS: u64 = 300;
const STALE_ENTRY_SECS: u64 = 7200;

// Internal telemetry events emitted by broker services.
#[derive(Debug)]
pub(crate) enum TelemetryEvent {
    Evaluated {
        /// Composite order ID: "0x{request_id}-{request_digest}-{fulfillment_type}".
        order_id: String,
        request_id: U256,
        request_digest: String,
        requestor: Address,
        outcome: EvalOutcome,
        skip_code: Option<String>,
        skip_reason: Option<String>,
        total_cycles: Option<u64>,
        estimated_total_proving_time_secs: Option<u64>,
        fulfillment_type: String,
        proof_type: String,
        preflight_cache_hit: bool,
        queue_duration_ms: Option<u64>,
        preflight_duration_ms: Option<u64>,
        received_at_timestamp: u64,
    },
    /// The order was committed to the proving pipeline (lock tx succeeded, DB
    /// status set to Proving). Marks the start of proving_duration_secs.
    OrderCommitted {
        order_id: String,
        committed_at: Instant,
        concurrent_proving_jobs: u32,
    },
    ProvingCompleted {
        order_id: String,
        total_cycles: Option<u64>,
        stark_proving_secs: Option<f64>,
        proof_compression_secs: Option<f64>,
    },
    AggregationCompleted {
        order_id: String,
        set_builder_proving_secs: Option<f64>,
        assessor_proving_secs: Option<f64>,
        assessor_compression_proof_secs: Option<f64>,
    },
    Fulfilled {
        order_id: String,
    },
    Failed {
        order_id: String,
        error_code: String,
        error_reason: String,
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

    /// Create a no-op handle where all events are silently dropped.
    pub(crate) fn noop() -> Self {
        let (tx, _rx) = mpsc::channel(1);
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
}

// In-memory tracking for a request as it moves through the pipeline.
struct InFlightRequest {
    received_at_timestamp: u64,
    request_id: Option<U256>,
    request_digest: String,
    fulfillment_type: String,
    proof_type: String,
    estimated_total_proving_time_secs: Option<u64>,
    total_cycles: Option<u64>,
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
            estimated_total_proving_time_secs: None,
            total_cycles: None,
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
            TelemetryEvent::Evaluated {
                order_id,
                request_id,
                request_digest,
                requestor,
                outcome,
                skip_code,
                skip_reason,
                total_cycles,
                estimated_total_proving_time_secs,
                fulfillment_type,
                proof_type,
                preflight_cache_hit,
                queue_duration_ms,
                preflight_duration_ms,
                received_at_timestamp,
            } => {
                let entry =
                    self.in_flight.entry(order_id.clone()).or_insert_with(InFlightRequest::new);
                entry.request_id = Some(request_id);
                entry.request_digest = request_digest.clone();
                entry.fulfillment_type = fulfillment_type.clone();
                entry.proof_type = proof_type.clone();
                entry.estimated_total_proving_time_secs = estimated_total_proving_time_secs;
                entry.total_cycles = total_cycles;
                entry.received_at_timestamp = received_at_timestamp;

                let evaluated = RequestEvaluated {
                    broker_address: self.broker_address,
                    order_id,
                    request_id: format!("0x{request_id:x}"),
                    request_digest,
                    requestor,
                    outcome,
                    skip_code,
                    skip_reason,
                    total_cycles,
                    estimated_total_proving_time_secs,
                    fulfillment_type,
                    preflight_cache_hit,
                    queue_duration_ms,
                    preflight_duration_ms,
                    received_at_timestamp,
                    evaluated_at: Utc::now(),
                };

                tracing::debug!(
                    payload = %serde_json::to_string(&evaluated).unwrap_or_default(),
                    "(Telemetry) Request Evaluated: {}",
                    evaluated.order_id,
                );

                self.eval_buffer.push(evaluated);
            }
            TelemetryEvent::OrderCommitted { order_id, committed_at, concurrent_proving_jobs } => {
                let entry = self.in_flight.entry(order_id).or_insert_with(InFlightRequest::new);
                entry.committed_at = Some(committed_at);
                entry.concurrent_proving_jobs_start = Some(concurrent_proving_jobs);
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
            TelemetryEvent::Failed { order_id, error_code, error_reason } => {
                let outcome = outcome_from_error_code(&error_code);
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

        let submission_duration_secs =
            entry.aggregation_completed_at.map(|a| now.duration_since(a).as_secs());

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
            lock_duration_secs: None,
            proving_duration_secs,
            aggregation_duration_secs,
            submission_duration_secs,
            total_duration_secs,
            estimated_total_proving_time_secs: entry.estimated_total_proving_time_secs,
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

        tracing::debug!(
            payload = %serde_json::to_string(&completed).unwrap_or_default(),
            "(Telemetry) Request Completed: {}",
            order_id,
        );

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
            match client.submit_heartbeat(&heartbeat, &self.signer).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!("Failed to send broker heartbeat: {e}");
                    false
                }
            }
        } else {
            false
        };

        tracing::debug!(
            submitted,
            payload = %serde_json::to_string(&heartbeat).unwrap_or_default(),
            "(Telemetry) Broker Heartbeat",
        );
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
            match client.submit_request_heartbeat(&heartbeat, &self.signer).await {
                Ok(_) => true,
                Err(e) => {
                    tracing::warn!(
                        "Failed to send request heartbeat ({} evals, {} completions): {e}",
                        heartbeat.evaluated.len(),
                        heartbeat.completed.len()
                    );
                    false
                }
            }
        } else {
            false
        };

        tracing::debug!(
            submitted,
            payload = %serde_json::to_string(&heartbeat).unwrap_or_default(),
            "(Telemetry) Request Heartbeat",
        );
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

fn outcome_from_error_code(error_code: &str) -> CompletionOutcome {
    match error_code {
        s if s.starts_with("[B-REAP-") => CompletionOutcome::ExpiredWhileProving,
        s if s.starts_with("[B-OM-") => CompletionOutcome::LockFailed,
        s if s.starts_with("[B-SUB-") => CompletionOutcome::TxFailed,
        s if s.starts_with("[B-PRO-") => CompletionOutcome::ProvingFailed,
        s if s.starts_with("[B-AGG-") => CompletionOutcome::AggregationFailed,
        _ => CompletionOutcome::Unknown,
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
    fn test_noop_handle_drops_silently() {
        let handle = TelemetryHandle::noop();
        handle.record(TelemetryEvent::Fulfilled { order_id: "test-order-1".to_string() });
        assert_eq!(handle.drop_count(), 1);
    }

    #[test]
    fn test_handle_atomic_ops() {
        let handle = TelemetryHandle::noop();

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

    #[test]
    fn test_outcome_from_error_code() {
        assert_eq!(outcome_from_error_code("[B-REAP-003]"), CompletionOutcome::ExpiredWhileProving);
        assert_eq!(outcome_from_error_code("[B-OM-007]"), CompletionOutcome::LockFailed);
        assert_eq!(outcome_from_error_code("[B-SUB-001]"), CompletionOutcome::TxFailed);
        assert_eq!(outcome_from_error_code("[B-PRO-501]"), CompletionOutcome::ProvingFailed);
        assert_eq!(outcome_from_error_code("[B-AGG-001]"), CompletionOutcome::AggregationFailed);
        assert_eq!(outcome_from_error_code("something random"), CompletionOutcome::Unknown);
    }

    fn test_order_id(n: u64) -> String {
        format!("0x{n:x}-0xdeadbeef-LockAndFulfill")
    }

    #[tokio::test]
    async fn test_process_evaluated_event() {
        let mut service = test_service().await;

        let oid = test_order_id(42);
        service
            .process_event(TelemetryEvent::Evaluated {
                order_id: oid.clone(),
                request_id: U256::from(42),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Locked,
                skip_code: None,
                skip_reason: None,
                total_cycles: Some(1_000_000),
                estimated_total_proving_time_secs: Some(30),
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Merkle".to_string(),
                preflight_cache_hit: false,
                queue_duration_ms: Some(500),
                preflight_duration_ms: Some(1200),
                received_at_timestamp: 1700000000,
            })
            .await;

        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].outcome, EvalOutcome::Locked);
        assert!(service.in_flight.contains_key(&oid));
    }

    #[tokio::test]
    async fn test_full_lifecycle_produces_completed() {
        let mut service = test_service().await;
        let oid = test_order_id(100);

        service
            .process_event(TelemetryEvent::Evaluated {
                order_id: oid.clone(),
                request_id: U256::from(100),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Locked,
                skip_code: None,
                skip_reason: None,
                total_cycles: Some(5_000_000),
                estimated_total_proving_time_secs: Some(60),
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Merkle".to_string(),
                preflight_cache_hit: true,
                queue_duration_ms: Some(100),
                preflight_duration_ms: Some(0),
                received_at_timestamp: 1700000000,
            })
            .await;

        service
            .process_event(TelemetryEvent::OrderCommitted {
                order_id: oid.clone(),
                committed_at: Instant::now(),
                concurrent_proving_jobs: 3,
            })
            .await;

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
    async fn test_failed_event_produces_completed() {
        let mut service = test_service().await;
        let oid = test_order_id(200);

        service
            .process_event(TelemetryEvent::Evaluated {
                order_id: oid.clone(),
                request_id: U256::from(200),
                request_digest: "0xdeadbeef".to_string(),
                requestor: Address::ZERO,
                outcome: EvalOutcome::Locked,
                skip_code: None,
                skip_reason: None,
                total_cycles: Some(1_000_000),
                estimated_total_proving_time_secs: None,
                fulfillment_type: "LockAndFulfill".to_string(),
                proof_type: "Groth16".to_string(),
                preflight_cache_hit: false,
                queue_duration_ms: None,
                preflight_duration_ms: None,
                received_at_timestamp: 1700000000,
            })
            .await;

        service
            .process_event(TelemetryEvent::Failed {
                order_id: oid,
                error_code: "[B-PRO-501]".to_string(),
                error_reason: "Proving failed".to_string(),
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
