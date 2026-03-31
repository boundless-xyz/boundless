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

//! Singleton OrderCommitter that manages global proving capacity across all chains.
//!
//! Receives priced orders from all per-chain OrderPricers via a unified channel,
//! applies capacity constraints (`max_concurrent_proofs`, `peak_prove_khz`), and
//! dispatches orders to per-chain [`crate::order_locker::OrderLocker`] instances.
//!
//! Capacity lifecycle:
//! - A slot in `in_flight` is consumed when an order is dispatched to a per-chain locker.
//! - A slot is freed when a [`CommitmentComplete`] is received with one of:
//!   - [`CommitmentOutcome::Skipped`] — order failed validation/locking, never entered proving
//!   - [`CommitmentOutcome::ProvingCompleted`] — order fulfilled on-chain (from Submitter)
//!   - [`CommitmentOutcome::ProvingFailed`] — proof/aggregation/submission failed (from any pipeline component)
//! - Capacity is NOT freed on lock success — the slot stays held until the full pipeline finishes.
//!
//! Orders with a future `target_timestamp` are held in `pending_orders` and only dispatched
//! once the target time is reached. This prevents reserving capacity for orders that can't
//! be acted on yet (e.g. `FulfillAfterLockExpire` waiting for lock expiry).
//!
//! [`OrderStateChange`] events (Locked/Fulfilled) are filtered by both `request_id` and
//! `chain_id` so that events on one chain don't incorrectly remove pending orders from
//! another chain that shares the same `request_id`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use crate::{
    config::{ConfigLock, OrderCommitmentPriority},
    errors::CodedError,
    now_timestamp,
    order_locker::OrderCommitmentMeta,
    prioritization::prioritize_orders_to_commit,
    requestor_monitor::PriorityRequestors,
    task::{RetryRes, RetryTask, SupervisorErr},
    FulfillmentType, OrderRequest, OrderStateChange,
};

const MIN_CAPACITY_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const MAX_PROVING_BATCH_SIZE: usize = 10;
const DEFAULT_MAX_COMMITMENT_DURATION_SECS: u64 = 7200;

/// Reason a proving capacity slot was released. Sent via [`CommitmentComplete`]
/// from the OrderLocker (for `Skipped`) or proving pipeline components
/// (for `ProvingCompleted`/`ProvingFailed`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum CommitmentOutcome {
    /// Order failed validation or locking in the OrderLocker and never entered the
    /// proving pipeline. Capacity is freed immediately.
    Skipped,
    /// Order was proven, aggregated, and fulfilled on-chain by the Submitter.
    ProvingCompleted,
    /// Order failed somewhere in the proving pipeline (ProvingService, Aggregator,
    /// Submitter, or ReaperTask).
    ProvingFailed,
}

impl std::fmt::Display for CommitmentOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommitmentOutcome::Skipped => write!(f, "Skipped"),
            CommitmentOutcome::ProvingCompleted => write!(f, "ProvingCompleted"),
            CommitmentOutcome::ProvingFailed => write!(f, "ProvingFailed"),
        }
    }
}

/// Capacity release signal sent back to the OrderCommitter to free an `in_flight` slot.
/// Produced by the OrderLocker (Skipped) and proving pipeline (ProvingCompleted/Failed).
pub(crate) struct CommitmentComplete {
    pub order_id: String,
    pub chain_id: u64,
    pub outcome: CommitmentOutcome,
}

struct InFlightOrder {
    dispatched_at: Instant,
    total_cycles: Option<u64>,
    proving_started_at: Option<u64>,
}

#[derive(Error, Debug)]
pub(crate) enum OrderCommitterErr {
    #[error("{code} Config read error: {0}", code = self.code())]
    ConfigReadErr(Arc<anyhow::Error>),

    #[error("{code} Stale capacity for order {order_id} expired after {elapsed_secs}s with no completion", code = self.code())]
    StaleCapacity { order_id: String, elapsed_secs: u64 },
}

impl CodedError for OrderCommitterErr {
    fn code(&self) -> &str {
        match self {
            OrderCommitterErr::ConfigReadErr(_) => "[B-OC-001]",
            OrderCommitterErr::StaleCapacity { .. } => "[B-OC-004]",
        }
    }
}

