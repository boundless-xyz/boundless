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
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::U256;
use thiserror::Error;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_util::sync::CancellationToken;

use crate::{
    config::ConfigLock,
    errors::CodedError,
    prioritization::prioritize_orders_to_evaluate,
    requestor_monitor::PriorityRequestors,
    task::{RetryRes, RetryTask, SupervisorErr},
    FulfillmentType, OrderRequest, OrderStateChange,
};

const MIN_CAPACITY_CHECK_INTERVAL: Duration = Duration::from_secs(5);
const DEFAULT_MAX_PREFLIGHT_DURATION_SECS: u64 = 30000;

#[derive(Debug, Clone, Copy)]
pub(crate) enum PreflightOutcome {
    Priced,
    Skipped,
    #[allow(dead_code)]
    Failed,
    Cancelled,
}

impl std::fmt::Display for PreflightOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PreflightOutcome::Priced => write!(f, "Priced"),
            PreflightOutcome::Skipped => write!(f, "Skipped"),
            PreflightOutcome::Failed => write!(f, "Failed"),
            PreflightOutcome::Cancelled => write!(f, "Cancelled"),
        }
    }
}

pub(crate) struct PreflightComplete {
    pub order_id: String,
    #[allow(dead_code)]
    pub request_id: U256,
    pub chain_id: u64,
    pub outcome: PreflightOutcome,
}

#[derive(Error, Debug)]
pub(crate) enum OrderEvaluatorErr {
    #[error("{code} Config read error: {0}", code = self.code())]
    ConfigReadErr(Arc<anyhow::Error>),

    #[error("{code} Stale capacity for order {order_id} expired after {elapsed_secs}s with no completion", code = self.code())]
    StaleCapacity { order_id: String, elapsed_secs: u64 },
}

impl CodedError for OrderEvaluatorErr {
    fn code(&self) -> &str {
        match self {
            OrderEvaluatorErr::ConfigReadErr(_) => "[B-OE-001]",
            OrderEvaluatorErr::StaleCapacity { .. } => "[B-OE-004]",
        }
    }
}

#[derive(Clone)]
pub(crate) struct OrderEvaluator {
    config: ConfigLock,
    new_order_rx: Arc<Mutex<mpsc::Receiver<Box<OrderRequest>>>>,
    chain_dispatchers: Arc<HashMap<u64, mpsc::Sender<Box<OrderRequest>>>>,
    pricing_completion_rx: Arc<Mutex<mpsc::Receiver<PreflightComplete>>>,
    order_state_tx: broadcast::Sender<OrderStateChange>,
    priority_requestors: Arc<HashMap<u64, PriorityRequestors>>,
}

impl OrderEvaluator {
    pub fn new(
        config: ConfigLock,
        new_order_rx: mpsc::Receiver<Box<OrderRequest>>,
        chain_dispatchers: HashMap<u64, mpsc::Sender<Box<OrderRequest>>>,
        pricing_completion_rx: mpsc::Receiver<PreflightComplete>,
        order_state_tx: broadcast::Sender<OrderStateChange>,
        priority_requestors: HashMap<u64, PriorityRequestors>,
    ) -> Self {
        Self {
            config,
            new_order_rx: Arc::new(Mutex::new(new_order_rx)),
            chain_dispatchers: Arc::new(chain_dispatchers),
            pricing_completion_rx: Arc::new(Mutex::new(pricing_completion_rx)),
            order_state_tx,
            priority_requestors: Arc::new(priority_requestors),
        }
    }

    fn read_config(
        &self,
    ) -> Result<
        (
            usize,
            boundless_market::prover_utils::config::OrderPricingPriority,
            Option<Vec<alloy::primitives::Address>>,
        ),
        OrderEvaluatorErr,
    > {
        let cfg = self.config.lock_all().map_err(|err| {
            OrderEvaluatorErr::ConfigReadErr(Arc::new(anyhow::anyhow!(
                "Failed to read config: {err}"
            )))
        })?;
        Ok((
            cfg.market.max_concurrent_preflights as usize,
            cfg.market.order_pricing_priority,
            cfg.market.priority_requestor_addresses.clone(),
        ))
    }

