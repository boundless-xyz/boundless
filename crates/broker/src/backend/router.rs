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

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use alloy::primitives::FixedBytes;
use anyhow::{Context, Result};
use boundless_market::prover_utils::{
    EvaluationLimits, EvaluationRequest, OrderPricingError, RequestEvaluation,
};

use crate::Order;

use super::types::{
    BackendError, BackendId, BackendObj, BatchClose, BatchSizeEstimate, BatchSizeEstimateRequest,
    BatchUpdate, CloseBatch, FulfillmentBatch, OrderProcessProgress, ProcessOrder, SubmissionPlan,
    UpdateBatch,
};

#[derive(Clone, Default)]
pub struct BackendRouter {
    backends: HashMap<BackendId, BackendEntry>,
    routes: HashMap<FixedBytes<4>, BackendId>,
}

#[derive(Clone)]
pub struct BackendEntry {
    id: BackendId,
    selectors: Vec<FixedBytes<4>>,
    backend: BackendObj,
}

impl BackendEntry {
    pub fn new(backend: BackendObj) -> Self {
        let id = backend.id().clone();
        let selectors = backend.supported_selectors();
        Self { id, selectors, backend }
    }

    pub fn id(&self) -> &BackendId {
        &self.id
    }
}

impl BackendRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_backend(mut self, entry: BackendEntry) -> Result<Self> {
        if entry.selectors.is_empty() {
            anyhow::bail!("backend {} must register at least one selector", entry.id());
        }

        if self.backends.contains_key(entry.id()) {
            anyhow::bail!("backend {} is already registered", entry.id());
        }