struct CommitterConfig {
    max_concurrent_proofs: usize,
    peak_prove_khz: Option<u64>,
    additional_proof_cycles: u64,
    batch_buffer_time_secs: u64,
    order_commitment_priority: OrderCommitmentPriority,
    priority_addresses: Option<Vec<Address>>,
}

/// Singleton service that manages global proving capacity across all chains.
///
/// Receives priced orders from all per-chain OrderPricers, holds them in `pending_orders`
/// until their `target_timestamp` is reached and capacity is available, then dispatches
/// to per-chain OrderLockers. Tracks dispatched orders in `in_flight` and frees slots
/// when [`CommitmentComplete`] messages arrive from the OrderLocker or proving pipeline.
#[derive(Clone)]
pub(crate) struct OrderCommitter {
    config: ConfigLock,
    priced_order_rx: Arc<Mutex<mpsc::Receiver<Box<OrderRequest>>>>,
    locker_dispatchers: Arc<HashMap<u64, mpsc::Sender<Box<OrderRequest>>>>,
    proving_completion_rx: Arc<Mutex<mpsc::Receiver<CommitmentComplete>>>,
    order_state_tx: broadcast::Sender<OrderStateChange>,
    priority_requestors: Arc<HashMap<u64, PriorityRequestors>>,
}

/// Records telemetry for orders skipped at the committer level (e.g. expiration, channel closed).
/// Successful commitment telemetry is recorded by the per-chain OrderLocker, not here.
fn record_skip_telemetry(
    order: &OrderRequest,
    committer_config: &CommitterConfig,
    pending_count: u32,
    code: &str,
    reason: &str,
    meta: Option<&OrderCommitmentMeta>,
) {
    use crate::order_locker::OrderLockerConfig;

    let monitor_wait = order.priced_at_timestamp.map(|t| now_timestamp().saturating_sub(t) * 1000);
    let telem_config = OrderLockerConfig {
        peak_prove_khz: committer_config.peak_prove_khz,
        max_concurrent_proofs: Some(committer_config.max_concurrent_proofs as u32),
        ..Default::default()
    };
    crate::telemetry::telemetry(order.chain_id).record_order_commitment(
        &order.id(),
        false,
        meta,
        monitor_wait,
        &telem_config,
        pending_count,
        Some(code),
        Some(reason),
        None,
    );
}

impl OrderCommitter {
    pub fn new(
        config: ConfigLock,
        priced_order_rx: mpsc::Receiver<Box<OrderRequest>>,
        locker_dispatchers: HashMap<u64, mpsc::Sender<Box<OrderRequest>>>,
        proving_completion_rx: mpsc::Receiver<CommitmentComplete>,
        order_state_tx: broadcast::Sender<OrderStateChange>,
        priority_requestors: HashMap<u64, PriorityRequestors>,
    ) -> Self {
        Self {
            config,
            priced_order_rx: Arc::new(Mutex::new(priced_order_rx)),
            locker_dispatchers: Arc::new(locker_dispatchers),
            proving_completion_rx: Arc::new(Mutex::new(proving_completion_rx)),
            order_state_tx,
            priority_requestors: Arc::new(priority_requestors),
        }
    }

    fn read_config(&self) -> Result<CommitterConfig, OrderCommitterErr> {
        let cfg = self.config.lock_all().map_err(|err| {
            OrderCommitterErr::ConfigReadErr(Arc::new(anyhow::anyhow!(
                "Failed to read config: {err}"
            )))
        })?;
        Ok(CommitterConfig {
            max_concurrent_proofs: cfg.market.max_concurrent_proofs as usize,
            peak_prove_khz: cfg.market.peak_prove_khz,
            additional_proof_cycles: cfg.market.additional_proof_cycles,
            batch_buffer_time_secs: cfg.batcher.block_deadline_buffer_secs,
            order_commitment_priority: cfg.market.order_commitment_priority,
            priority_addresses: cfg.market.priority_requestor_addresses.clone(),
        })
    }