    fn collect_priority_addresses(
        &self,
        static_addresses: Option<Vec<alloy::primitives::Address>>,
    ) -> Vec<alloy::primitives::Address> {
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

    #[allow(clippy::vec_box)]
    fn dispatch_pending(
        &self,
        pending_orders: &mut Vec<Box<OrderRequest>>,
        in_flight: &mut HashMap<String, Instant>,
        max_concurrent_preflights: usize,
        priority_mode: boundless_market::prover_utils::config::OrderPricingPriority,
        priority_addresses: &[alloy::primitives::Address],
    ) {
        if pending_orders.is_empty() || in_flight.len() >= max_concurrent_preflights {
            return;
        }

        let available_capacity = max_concurrent_preflights - in_flight.len();
        let priority_ref =
            if priority_addresses.is_empty() { None } else { Some(priority_addresses) };
        let selected = prioritize_orders_to_evaluate(
            pending_orders,
            priority_mode,
            priority_ref,
            available_capacity,
        );

        for order in selected {
            let order_id = order.id();
            let chain_id = order.chain_id;

            let Some(pricer_tx) = self.chain_dispatchers.get(&chain_id) else {
                tracing::warn!(
                    chain_id,
                    "[B-OE-002] Dropping order {order_id}: no dispatch channel for chain"
                );
                continue;
            };

            match pricer_tx.try_send(order) {
                Ok(()) => {
                    in_flight.insert(order_id.clone(), Instant::now());
                    tracing::debug!(
                        chain_id,
                        "Dispatched order {order_id} for preflight ({} in-flight, {} pending)",
                        in_flight.len(),
                        pending_orders.len(),
                    );
                }
                Err(mpsc::error::TrySendError::Full(order)) => {
                    tracing::warn!(
                        chain_id,
                        "Dispatch channel full for chain {chain_id}, re-queuing order {}",
                        order.id()
                    );
                    pending_orders.push(order);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    tracing::error!(
                        chain_id,
                        "[B-OE-003] Dispatch channel closed for chain {chain_id}, dropping order {order_id}"
                    );
                }
            }
        }
    }

    fn reap_stale_capacity(in_flight: &mut HashMap<String, Instant>, max_age_secs: u64) {
        let threshold = Duration::from_secs(max_age_secs);
        let stale: Vec<String> = in_flight
            .iter()
            .filter(|(_, dispatched_at)| dispatched_at.elapsed() > threshold)
            .map(|(id, _)| id.clone())
            .collect();

        for order_id in stale {
            if let Some(dispatched_at) = in_flight.remove(&order_id) {
                let err = OrderEvaluatorErr::StaleCapacity {
                    order_id: order_id.clone(),
                    elapsed_secs: dispatched_at.elapsed().as_secs(),
                };
                tracing::warn!("{err}");
            }
        }
    }

    fn update_pending_preflight_telemetry(&self, count: u32) {
        for chain_id in self.chain_dispatchers.keys() {
            crate::telemetry::telemetry(*chain_id).set_pending_preflight(count);
        }
    }
}

impl RetryTask for OrderEvaluator {
    type Error = OrderEvaluatorErr;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let evaluator = self.clone();

