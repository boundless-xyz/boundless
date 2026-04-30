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

//! Per-chain telemetry pipeline. Each chain registers a [`TelemetryHandle`]
//! that broker services use to record events; a [`TelemetryService`] drains
//! those events, merges them with on-chain status, and emits the
//! `RequestEvaluated` / `RequestCompleted` payloads consumed by the
//! order-stream telemetry endpoint.
//!
//! Layout:
//! - [`event`] — `TelemetryEvent` enum (the broker-internal event types).
//! - [`handle`] — [`TelemetryHandle`] cloneable producer.
//! - [`state`] — `InFlightRequest` / `CommitmentInfo` per-request tracking
//!   plus the `build_request_evaluated` helper.
//! - [`service`] — [`TelemetryService`] consumer + the
//!   [`run_telemetry_service`] driver loop and the chain-level test module.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

use alloy::primitives::FixedBytes;
use boundless_market::selector::{is_blake3_groth16_selector, is_groth16_selector};
use tokio::sync::mpsc;

mod event;
mod handle;
mod service;
mod state;

pub(crate) use event::TelemetryEvent;
pub(crate) use handle::TelemetryHandle;
pub(crate) use service::{run_telemetry_service, TelemetryService};

static CHAIN_HANDLES: LazyLock<Mutex<HashMap<u64, TelemetryHandle>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NOOP_HANDLE: OnceLock<TelemetryHandle> = OnceLock::new();

/// Sum a per-chain counter across all registered telemetry handles.
fn sum_chain_handles(f: impl Fn(&TelemetryHandle) -> u32) -> u32 {
    CHAIN_HANDLES.lock().map(|handles| handles.values().map(&f).sum()).unwrap_or(0)
}

fn global_committed_count() -> u32 {
    sum_chain_handles(|h| h.committed_count.load(std::sync::atomic::Ordering::Relaxed))
}

fn global_pending_preflight_count() -> u32 {
    sum_chain_handles(|h| h.pending_preflight_count.load(std::sync::atomic::Ordering::Relaxed))
}

/// Initializes the telemetry handle for a specific chain. Each chain gets its own
/// handle and underlying TelemetryService that sends heartbeats to its order-stream URL.
pub(crate) fn init_for_chain<F>(chain_id: u64, build_handle: F) -> TelemetryHandle
where
    F: FnOnce() -> TelemetryHandle,
{
    let mut map = CHAIN_HANDLES.lock().unwrap();
    if let Some(handle) = map.get(&chain_id) {
        tracing::warn!(chain_id, "Telemetry handle already initialized for chain");
        return handle.clone();
    }

    let handle = build_handle();
    tracing::debug!(chain_id, "Telemetry handle initialized");
    map.insert(chain_id, handle.clone());
    handle
}

/// Returns the telemetry handle for the given chain, or a noop fallback if none was registered.
pub(crate) fn telemetry(chain_id: u64) -> TelemetryHandle {
    CHAIN_HANDLES.lock().unwrap().get(&chain_id).cloned().unwrap_or_else(noop_handle)
}

fn noop_handle() -> TelemetryHandle {
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

pub(crate) fn proof_type_label(selector: FixedBytes<4>) -> &'static str {
    if is_groth16_selector(selector) {
        "Groth16"
    } else if is_blake3_groth16_selector(selector) {
        "Blake3Groth16"
    } else {
        "Merkle"
    }
}
