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

//! Lightweight, cheaply cloneable handle that broker services use to record
//! telemetry events without blocking. The handle owns the channel sender and
//! a few atomic counters; the [`TelemetryService`](super::TelemetryService)
//! drains the receiver side.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use boundless_market::prover_utils::OrderRequest;
use boundless_market::telemetry::EvalOutcome;
use tokio::sync::mpsc;

use crate::errors::BrokerFailure;
use crate::order_locker::{OrderCommitmentMeta, OrderLockerConfig};

use super::event::TelemetryEvent;
use super::proof_type_label;

#[derive(Clone)]
pub(crate) struct TelemetryHandle {
    pub(super) tx: mpsc::Sender<TelemetryEvent>,
    pub(super) drop_count: Arc<AtomicU64>,
    pub(super) pending_preflight_count: Arc<AtomicU32>,
    pub(super) committed_count: Arc<AtomicU32>,
}

impl TelemetryHandle {
    pub(crate) fn new(tx: mpsc::Sender<TelemetryEvent>) -> Self {
        Self {
            tx,
            drop_count: Arc::new(AtomicU64::new(0)),
            pending_preflight_count: Arc::new(AtomicU32::new(0)),
            committed_count: Arc::new(AtomicU32::new(0)),
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

    pub(crate) fn set_committed_count(&self, count: u32) {
        self.committed_count.store(count, Ordering::Relaxed);
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
        queue_duration_ms: u64,
        preflight_duration_ms: u64,
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
            queue_duration_ms: Some(queue_duration_ms),
            preflight_duration_ms: Some(preflight_duration_ms),
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
        config: &OrderLockerConfig,
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

    pub(crate) fn record_application_proving_completed(
        &self,
        order_id: &str,
        total_cycles: Option<u64>,
        stark_proving_secs: Option<f64>,
        proof_compression_secs: Option<f64>,
    ) {
        self.record(TelemetryEvent::ApplicationProvingCompleted {
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