        let mut entry_selectors = HashSet::new();
        for selector in &entry.selectors {
            if !entry_selectors.insert(*selector) {
                anyhow::bail!(
                    "selector {selector:?} is listed more than once for backend {}",
                    entry.id()
                );
            }
            if let Some(existing_backend) = self.routes.get(selector) {
                anyhow::bail!(
                    "selector {selector:?} is already registered to backend {existing_backend}"
                );
            }
            self.routes.insert(*selector, entry.id.clone());
        }
        self.backends.insert(entry.id.clone(), entry);
        Ok(self)
    }

    fn backend_for_order(&self, order: &Order) -> Result<BackendObj> {
        let selector = order.request.requirements.selector;
        let backend_id = self
            .routes
            .get(&selector)
            .with_context(|| format!("no backend registered for selector {selector:?}"))?;
        self.backends
            .get(backend_id)
            .map(|entry| entry.backend.clone())
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    fn backend_for_id(&self, backend_id: &BackendId) -> Result<BackendObj> {
        self.backends
            .get(backend_id)
            .map(|entry| entry.backend.clone())
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    pub fn backend_ids(&self) -> Vec<BackendId> {
        let mut ids: Vec<_> = self.backends.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError> {
        let selector = request.selector;
        let backend_id = self.routes.get(&selector).ok_or_else(|| {
            OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                "no backend registered for selector {selector:?}"
            )))
        })?;
        let backend =
            self.backends.get(backend_id).map(|entry| entry.backend.clone()).ok_or_else(|| {
                OrderPricingError::UnexpectedErr(Arc::new(anyhow::anyhow!(
                    "backend {backend_id} is not registered"
                )))
            })?;

        backend.evaluate_request(request, limits).await
    }

    pub async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let backend = self.backend_for_order(&cmd.order)?;
        backend.process_order(cmd).await
    }

    pub async fn cancel_order(&self, order: &Order) -> Result<()> {
        let backend = self.backend_for_order(order)?;
        backend.cancel_order(order).await
    }

    pub async fn estimate_batch_size(
        &self,
        backend_id: &BackendId,
        cmd: BatchSizeEstimateRequest,
    ) -> Result<BatchSizeEstimate> {
        let backend = self.backend_for_id(backend_id)?;
        backend.estimate_batch_size(cmd).await
    }

    pub async fn update_batch(
        &self,
        backend_id: &BackendId,
        cmd: UpdateBatch,
    ) -> Result<BatchUpdate> {
        let backend = self.backend_for_id(backend_id)?;
        backend.update_batch(cmd).await
    }

    pub async fn close_batch(
        &self,
        backend_id: &BackendId,
        cmd: CloseBatch,
    ) -> Result<BatchClose, BackendError> {
        let backend = self.backend_for_id(backend_id).map_err(BackendError::operation)?;
        backend.close_batch(cmd).await
    }

    pub async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.build_fulfillments(cmd).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, Bytes, U256};
    use async_trait::async_trait;
    use boundless_market::contracts::{
        eip712_domain, Fulfillment as MarketFulfillment, FulfillmentDataType, Offer, Predicate,
        ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use chrono::Utc;
    use risc0_zkvm::sha::Digest;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        OnceLock,
    };

    use crate::OrderStatus;

    use super::super::types::{OrderFulfillmentArtifact, OrderFulfillmentResult, ProcessedOrder};

    #[derive(Debug)]
    struct MockBackend {
        id: BackendId,
        supported: Vec<FixedBytes<4>>,
        calls: AtomicUsize,
        cancel_calls: AtomicUsize,
        estimate_calls: AtomicUsize,
        update_calls: AtomicUsize,
        fulfillment_calls: AtomicUsize,
    }

    impl MockBackend {
        fn new(id: &str, supported: Vec<FixedBytes<4>>) -> Self {
            Self {
                id: BackendId::new(id).unwrap(),
                supported,
                calls: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
                estimate_calls: AtomicUsize::new(0),
                update_calls: AtomicUsize::new(0),
                fulfillment_calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn cancel_calls(&self) -> usize {
            self.cancel_calls.load(Ordering::SeqCst)
        }

        fn estimate_calls(&self) -> usize {
            self.estimate_calls.load(Ordering::SeqCst)
        }

        fn update_calls(&self) -> usize {
            self.update_calls.load(Ordering::SeqCst)
        }

        fn fulfillment_calls(&self) -> usize {
            self.fulfillment_calls.load(Ordering::SeqCst)
        }

        fn ensure_backend_id(&self, backend_id: &BackendId) -> Result<()> {
            anyhow::ensure!(backend_id == &self.id, "wrong backend id");
            Ok(())
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        fn id(&self) -> &BackendId {
            &self.id
        }

        fn supported_selectors(&self) -> Vec<FixedBytes<4>> {
            self.supported.clone()
        }

        async fn evaluate_request(
            &self,
            _request: EvaluationRequest,
            _limits: EvaluationLimits,
        ) -> Result<RequestEvaluation, OrderPricingError> {
            anyhow::bail!("mock backend does not evaluate requests")
        }

        async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(OrderProcessProgress::Completed(ProcessedOrder {
                backend_id: self.id.clone(),
                order_id: cmd.order.id(),
                proof_id: "proof".try_into().unwrap(),
                compressed_proof_id: None,
                next_status: OrderStatus::PendingAgg,
            }))
        }

        async fn cancel_order(&self, _order: &Order) -> Result<()> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn estimate_batch_size(
            &self,
            _cmd: BatchSizeEstimateRequest,
        ) -> Result<BatchSizeEstimate> {
            self.estimate_calls.fetch_add(1, Ordering::SeqCst);
            Ok(BatchSizeEstimate { size: 0 })
        }

        async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate> {
            self.update_calls.fetch_add(1, Ordering::SeqCst);
            Ok(BatchUpdate {
                state: super::super::types::BackendBatchState {
                    data: serde_json::json!({
                        "backend_id": self.id.to_string(),
                        "batch_id": cmd.batch_id,
                    }),
                    proof_id: None,
                    compressed_proof_id: None,
                },
                assessor_proof_id: None,
                batch_update_secs: None,
                assessor_secs: None,
            })
        }

        async fn close_batch(&self, _cmd: CloseBatch) -> Result<BatchClose, BackendError> {
            Err(BackendError::operation(anyhow::anyhow!("mock backend does not close batches")))
        }

        async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan> {
            self.ensure_backend_id(&cmd.backend_id)?;
            self.fulfillment_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SubmissionPlan {
                verifier_updates: Vec::new(),
                orders: cmd
                    .orders
                    .into_iter()
                    .map(|order| OrderFulfillmentArtifact {
                        order_id: order.order_id,
                        fulfillment: MarketFulfillment {
                            id: alloy::primitives::U256::ZERO,
                            requestDigest: Default::default(),
                            fulfillmentData: Default::default(),
                            fulfillmentDataType: FulfillmentDataType::None,
                            claimDigest: Default::default(),
                            seal: Default::default(),
                        },
                    })
                    .map(OrderFulfillmentResult::Fulfilled)
                    .collect(),
                assessor: super::super::types::SubmissionAssessorArtifact {
                    seal: Default::default(),
                    selectors: Vec::new(),
                    callbacks: Vec::new(),
                },
            })
        }
    }

    fn selector(byte: u8) -> FixedBytes<4> {
        FixedBytes::from([byte; 4])
    }

    fn test_order(selector: FixedBytes<4>) -> Order {
        let mut request = ProofRequest::new(
            RequestId::new(Address::ZERO, 1),
            Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
            "http://example.com",
            RequestInput { inputType: RequestInputType::Inline, data: Bytes::new() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(2),
                rampUpStart: 0,
                timeout: 300,
                lockTimeout: 300,
                rampUpPeriod: 1,
                lockCollateral: U256::ZERO,
            },
        );
        request.requirements.selector = selector;

        Order {
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            fulfillment_type: crate::FulfillmentType::LockAndFulfill,
            request,
            status: OrderStatus::Proving,
            client_sig: Bytes::new(),
            updated_at: Utc::now(),
            image_id: None,
            input_id: None,
            total_cycles: None,
            journal_bytes: None,
            target_timestamp: None,
            expire_timestamp: None,
            proving_started_at: None,
            proof_id: Some("stark".to_string()),
            compressed_proof_id: None,
            backend_id: None,
            lock_price: None,
            error_msg: None,
            cached_id: OnceLock::new(),
        }
    }

    #[tokio::test]
    async fn router_routes_supported_selector() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router =
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap();

        let progress =
            router.process_order(ProcessOrder { order: test_order(selector(1)) }).await.unwrap();

        assert_eq!(backend.calls(), 1);
        assert_eq!(
            progress,
            OrderProcessProgress::Completed(ProcessedOrder {
                backend_id: BackendId::new("mock_a").unwrap(),
                order_id: test_order(selector(1)).id(),
                proof_id: "proof".try_into().unwrap(),
                compressed_proof_id: None,
                next_status: OrderStatus::PendingAgg,
            })
        );
    }

    #[tokio::test]
    async fn router_keeps_backend_lifecycle_calls_separate() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_b", vec![selector(2)]));
        let backend_a_id = BackendId::new("mock_a").unwrap();
        let backend_b_id = BackendId::new("mock_b").unwrap();
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(backend_a.clone()))
            .unwrap()
            .register_backend(BackendEntry::new(backend_b.clone()))
            .unwrap();

        let progress_a =
            router.process_order(ProcessOrder { order: test_order(selector(1)) }).await.unwrap();
        let progress_b =
            router.process_order(ProcessOrder { order: test_order(selector(2)) }).await.unwrap();

        assert_eq!(backend_a.calls(), 1);
        assert_eq!(backend_b.calls(), 1);
        assert!(matches!(
            progress_a,
            OrderProcessProgress::Completed(ProcessedOrder { backend_id, .. })
                if backend_id == backend_a_id
        ));
        assert!(matches!(
            progress_b,
            OrderProcessProgress::Completed(ProcessedOrder { backend_id, .. })
                if backend_id == backend_b_id
        ));

        router
            .estimate_batch_size(
                &backend_a_id,
                BatchSizeEstimateRequest {
                    state: None,
                    existing_order_ids: Vec::new(),
                    pending_order_ids: vec!["a1".to_string()],
                },
            )
            .await
            .unwrap();
        router
            .estimate_batch_size(
                &backend_b_id,
                BatchSizeEstimateRequest {
                    state: None,
                    existing_order_ids: Vec::new(),
                    pending_order_ids: vec!["b1".to_string()],
                },
            )
            .await
            .unwrap();
        assert_eq!(backend_a.estimate_calls(), 1);
        assert_eq!(backend_b.estimate_calls(), 1);

        let update_a = router
            .update_batch(
                &backend_a_id,
                UpdateBatch {
                    batch_id: 10,
                    existing_order_ids: Vec::new(),
                    state: None,
                    new_orders: Vec::new(),
                    finalize: false,
                },
            )
            .await
            .unwrap();
        let update_b = router
            .update_batch(
                &backend_b_id,
                UpdateBatch {
                    batch_id: 20,
                    existing_order_ids: Vec::new(),
                    state: None,
                    new_orders: Vec::new(),
                    finalize: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(backend_a.update_calls(), 1);
        assert_eq!(backend_b.update_calls(), 1);
        assert_eq!(update_a.state.data["backend_id"], "mock_a");
        assert_eq!(update_b.state.data["backend_id"], "mock_b");

        router
            .build_fulfillments(FulfillmentBatch {
                backend_id: backend_a_id.clone(),
                state: None,
                assessor_proof_id: None,
                eip712_domain: eip712_domain(Address::ZERO, 1),
                orders: Vec::new(),
            })
            .await
            .unwrap();
        router
            .build_fulfillments(FulfillmentBatch {
                backend_id: backend_b_id.clone(),
                state: None,
                assessor_proof_id: None,
                eip712_domain: eip712_domain(Address::ZERO, 1),
                orders: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(backend_a.fulfillment_calls(), 1);
        assert_eq!(backend_b.fulfillment_calls(), 1);
    }

    #[tokio::test]
    async fn router_rejects_unsupported_selector_before_backend_call() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router =
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap();

        let err = router
            .process_order(ProcessOrder { order: test_order(selector(2)) })
            .await
            .unwrap_err();

        assert_eq!(backend.calls(), 0);
        assert!(err.to_string().contains("no backend registered for selector"));
    }

    #[tokio::test]
    async fn router_routes_cancellation() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router =
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap();

        router.cancel_order(&test_order(selector(1))).await.unwrap();

        assert_eq!(backend.cancel_calls(), 1);
    }

    #[test]
    fn router_rejects_duplicate_backend_id() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_a", vec![selector(2)]));

        let err = BackendRouter::new()
            .register_backend(BackendEntry::new(backend_a))
            .unwrap()
            .register_backend(BackendEntry::new(backend_b))
            .err()
            .unwrap();

        assert!(err.to_string().contains("backend mock_a is already registered"));
    }

    #[test]
    fn router_rejects_duplicate_selector() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_b", vec![selector(1)]));

        let err = BackendRouter::new()
            .register_backend(BackendEntry::new(backend_a))
            .unwrap()
            .register_backend(BackendEntry::new(backend_b))
            .err()
            .unwrap();

        assert!(err.to_string().contains("is already registered to backend mock_a"));
    }

    #[test]
    fn router_rejects_duplicate_selector_inside_backend_entry() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1), selector(1)]));

        let err = BackendRouter::new().register_backend(BackendEntry::new(backend)).err().unwrap();

        assert!(err.to_string().contains("is listed more than once for backend mock_a"));
    }

    #[test]
    fn router_rejects_empty_selector_set() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![]));

        let err = BackendRouter::new().register_backend(BackendEntry::new(backend)).err().unwrap();

        assert!(err.to_string().contains("must register at least one selector"));
    }
}