        Box::pin(async move {
            tracing::info!("Starting order evaluator");

            let (mut max_concurrent_preflights, mut priority_mode, static_addresses) =
                evaluator.read_config().map_err(SupervisorErr::Fault)?;
            let mut priority_addresses = evaluator.collect_priority_addresses(static_addresses);

            let mut order_rx = evaluator.new_order_rx.lock().await;
            let mut pricing_completion_rx = evaluator.pricing_completion_rx.lock().await;
            let mut order_state_rx = evaluator.order_state_tx.subscribe();
            let mut capacity_check_interval = tokio::time::interval(MIN_CAPACITY_CHECK_INTERVAL);

            let mut pending_orders: Vec<Box<OrderRequest>> = Vec::new();
            let mut in_flight: HashMap<String, Instant> = HashMap::new();

            loop {
                tokio::select! {
                    Some(order) = order_rx.recv() => {
                        let order_id = order.id();
                        let chain_id = order.chain_id;
                        pending_orders.push(order);
                        tracing::debug!(
                            chain_id,
                            "Evaluator queued order {order_id} ({} pending [{}], {} in-flight)",
                            pending_orders.len(),
                            Self::format_per_chain_counts(&pending_orders),
                            in_flight.len(),
                        );
                    }

                    Some(completion) = pricing_completion_rx.recv() => {
                        if in_flight.remove(&completion.order_id).is_some() {
                            tracing::debug!(
                                chain_id = completion.chain_id,
                                "Preflight complete for order {} ({}), {} in-flight remaining",
                                completion.order_id,
                                completion.outcome,
                                in_flight.len(),
                            );
                        } else {
                            tracing::trace!(
                                chain_id = completion.chain_id,
                                "Received completion for order {} not in in-flight map (possibly reaped)",
                                completion.order_id,
                            );
                        }
                    }

                    Ok(state_change) = order_state_rx.recv() => {
                        match state_change {
                            OrderStateChange::Locked { request_id, chain_id, .. } => {
                                tracing::debug!(
                                    chain_id,
                                    "Evaluator: request 0x{:x} locked, removing pending LockAndFulfill orders",
                                    request_id,
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
                                        "Removed {removed} LockAndFulfill orders from preflight queue for locked request 0x{:x}",
                                        request_id,
                                    );
                                }
                            }
                            OrderStateChange::Fulfilled { request_id, chain_id } => {
                                tracing::debug!(
                                    chain_id,
                                    "Evaluator: request 0x{:x} fulfilled, removing all pending orders",
                                    request_id,
                                );
                                let initial_len = pending_orders.len();
                                pending_orders.retain(|order| {
                                    !(U256::from(order.request.id) == request_id && order.chain_id == chain_id)
                                });
                                let removed = initial_len - pending_orders.len();
                                if removed > 0 {
                                    tracing::debug!(
                                        "Removed {removed} orders from preflight queue for fulfilled request 0x{:x}",
                                        request_id,
                                    );
                                }
                            }
                        }
                    }

                    _ = capacity_check_interval.tick() => {
                        let (new_capacity, new_priority, new_static_addresses) =
                            evaluator.read_config().map_err(SupervisorErr::Fault)?;

                        if new_capacity != max_concurrent_preflights {
                            tracing::debug!(
                                "Evaluator preflight capacity changed from {} to {}",
                                max_concurrent_preflights, new_capacity,
                            );
                            max_concurrent_preflights = new_capacity;
                        }
                        if new_priority != priority_mode {
                            tracing::debug!(
                                "Evaluator pricing priority changed from {:?} to {:?}",
                                priority_mode, new_priority,
                            );
                            priority_mode = new_priority;
                        }
                        priority_addresses =
                            evaluator.collect_priority_addresses(new_static_addresses);

                        Self::reap_stale_capacity(
                            &mut in_flight,
                            DEFAULT_MAX_PREFLIGHT_DURATION_SECS,
                        );
                    }

                    _ = cancel_token.cancelled() => {
                        tracing::info!("Order evaluator received cancellation, shutting down");
                        break;
                    }
                }

                evaluator.update_pending_preflight_telemetry(pending_orders.len() as u32);

                evaluator.dispatch_pending(
                    &mut pending_orders,
                    &mut in_flight,
                    max_concurrent_preflights,
                    priority_mode,
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
        Box::new(OrderRequest::new(
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
        ))
    }

    #[allow(clippy::type_complexity)]
    fn setup_evaluator(
        max_concurrent_preflights: u32,
        chain_ids: &[u64],
    ) -> (
        OrderEvaluator,
        mpsc::Sender<Box<OrderRequest>>,
        mpsc::Sender<PreflightComplete>,
        broadcast::Sender<OrderStateChange>,
        HashMap<u64, mpsc::Receiver<Box<OrderRequest>>>,
    ) {
        let config = ConfigLock::default();
        config.load_write().unwrap().market.max_concurrent_preflights = max_concurrent_preflights;

        let (order_tx, order_rx) = mpsc::channel(100);
        let (pricing_completion_tx, pricing_completion_rx) = mpsc::channel(100);
        let (state_tx, _) = broadcast::channel(100);

        let mut dispatchers = HashMap::new();
        let mut pricer_rxs = HashMap::new();
        for &chain_id in chain_ids {
            let (tx, rx) = mpsc::channel(100);
            dispatchers.insert(chain_id, tx);
            pricer_rxs.insert(chain_id, rx);
        }

        let mut priority_requestors_map = HashMap::new();
        for &chain_id in chain_ids {
            priority_requestors_map
                .insert(chain_id, PriorityRequestors::new(config.clone(), chain_id));
        }

        let evaluator = OrderEvaluator::new(
            config,
            order_rx,
            dispatchers,
            pricing_completion_rx,
            state_tx.clone(),
            priority_requestors_map,
        );

        (evaluator, order_tx, pricing_completion_tx, state_tx, pricer_rxs)
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
        let (evaluator, order_tx, _pricing_completion_tx, _state_tx, mut pricer_rxs) =
            setup_evaluator(2, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        let pricer_rx = pricer_rxs.get_mut(&1).unwrap();

        // Send 4 orders, capacity is 2
        for i in 0..4 {
            order_tx.send(make_order(1, i, FulfillmentType::LockAndFulfill)).await.unwrap();
        }

        // Should receive exactly 2 dispatched orders
        let _ = recv_with_timeout(pricer_rx).await;
        let _ = recv_with_timeout(pricer_rx).await;

        // Third should not arrive (capacity full)
        assert_no_message(pricer_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_completion_frees_capacity() {
        let (evaluator, order_tx, pricing_completion_tx, _state_tx, mut pricer_rxs) =
            setup_evaluator(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        let pricer_rx = pricer_rxs.get_mut(&1).unwrap();

        // Send 2 orders, capacity is 1
        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();

        // First dispatched
        let first = recv_with_timeout(pricer_rx).await;
        assert_no_message(pricer_rx).await;

        // Send completion for the first
        pricing_completion_tx
            .send(PreflightComplete {
                order_id: first.id(),
                request_id: first.request.id,
                chain_id: 1,
                outcome: PreflightOutcome::Priced,
            })
            .await
            .unwrap();

        // Second should now be dispatched
        let _ = recv_with_timeout(pricer_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_lock_event_removes_pending() {
        let (evaluator, order_tx, pricing_completion_tx, state_tx, mut pricer_rxs) =
            setup_evaluator(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        let pricer_rx = pricer_rxs.get_mut(&1).unwrap();

        // Fill capacity with order 0
        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        let first = recv_with_timeout(pricer_rx).await;

        // Queue a LockAndFulfill order for request 1
        let queued_order = make_order(1, 1, FulfillmentType::LockAndFulfill);
        let queued_request_id: U256 = queued_order.request.id;
        order_tx.send(queued_order).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Also queue a FulfillAfterLockExpire order for the same request
        let mut fale_order = *make_order(1, 99, FulfillmentType::FulfillAfterLockExpire);
        fale_order.request.id = RequestId::new(Address::ZERO, 1).into();
        order_tx.send(Box::new(fale_order)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Lock request 1 -- should remove LockAndFulfill but keep FulfillAfterLockExpire
        state_tx
            .send(OrderStateChange::Locked {
                request_id: queued_request_id,
                prover: Address::ZERO,
                chain_id: 1,
            })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Complete the in-flight order 0 to free capacity
        pricing_completion_tx
            .send(PreflightComplete {
                order_id: first.id(),
                request_id: first.request.id,
                chain_id: 1,
                outcome: PreflightOutcome::Priced,
            })
            .await
            .unwrap();

        // The FulfillAfterLockExpire order should be dispatched (not removed by lock)
        let dispatched = recv_with_timeout(pricer_rx).await;
        assert_eq!(dispatched.fulfillment_type, FulfillmentType::FulfillAfterLockExpire);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_fulfill_event_removes_pending() {
        let (evaluator, order_tx, pricing_completion_tx, state_tx, mut pricer_rxs) =
            setup_evaluator(1, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        let pricer_rx = pricer_rxs.get_mut(&1).unwrap();

        // Fill capacity
        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        let first = recv_with_timeout(pricer_rx).await;

        // Queue orders for request_id 1
        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();
        let mut fale_order = *make_order(1, 99, FulfillmentType::FulfillAfterLockExpire);
        fale_order.request.id = RequestId::new(Address::ZERO, 1).into();
        order_tx.send(Box::new(fale_order)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Fulfill request 1 -- should remove ALL orders for that request
        let fulfilled_request_id: U256 = RequestId::new(Address::ZERO, 1).into();
        state_tx
            .send(OrderStateChange::Fulfilled { request_id: fulfilled_request_id, chain_id: 1 })
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Free capacity
        pricing_completion_tx
            .send(PreflightComplete {
                order_id: first.id(),
                request_id: first.request.id,
                chain_id: 1,
                outcome: PreflightOutcome::Priced,
            })
            .await
            .unwrap();

        // Nothing should be dispatched since all pending orders for request 1 were removed
        assert_no_message(pricer_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_routes_to_correct_chain() {
        let (evaluator, order_tx, _pricing_completion_tx, _state_tx, mut pricer_rxs) =
            setup_evaluator(10, &[1, 8453]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        order_tx.send(make_order(1, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(8453, 1, FulfillmentType::LockAndFulfill)).await.unwrap();
        order_tx.send(make_order(1, 2, FulfillmentType::LockAndFulfill)).await.unwrap();

        // Check chain 1 dispatches
        {
            let chain_1_rx = pricer_rxs.get_mut(&1).unwrap();
            let dispatched_1a = recv_with_timeout(chain_1_rx).await;
            assert_eq!(dispatched_1a.chain_id, 1);
        }

        // Check chain 8453 dispatch
        {
            let chain_8453_rx = pricer_rxs.get_mut(&8453).unwrap();
            let dispatched_8453 = recv_with_timeout(chain_8453_rx).await;
            assert_eq!(dispatched_8453.chain_id, 8453);
        }

        // Check second chain 1 dispatch
        {
            let chain_1_rx = pricer_rxs.get_mut(&1).unwrap();
            let dispatched_1b = recv_with_timeout(chain_1_rx).await;
            assert_eq!(dispatched_1b.chain_id, 1);
        }

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_unknown_chain_drops_order() {
        let (evaluator, order_tx, _pricing_completion_tx, _state_tx, mut pricer_rxs) =
            setup_evaluator(10, &[1]);
        let cancel = CancellationToken::new();

        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        let chain_1_rx = pricer_rxs.get_mut(&1).unwrap();

        // Send order for unknown chain 9999
        order_tx.send(make_order(9999, 0, FulfillmentType::LockAndFulfill)).await.unwrap();
        // Send order for known chain 1
        order_tx.send(make_order(1, 1, FulfillmentType::LockAndFulfill)).await.unwrap();

        // Only the chain 1 order should be dispatched
        let dispatched = recv_with_timeout(chain_1_rx).await;
        assert_eq!(dispatched.chain_id, 1);

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_config_reload_changes_capacity() {
        let config = ConfigLock::default();
        config.load_write().unwrap().market.max_concurrent_preflights = 1;

        let (order_tx, order_rx) = mpsc::channel(100);
        let (pricing_completion_tx, pricing_completion_rx) = mpsc::channel(100);
        let (state_tx, _) = broadcast::channel(100);
        let (pricer_tx, mut pricer_rx) = mpsc::channel(100);

        let mut dispatchers = HashMap::new();
        dispatchers.insert(1u64, pricer_tx);

        let mut priority_requestors_map = HashMap::new();
        priority_requestors_map.insert(1u64, PriorityRequestors::new(config.clone(), 1));
        let evaluator = OrderEvaluator::new(
            config.clone(),
            order_rx,
            dispatchers,
            pricing_completion_rx,
            state_tx,
            priority_requestors_map,
        );

        let cancel = CancellationToken::new();
        let handle = tokio::spawn({
            let cancel = cancel.clone();
            async move { evaluator.spawn(cancel).await }
        });

        // Send 3 orders, capacity = 1
        for i in 0..3 {
            order_tx.send(make_order(1, i, FulfillmentType::LockAndFulfill)).await.unwrap();
        }

        let first = recv_with_timeout(&mut pricer_rx).await;
        assert_no_message(&mut pricer_rx).await;

        // Increase capacity to 3 before completing first
        config.load_write().unwrap().market.max_concurrent_preflights = 3;

        // Wait for capacity check interval to pick up the new capacity
        tokio::time::sleep(Duration::from_secs(6)).await;

        // Complete first to trigger re-dispatch with new capacity (3)
        pricing_completion_tx
            .send(PreflightComplete {
                order_id: first.id(),
                request_id: first.request.id,
                chain_id: 1,
                outcome: PreflightOutcome::Skipped,
            })
            .await
            .unwrap();

        // Both remaining orders should now be dispatched (capacity=3, in_flight=0)
        let _ = recv_with_timeout(&mut pricer_rx).await;
        let _ = recv_with_timeout(&mut pricer_rx).await;

        cancel.cancel();
        let _ = handle.await;
    }

    #[tokio::test]
    async fn test_stale_capacity_reaper() {
        let mut in_flight: HashMap<String, Instant> = HashMap::new();

        in_flight.insert("stale_order".to_string(), Instant::now() - Duration::from_secs(700));
        in_flight.insert("fresh_order".to_string(), Instant::now());

        OrderEvaluator::reap_stale_capacity(&mut in_flight, 600);

        assert!(!in_flight.contains_key("stale_order"));
        assert!(in_flight.contains_key("fresh_order"));
        assert_eq!(in_flight.len(), 1);
    }
}
