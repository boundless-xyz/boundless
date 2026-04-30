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

//! [`TelemetryService`] — drains [`TelemetryEvent`]s emitted by broker
//! services, merges them with on-chain status, and pushes
//! `RequestEvaluated` / `RequestCompleted` payloads upstream.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use anyhow::Result;
use boundless_market::order_stream_client::OrderStreamClient;
use boundless_market::telemetry::{
    BrokerHeartbeat, CommitmentOutcome, CompletionOutcome, EvalOutcome, RequestCompleted,
    RequestEvaluated, RequestHeartbeat,
};
use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ConfigLock;
use crate::db::DbObj;
use crate::futures_retry::retry;

use super::event::TelemetryEvent;
use super::handle::TelemetryHandle;
use super::state::{build_request_evaluated, CommitmentInfo, InFlightRequest};
use super::{global_committed_count, global_pending_preflight_count, now_unix};

const REQUEST_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const BROKER_HEARTBEAT_INTERVAL_SECS: u64 = 60;
const EVICTION_INTERVAL_SECS: u64 = 300;
const STALE_ENTRY_SECS: u64 = 7200;
const HEARTBEAT_RETRY_COUNT: u64 = 2;
const HEARTBEAT_RETRY_SLEEP_MS: u64 = 1000;

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
            TelemetryEvent::ApplicationProvingCompleted {
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

        let committed_to_application_proof_duration_secs =
            match (entry.committed_at, entry.proving_completed_at) {
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

        // Path A: committed_at to proving_completed_at (STARK + compression).
        // Path B: committed_at to aggregation_completed_at (STARK + set builder + assessor + batch compression).
        let actual_total_proving_time_secs = entry.committed_at.and_then(|committed| {
            let all_proofs_done = entry.aggregation_completed_at.or(entry.proving_completed_at)?;
            Some(all_proofs_done.duration_since(committed).as_secs())
        });

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
            committed_to_application_proof_duration_secs,
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
        self.handle.set_committed_count(committed_orders_count);

        let config_value = match self.config.lock_all() {
            Ok(c) => serde_json::to_value(&*c).unwrap_or(serde_json::Value::Null),
            Err(_) => serde_json::Value::Null,
        };

        let heartbeat = BrokerHeartbeat {
            broker_address: self.broker_address,
            config: config_value,
            committed_orders_count,
            global_committed_orders_count: global_committed_count(),
            pending_preflight_count: self.handle.pending_preflight(),
            global_pending_preflight_count: global_pending_preflight_count(),
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::primitives::U256;

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
                skip_commit_code: Some("[B-OL-001]".to_string()),
                skip_commit_reason: Some("Lock failed".to_string()),
                lock_submitted_at: Some(Instant::now()),
            })
            .await;

        assert_eq!(service.eval_buffer.len(), 1);
        assert_eq!(service.eval_buffer[0].commitment_outcome, Some(CommitmentOutcome::Dropped));
        assert_eq!(service.eval_buffer[0].commitment_skip_code, Some("[B-OL-001]".to_string()));
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
            .process_event(TelemetryEvent::ApplicationProvingCompleted {
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

    /// Two-phase shutdown contract: the broker uses `non_critical_cancel_token`
    /// for tasks like OrderPricer/OrderLocker (Phase 1, immediate cancel) and
    /// `critical_cancel_token` for tasks like ProvingService/Submitter (Phase 2,
    /// waited up to 2 h while in-flight orders drain).
    ///
    /// Telemetry must be bound to the **critical** token so monitoring keeps
    /// seeing broker heartbeats during the Phase 2 drain. If it were bound to
    /// the non-critical token, heartbeats would stop within seconds of SIGTERM
    /// while the broker is still actively proving — causing false "broker
    /// offline" alarms.
    ///
    /// This test validates the function-level contract of `run_telemetry_service`:
    /// when given a token that has not been cancelled, it keeps running and
    /// emitting heartbeats indefinitely. The lib.rs wiring is what binds it to
    /// the right token; this test makes sure the function itself respects that
    /// contract correctly.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_run_telemetry_only_exits_on_cancel() {
        // Simulate the broker's two cancel tokens.
        let non_critical_cancel = CancellationToken::new();
        let critical_cancel = CancellationToken::new();

        let (_tx, rx) = mpsc::channel(16);
        let handle = TelemetryHandle::new(_tx.clone());
        let signer = PrivateKeySigner::random();
        let config = ConfigLock::default();
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());

        // Short intervals so the broker_heartbeat tick fires quickly inside the test.
        let mut service = TelemetryService::new_debug(rx, handle, signer, config, db);
        service.broker_heartbeat_interval_secs = 1;
        service.request_heartbeat_interval_secs = 1;

        // Wire telemetry to critical_cancel (the FIX). The bug was that lib.rs
        // wired this to non_critical_cancel, so Phase 1 cancellation killed it.
        let cancel_token = critical_cancel.clone();
        let join = tokio::spawn(async move { run_telemetry_service(service, cancel_token).await });

        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!join.is_finished(), "service should be running after startup");

        // Phase 1: non-critical tasks get cancelled. Telemetry is bound to
        // critical_cancel, so it must NOT exit here.
        non_critical_cancel.cancel();
        tokio::time::sleep(Duration::from_secs(3)).await;
        assert!(
            !join.is_finished(),
            "telemetry exited during Phase 1 (non_critical_cancel); \
             must survive until critical_cancel fires so monitoring continues to see \
             broker heartbeats during the in-flight-order drain phase",
        );

        // Phase 2: critical token cancels. Telemetry should now exit cleanly
        // after a final heartbeat flush.
        critical_cancel.cancel();
        let result = tokio::time::timeout(Duration::from_secs(5), join).await;
        assert!(result.is_ok(), "telemetry did not exit within 5s of critical_cancel");
    }
}