    fn collect_priority_addresses(&self, static_addresses: Option<Vec<Address>>) -> Vec<Address> {
        let mut merged = static_addresses.unwrap_or_default();
        for pr in self.priority_requestors.values() {
            merged.extend(pr.dynamic_addresses());
        }
        merged
    }

    #[allow(clippy::vec_box)]
    fn format_per_chain_counts(orders: &[Box<OrderRequest>]) -> String {
        let mut counts: HashMap<u64, usize> = HashMap::new();
        for order in orders {
            *counts.entry(order.chain_id).or_default() += 1;
        }
        let mut entries: Vec<_> = counts.into_iter().collect();
        entries.sort_by_key(|(chain_id, _)| *chain_id);
        entries
            .iter()
            .map(|(chain_id, count)| format!("chain {chain_id}: {count}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn estimate_prover_available_at(
        in_flight: &HashMap<String, InFlightOrder>,
        peak_prove_khz: u64,
        additional_proof_cycles: u64,
    ) -> u64 {
        let now = now_timestamp();

        if in_flight.is_empty() {
            return now;
        }

        let total_committed_cycles: u64 = in_flight
            .values()
            .filter_map(|o| o.total_cycles.map(|c| c + additional_proof_cycles))
            .sum();

        let started_proving_at =
            in_flight.values().filter_map(|o| o.proving_started_at).min().unwrap_or(now);

        let proof_time_seconds = total_committed_cycles.div_ceil(1_000).div_ceil(peak_prove_khz);
        let prover_available_at = started_proving_at + proof_time_seconds;

        if prover_available_at < now {
            let seconds_behind = now - prover_available_at;
            tracing::warn!(
                "Proofs are behind what is estimated from peak_prove_khz config by {} seconds. \
                 Consider lowering this value to avoid overlocking orders.",
                seconds_behind
            );
            return now;
        }

        prover_available_at
    }

    /// Selects ready orders from `pending_orders` and dispatches them to per-chain lockers.
    ///
    /// 1. Partitions `pending_orders` into ready (target_timestamp reached) and not-ready.
    /// 2. Prioritizes ready orders via `prioritize_orders_to_commit`.
    /// 3. Applies `peak_prove_khz` timing filter (can order finish before expiry?).
    /// 4. Reserves `in_flight` slots and sends to per-chain dispatch channels.
    /// 5. On send failure, removes the slot and re-queues (Full) or records telemetry (Closed).
    #[allow(clippy::vec_box)]
    fn commit_orders(
        &self,
        pending_orders: &mut Vec<Box<OrderRequest>>,
        in_flight: &mut HashMap<String, InFlightOrder>,
        committer_config: &CommitterConfig,
        priority_addresses: &[Address],
    ) {
        if pending_orders.is_empty() {
            return;
        }

        if in_flight.len() >= committer_config.max_concurrent_proofs {
            tracing::debug!(
                pending_commitment_order_count = pending_orders.len(),
                in_flight_count = in_flight.len(),
                max_concurrent_proofs = committer_config.max_concurrent_proofs,
                "Order committer at capacity, deferring {} pending orders",
                pending_orders.len(),
            );
            return;
        }

        let now = now_timestamp();
        let (mut ready, not_ready): (Vec<_>, Vec<_>) = pending_orders
            .drain(..)
            .partition(|order| order.target_timestamp.is_none_or(|t| t <= now));
        *pending_orders = not_ready;

        if ready.is_empty() {
            return;
        }

        let available_capacity = committer_config
            .max_concurrent_proofs
            .saturating_sub(in_flight.len())
            .min(MAX_PROVING_BATCH_SIZE);

        let priority_ref =
            if priority_addresses.is_empty() { None } else { Some(priority_addresses) };
        let mut selected = prioritize_orders_to_commit(
            &mut ready,
            committer_config.order_commitment_priority,
            priority_ref,
            available_capacity,
        );

        let pending_ready_commitment_order_count = ready.len();
        pending_orders.extend(ready);

        if pending_ready_commitment_order_count > 0 {
            tracing::debug!(
                pending_ready_commitment_order_count,
                pending_commitment_order_count = pending_orders.len(),
                in_flight_count = in_flight.len(),
                max_concurrent_proofs = committer_config.max_concurrent_proofs,
                dispatching = selected.len(),
                "Ready orders waiting for capacity",
            );
        }

        if let Some(peak_prove_khz) = committer_config.peak_prove_khz {
            let mut prover_available_at = Self::estimate_prover_available_at(
                in_flight,
                peak_prove_khz,
                committer_config.additional_proof_cycles,
            );

            let now = now_timestamp();
            let mut kept = Vec::with_capacity(selected.len());
            for order in selected {
                let Some(order_cycles) = order.total_cycles else {
                    kept.push(order);
                    continue;
                };

                let total_cycles = order_cycles + committer_config.additional_proof_cycles;
                let proof_time_seconds = total_cycles.div_ceil(1_000).div_ceil(peak_prove_khz);
                let completion_time = prover_available_at + proof_time_seconds;
                let expiration = order.expiry();

                if completion_time + committer_config.batch_buffer_time_secs > expiration {
                    if now + proof_time_seconds > expiration {
                        tracing::info!(
                            "Order {} cannot be completed before its expiration at {}, \
                             proof estimated to take {} seconds. Skipping permanently",
                            order.id(),
                            expiration,
                            proof_time_seconds,
                        );
                        let meta = OrderCommitmentMeta {
                            estimated_proving_time_secs: Some(completion_time.saturating_sub(now)),
                            estimated_proving_time_no_load_secs: Some(proof_time_seconds),
                            concurrent_proving_jobs: in_flight.len() as u32,
                        };
                        record_skip_telemetry(
                            &order,
                            committer_config,
                            pending_orders.len() as u32,
                            "[B-OC-005]",
                            "Cannot be completed before expiration",
                            Some(&meta),
                        );
                    } else {
                        tracing::debug!(
                            "Given current committed orders and capacity, order {} cannot be \
                             completed before its expiration. Re-queuing as capacity may free up.",
                            order.id(),
                        );
                        pending_orders.push(order);
                    }
                    continue;
                }

                prover_available_at = completion_time;
                kept.push(order);
            }
            selected = kept;
        }

        for order in selected {
            let order_id = order.id();
            let chain_id = order.chain_id;
            let total_cycles = order.total_cycles;

            let Some(locker_tx) = self.locker_dispatchers.get(&chain_id) else {
                tracing::warn!(
                    chain_id,
                    "[B-OC-002] Dropping order {order_id}: no dispatch channel for chain"
                );
                continue;
            };

            in_flight.insert(
                order_id.clone(),
                InFlightOrder {
                    dispatched_at: Instant::now(),
                    total_cycles,
                    proving_started_at: Some(now_timestamp()),
                },
            );

            match locker_tx.try_send(order) {
                Ok(()) => {
                    tracing::debug!(
                        chain_id,
                        "Dispatched order {order_id} for commitment ({} in-flight, {} pending)",
                        in_flight.len(),
                        pending_orders.len(),
                    );
                }
                Err(mpsc::error::TrySendError::Full(order)) => {
                    in_flight.remove(&order_id);
                    tracing::warn!(
                        chain_id,
                        "Dispatch channel full for chain {chain_id}, re-queuing order {}",
                        order.id()
                    );
                    pending_orders.push(order);
                }
                Err(mpsc::error::TrySendError::Closed(order)) => {
                    in_flight.remove(&order_id);
                    tracing::error!(
                        chain_id,
                        "[B-OC-003] Dispatch channel closed for chain {chain_id}, dropping order {order_id}"
                    );
                    record_skip_telemetry(
                        &order,
                        committer_config,
                        pending_orders.len() as u32,
                        "[B-OC-003]",
                        "Dispatch channel closed",
                        None,
                    );
                }
            }
        }
    }

    fn reap_stale_capacity(in_flight: &mut HashMap<String, InFlightOrder>, max_age_secs: u64) {
        let threshold = Duration::from_secs(max_age_secs);
        let stale: Vec<String> = in_flight
            .iter()
            .filter(|(_, info)| info.dispatched_at.elapsed() > threshold)
            .map(|(id, _)| id.clone())
            .collect();

        for order_id in stale {
            if let Some(info) = in_flight.remove(&order_id) {
                let err = OrderCommitterErr::StaleCapacity {
                    order_id: order_id.clone(),
                    elapsed_secs: info.dispatched_at.elapsed().as_secs(),
                };
                tracing::warn!("{err}");
            }
        }
    }
}

impl RetryTask for OrderCommitter {
    type Error = OrderCommitterErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let committer = self.clone();

        Box::pin(async move {
            tracing::info!("Starting order committer");

            let mut committer_config = committer.read_config().map_err(SupervisorErr::Fault)?;
            let mut priority_addresses =
                committer.collect_priority_addresses(committer_config.priority_addresses.take());

            let mut priced_order_rx = committer.priced_order_rx.lock().await;
            let mut proving_completion_rx = committer.proving_completion_rx.lock().await;
            let mut order_state_rx = committer.order_state_tx.subscribe();
            let mut capacity_check_interval = tokio::time::interval(MIN_CAPACITY_CHECK_INTERVAL);

            let mut pending_orders: Vec<Box<OrderRequest>> = Vec::new();
            let mut in_flight: HashMap<String, InFlightOrder> = HashMap::new();

            loop {
                tokio::select! {
                    Some(order) = priced_order_rx.recv() => {
                        let order_id = order.id();
                        let chain_id = order.chain_id;
                        pending_orders.push(order);
                        tracing::debug!(
                            chain_id,
                            "Order committer queued order {order_id} ({} pending [{}], {} in-flight)",
                            pending_orders.len(),
                            Self::format_per_chain_counts(&pending_orders),
                            in_flight.len(),
                        );
                    }

                    Some(completion) = proving_completion_rx.recv() => {
                        if in_flight.remove(&completion.order_id).is_some() {
                            tracing::debug!(
                                chain_id = completion.chain_id,
                                "Commitment complete for order {} ({}), {} in-flight remaining",
                                completion.order_id,
                                completion.outcome,
                                in_flight.len(),
                            );
                        } else {
                            tracing::trace!(
                                chain_id = completion.chain_id,
                                "Received commitment completion for order {} not in in-flight map (possibly reaped)",
                                completion.order_id,
                            );
                        }
                    }

                    Ok(state_change) = order_state_rx.recv() => {
                        match state_change {
                            OrderStateChange::Locked { request_id, prover, chain_id } => {
                                tracing::debug!(
                                    chain_id,
                                    "Order committer: request 0x{:x} on chain {chain_id} locked by {:x}, removing pending LockAndFulfill orders",
                                    request_id, prover,
                                );
                                let initial_len = pending_orders.len();
                                pending_orders.retain(|order| {
                                    let same_request = U256::from(order.request.id) == request_id;
                                    let same_chain = order.chain_id == chain_id;
                                    let is_lock_and_fulfill = order.fulfillment_type == FulfillmentType::LockAndFulfill;
                                    !(same_request && same_chain && is_lock_and_fulfill)
                                });
                                let removed = initial_len - pending_orders.len();
                                if removed > 0 {
                                    tracing::debug!(
                                        "Removed {removed} LockAndFulfill orders from commitment queue for locked request 0x{:x}",
                                        request_id,
                                    );
                                }
                            }
                            OrderStateChange::Fulfilled { request_id, chain_id } => {
                                tracing::debug!(
                                    chain_id,
                                    "Order committer: request 0x{:x} on chain {chain_id} fulfilled, removing all pending orders",
                                    request_id,
                                );
                                let initial_len = pending_orders.len();
                                pending_orders.retain(|order| {
                                    !(U256::from(order.request.id) == request_id && order.chain_id == chain_id)
                                });
                                let removed = initial_len - pending_orders.len();
                                if removed > 0 {
                                    tracing::debug!(
                                        "Removed {removed} orders from commitment queue for fulfilled request 0x{:x}",
                                        request_id,
                                    );
                                }
                            }
                        }
                    }

                    _ = capacity_check_interval.tick() => {
                        let new_config = committer.read_config().map_err(SupervisorErr::Fault)?;

                        if new_config.max_concurrent_proofs != committer_config.max_concurrent_proofs {
                            tracing::debug!(
                                "Order committer proving capacity changed from {} to {}",
                                committer_config.max_concurrent_proofs, new_config.max_concurrent_proofs,
                            );
                        }
                        if new_config.peak_prove_khz != committer_config.peak_prove_khz {
                            tracing::debug!(
                                "Order committer peak_prove_khz changed from {:?} to {:?}",
                                committer_config.peak_prove_khz, new_config.peak_prove_khz,
                            );
                        }
                        if new_config.order_commitment_priority != committer_config.order_commitment_priority {
                            tracing::debug!(
                                "Order committer commitment priority changed from {:?} to {:?}",
                                committer_config.order_commitment_priority, new_config.order_commitment_priority,
                            );
                        }

                        committer_config = new_config;
                        priority_addresses = committer
                            .collect_priority_addresses(committer_config.priority_addresses.take());

                        Self::reap_stale_capacity(
                            &mut in_flight,
                            DEFAULT_MAX_COMMITMENT_DURATION_SECS,
                        );
                    }

                    _ = cancel_token.cancelled() => {
                        tracing::info!("Order committer received cancellation, shutting down");
                        break;
                    }
                }

                committer.commit_orders(
                    &mut pending_orders,
                    &mut in_flight,
                    &committer_config,
                    &priority_addresses,
                );
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{now_timestamp, FulfillmentType};
    use alloy::primitives::{Address, Bytes};
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use risc0_zkvm::sha::Digest;
    use tokio::time::timeout;

    fn make_order(
        chain_id: u64,
        request_index: u32,
        fulfillment_type: FulfillmentType,
    ) -> Box<OrderRequest> {
        let now = now_timestamp();
        let mut order = Box::new(OrderRequest::new(
            ProofRequest::new(
                RequestId::new(Address::ZERO, request_index),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "http://example.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1u64),
                    maxPrice: U256::from(100u64),
                    rampUpStart: now.saturating_sub(100),
                    timeout: 600,
                    lockTimeout: 300,
                    rampUpPeriod: 1,
                    lockCollateral: U256::ZERO,
                },
            ),
            Bytes::default(),
            fulfillment_type,
            Address::ZERO,
            chain_id,
        ));
        order.total_cycles = Some(1_000_000);
        order
    }

    #[allow(clippy::type_complexity)]
    fn setup_committer(
        max_concurrent_proofs: u32,
        chain_ids: &[u64],
    ) -> (
        OrderCommitter,
        mpsc::Sender<Box<OrderRequest>>,
        mpsc::Sender<CommitmentComplete>,
        broadcast::Sender<OrderStateChange>,
        HashMap<u64, mpsc::Receiver<Box<OrderRequest>>>,
    ) {
        let config = ConfigLock::default();
        config.load_write().unwrap().market.max_concurrent_proofs = max_concurrent_proofs;

        let (order_tx, order_rx) = mpsc::channel(100);
        let (proving_completion_tx, proving_completion_rx) = mpsc::channel(100);
        let (state_tx, _) = broadcast::channel(100);

        let mut dispatchers = HashMap::new();
        let mut locker_rxs = HashMap::new();
        for &chain_id in chain_ids {
            let (tx, rx) = mpsc::channel(100);
            dispatchers.insert(chain_id, tx);
            locker_rxs.insert(chain_id, rx);
        }

        let mut priority_requestors_map = HashMap::new();
        for &chain_id in chain_ids {
            priority_requestors_map
                .insert(chain_id, PriorityRequestors::new(config.clone(), chain_id));
        }

        let committer = OrderCommitter::new(
            config,
            order_rx,
            dispatchers,
            proving_completion_rx,
            state_tx.clone(),
            priority_requestors_map,
        );

        (committer, order_tx, proving_completion_tx, state_tx, locker_rxs)
    }

    async fn recv_with_timeout<T>(rx: &mut mpsc::Receiver<T>) -> T {
        timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("timed out waiting for message")
            .expect("channel closed")
    }

    async fn assert_no_message<T: std::fmt::Debug>(rx: &mut mpsc::Receiver<T>) {
        let result = timeout(Duration::from_millis(100), rx.recv()).await;
        assert!(result.is_err(), "Expected no message but got one");
    }

    #[tokio::test]
    async fn test_dispatches_up_to_capacity() {
        let (committer, order_tx, _proving_completion_tx, _state_tx, mut locker_rxs) =
            setup_committer(2, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let locker_rx = locker_rxs.get_mut(&1).unwrap();

        for i in 0..4 {
            order_tx.send(make_order(1, i, FulfillmentType::LockAndFulfill)).await.unwrap();
        }

        let _ = recv_with_timeout(locker_rx).await;
        let _ = recv_with_timeout(locker_rx).await;

        assert_no_message(locker_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_completion_frees_capacity() {
        let (committer, order_tx, proving_completion_tx, _state_tx, mut locker_rxs) =
            setup_committer(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let locker_rx = locker_rxs.get_mut(&1).unwrap();

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();

        let first = recv_with_timeout(locker_rx).await;
        assert_no_message(locker_rx).await;

        proving_completion_tx
            .send(CommitmentComplete {
                order_id: first.id(),
                chain_id: 1,
                outcome: CommitmentOutcome::ProvingCompleted,
            })
            .await
            .unwrap();

        let _ = recv_with_timeout(locker_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_lock_event_removes_pending() {
        let (committer, order_tx, proving_completion_tx, state_tx, mut locker_rxs) =
            setup_committer(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let locker_rx = locker_rxs.get_mut(&1).unwrap();

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        let first = recv_with_timeout(locker_rx).await;

        let queued_order = make_order(1, 1, FulfillmentType::LockAndFulfill);
        let queued_request_id: U256 = queued_order.request.id;
        order_tx.send(queued_order).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let mut fale_order = *make_order(1, 99, FulfillmentType::FulfillAfterLockExpire);
        fale_order.request.id = RequestId::new(Address::ZERO, 1).into();
        order_tx.send(Box::new(fale_order)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        state_tx
            .send(OrderStateChange::Locked {
                request_id: queued_request_id,
                prover: Address::ZERO,
                chain_id: 1,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        proving_completion_tx
            .send(CommitmentComplete {
                order_id: first.id(),
                chain_id: 1,
                outcome: CommitmentOutcome::ProvingCompleted,
            })
            .await
            .unwrap();

        let dispatched = recv_with_timeout(locker_rx).await;
        assert_eq!(dispatched.fulfillment_type, FulfillmentType::FulfillAfterLockExpire);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_fulfill_event_removes_pending() {
        let (committer, order_tx, proving_completion_tx, state_tx, mut locker_rxs) =
            setup_committer(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let locker_rx = locker_rxs.get_mut(&1).unwrap();

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        let first = recv_with_timeout(locker_rx).await;

        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();
        let mut fale_order = *make_order(1, 99, FulfillmentType::FulfillAfterLockExpire);
        fale_order.request.id = RequestId::new(Address::ZERO, 1).into();
        order_tx.send(Box::new(fale_order)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        let fulfilled_request_id: U256 = RequestId::new(Address::ZERO, 1).into();
        state_tx
            .send(OrderStateChange::Fulfilled { request_id: fulfilled_request_id, chain_id: 1 })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        proving_completion_tx
            .send(CommitmentComplete {
                order_id: first.id(),
                chain_id: 1,
                outcome: CommitmentOutcome::ProvingCompleted,
            })
            .await
            .unwrap();

        assert_no_message(locker_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_fulfill_event_does_not_remove_other_chain() {
        let (committer, order_tx, proving_completion_tx, state_tx, mut locker_rxs) =
            setup_committer(1, &[1, 8453]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let chain_1_rx = locker_rxs.get_mut(&1).unwrap();

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        let first = recv_with_timeout(chain_1_rx).await;

        // Queue an order on chain 8453 with the same request_index (same request_id)
        order_tx.send(make_order(8453, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Fulfill the chain 1 order — should NOT remove the chain 8453 order
        let fulfilled_request_id: U256 = RequestId::new(Address::ZERO, 0).into();
        state_tx
            .send(OrderStateChange::Fulfilled { request_id: fulfilled_request_id, chain_id: 1 })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Free capacity from chain 1 order
        proving_completion_tx
            .send(CommitmentComplete {
                order_id: first.id(),
                chain_id: 1,
                outcome: CommitmentOutcome::ProvingCompleted,
            })
            .await
            .unwrap();

        // Chain 8453 order should still be dispatched
        let chain_8453_rx = locker_rxs.get_mut(&8453).unwrap();
        let dispatched = recv_with_timeout(chain_8453_rx).await;
        assert_eq!(dispatched.chain_id, 8453);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_routes_to_correct_chain() {
        let (committer, order_tx, _proving_completion_tx, _state_tx, mut locker_rxs) =
            setup_committer(10, &[1, 8453]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(8453, 1, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(1, 2, FulfillmentType::LockAndFulfill)).await.unwrap();

        {
            let chain_1_rx = locker_rxs.get_mut(&1).unwrap();
            let dispatched_1a = recv_with_timeout(chain_1_rx).await;
            assert_eq!(dispatched_1a.chain_id, 1);
        }

        {
            let chain_8453_rx = locker_rxs.get_mut(&8453).unwrap();
            let dispatched_8453 = recv_with_timeout(chain_8453_rx).await;
            assert_eq!(dispatched_8453.chain_id, 8453);
        }

        {
            let chain_1_rx = locker_rxs.get_mut(&1).unwrap();
            let dispatched_1b = recv_with_timeout(chain_1_rx).await;
            assert_eq!(dispatched_1b.chain_id, 1);
        }

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_unknown_chain_drops_order() {
        let (committer, order_tx, _proving_completion_tx, _state_tx, mut locker_rxs) =
            setup_committer(10, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        let chain_1_rx = locker_rxs.get_mut(&1).unwrap();

        order_tx.send(make_order(9999, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();

        let dispatched = recv_with_timeout(chain_1_rx).await;
        assert_eq!(dispatched.chain_id, 1);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_config_reload_changes_capacity() {
        let config = ConfigLock::default();
        config.load_write().unwrap().market.max_concurrent_proofs = 1;

        let (order_tx, order_rx) = mpsc::channel(100);
        let (proving_completion_tx, proving_completion_rx) = mpsc::channel(100);
        let (state_tx, _) = broadcast::channel(100);
        let (locker_tx, mut locker_rx) = mpsc::channel(100);

        let mut dispatchers = HashMap::new();
        dispatchers.insert(1u64, locker_tx);

        let mut priority_requestors_map = HashMap::new();
        priority_requestors_map.insert(1u64, PriorityRequestors::new(config.clone(), 1));
        let committer = OrderCommitter::new(
            config.clone(),
            order_rx,
            dispatchers,
            proving_completion_rx,
            state_tx,
            priority_requestors_map,
        );

        let cancel = CancellationToken::new();
        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { committer.spawn(cancel).await }
        });

        for i in 0..3 {
            order_tx.send(make_order(1, i, FulfillmentType::LockAndFulfill)).await.unwrap();
        }

        let first = recv_with_timeout(&mut locker_rx).await;
        assert_no_message(&mut locker_rx).await;

        config.load_write().unwrap().market.max_concurrent_proofs = 3;

        tokio::time::sleep(Duration::from_secs(6)).await;

        proving_completion_tx
            .send(CommitmentComplete {
                order_id: first.id(),
                chain_id: 1,
                outcome: CommitmentOutcome::Skipped,
            })
            .await
            .unwrap();

        let _ = recv_with_timeout(&mut locker_rx).await;
        let _ = recv_with_timeout(&mut locker_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_stale_capacity_reaper() {
        let mut in_flight: HashMap<String, InFlightOrder> = HashMap::new();

        in_flight.insert(
            "stale_order".to_string(),
            InFlightOrder {
                dispatched_at: Instant::now() - Duration::from_secs(8000),
                total_cycles: Some(1_000_000),
                proving_started_at: Some(now_timestamp().saturating_sub(8000)),
            },
        );
        in_flight.insert(
            "fresh_order".to_string(),
            InFlightOrder {
                dispatched_at: Instant::now(),
                total_cycles: Some(1_000_000),
                proving_started_at: Some(now_timestamp()),
            },
        );

        OrderCommitter::reap_stale_capacity(&mut in_flight, 7200);

        assert!(!in_flight.contains_key("stale_order"));
        assert!(in_flight.contains_key("fresh_order"));
        assert_eq!(in_flight.len(), 1);
    }
}
