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
use boundless_market::selector::SupportedSelectors;

use super::types::{
    BackendEntry, BackendError, BackendId, BackendObj, BatchClose, BatchProcessorObj,
    BatchSizeEstimate, BatchSizeEstimateRequest, BatchUpdate, CancelOrder, CloseBatch,
    FulfillmentBatch, OrderProcessProgress, ProcessOrder, SubmissionPlan, UpdateBatch,
    VerifierUpdate, VerifierUpdateError,
};

#[derive(Clone, Default)]
pub struct BackendRouter {
    backends: HashMap<BackendId, BackendEntry>,
    // TODO(#1982): mirror on-chain verifier router dispatch (default + classes + entries).
    routes: HashMap<FixedBytes<4>, BackendId>,
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

        // Validate all selectors before mutating.
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
            entry.backend.proof_type(*selector).with_context(|| {
                format!("backend {} did not classify selector {selector:?}", entry.id())
            })?;
        }

        for selector in &entry.selectors {
            self.routes.insert(*selector, entry.id.clone());
        }
        self.backends.insert(entry.id.clone(), entry);
        Ok(self)
    }

    pub fn supported_selectors(&self) -> SupportedSelectors {
        let mut supported = SupportedSelectors::new();
        for (selector, backend_id) in &self.routes {
            if let Some(entry) = self.backends.get(backend_id) {
                if let Some(proof_type) = entry.backend.proof_type(*selector) {
                    supported.add_selector(*selector, proof_type);
                }
            }
        }
        supported
    }

    fn backend_for_selector(&self, selector: FixedBytes<4>) -> Result<BackendObj> {
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

    /// The assessor grouping key a backend assigns to an order with this signed verifier selector.
    /// Orders under the same backend that share this key may co-batch (see [`Backend::assessor_group`]).
    ///
    /// Resolved through the backend because the key is backend policy, not an on-chain fact: it
    /// depends on the backend's router snapshot and its assessor candidates/priority, and it must
    /// agree with the assessor the same backend later seals the batch with in `build_fulfillments`.
    pub fn assessor_group(
        &self,
        backend_id: &BackendId,
        selector: FixedBytes<4>,
    ) -> Result<Option<FixedBytes<4>>> {
        self.backend_for_id(backend_id)?.assessor_group(selector)
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
        let backend = self.backend_for_selector(cmd.request.requirements.selector)?;
        backend.process_order(cmd).await
    }

    pub async fn cancel_order(&self, cmd: CancelOrder) -> Result<()> {
        let backend = self.backend_for_selector(cmd.selector)?;
        backend.cancel_order(cmd).await
    }

    fn batch_processor_for_id(&self, backend_id: &BackendId) -> Result<BatchProcessorObj> {
        let backend = self.backend_for_id(backend_id)?;
        backend
            .batch_processor()
            .with_context(|| format!("backend {backend_id} does not support batching"))
    }

    pub async fn estimate_batch_size(
        &self,
        backend_id: &BackendId,
        cmd: BatchSizeEstimateRequest,
    ) -> Result<BatchSizeEstimate> {
        self.batch_processor_for_id(backend_id)?.estimate_batch_size(cmd).await
    }

    pub async fn update_batch(
        &self,
        backend_id: &BackendId,
        cmd: UpdateBatch,
    ) -> Result<BatchUpdate> {
        self.batch_processor_for_id(backend_id)?.update_batch(cmd).await
    }

    pub async fn close_batch(
        &self,
        backend_id: &BackendId,
        cmd: CloseBatch,
    ) -> Result<BatchClose, BackendError> {
        self.batch_processor_for_id(backend_id)
            .map_err(BackendError::operation)?
            .close_batch(cmd)
            .await
    }

    pub async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.build_fulfillments(cmd).await
    }

    pub async fn verifier_update_applied(
        &self,
        backend_id: &BackendId,
        update: &VerifierUpdate,
    ) -> Result<bool> {
        let backend = self.backend_for_id(backend_id)?;
        backend.verifier_update_applied(update).await
    }

    pub async fn apply_verifier_update(
        &self,
        backend_id: &BackendId,
        update: &VerifierUpdate,
    ) -> Result<(), VerifierUpdateError> {
        let backend = self.backend_for_id(backend_id).map_err(VerifierUpdateError::Other)?;
        backend.apply_verifier_update(update).await
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
    use boundless_market::selector::ProofType;
    use risc0_zkvm::sha::Digest;
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use super::super::types::{
        Backend, BackendBatchState, BatchProcessor, BatchProcessorObj, CancelOrder,
        OrderFulfillmentArtifact, ProcessedOrder, SubmissionPath,
    };

    #[derive(Debug)]
    struct MockBatchProcessor {
        id: BackendId,
        estimate_calls: AtomicUsize,
        update_calls: AtomicUsize,
    }

    impl MockBatchProcessor {
        fn new(id: BackendId) -> Self {
            Self { id, estimate_calls: AtomicUsize::new(0), update_calls: AtomicUsize::new(0) }
        }

        fn estimate_calls(&self) -> usize {
            self.estimate_calls.load(Ordering::SeqCst)
        }

        fn update_calls(&self) -> usize {
            self.update_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl BatchProcessor for MockBatchProcessor {
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
                state: BackendBatchState(serde_json::json!({
                    "backend_id": self.id.to_string(),
                    "batch_id": cmd.batch_id,
                })),
                finalize: cmd.finalize,
                batch_update_secs: None,
                assessor_secs: None,
            })
        }

        async fn close_batch(&self, _cmd: CloseBatch) -> Result<BatchClose, BackendError> {
            Err(BackendError::operation(anyhow::anyhow!(
                "mock batch processor does not close batches"
            )))
        }
    }

    #[derive(Debug)]
    struct MockBackend {
        id: BackendId,
        supported: Vec<FixedBytes<4>>,
        proof_types: HashMap<FixedBytes<4>, ProofType>,
        calls: AtomicUsize,
        cancel_calls: AtomicUsize,
        fulfillment_calls: AtomicUsize,
        batch_processor: Option<Arc<MockBatchProcessor>>,
    }

    impl MockBackend {
        fn new(id: &str, supported: Vec<FixedBytes<4>>) -> Self {
            let id = BackendId::new(id);
            Self {
                proof_types: supported
                    .iter()
                    .copied()
                    .map(|selector| (selector, ProofType::Any))
                    .collect(),
                supported,
                calls: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
                fulfillment_calls: AtomicUsize::new(0),
                batch_processor: Some(Arc::new(MockBatchProcessor::new(id.clone()))),
                id,
            }
        }

        fn with_proof_types(id: &str, proof_types: Vec<(FixedBytes<4>, ProofType)>) -> Self {
            let id = BackendId::new(id);
            Self {
                supported: proof_types.iter().map(|(selector, _)| *selector).collect(),
                proof_types: proof_types.into_iter().collect(),
                calls: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
                fulfillment_calls: AtomicUsize::new(0),
                batch_processor: Some(Arc::new(MockBatchProcessor::new(id.clone()))),
                id,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn cancel_calls(&self) -> usize {
            self.cancel_calls.load(Ordering::SeqCst)
        }

        fn estimate_calls(&self) -> usize {
            self.batch_processor.as_ref().map(|p| p.estimate_calls()).unwrap_or(0)
        }

        fn update_calls(&self) -> usize {
            self.batch_processor.as_ref().map(|p| p.update_calls()).unwrap_or(0)
        }

        fn without_batch_processor(mut self) -> Self {
            self.batch_processor = None;
            self
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

        fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType> {
            self.proof_types.get(&selector).copied()
        }

        fn assessor_group(&self, _selector: FixedBytes<4>) -> Result<Option<FixedBytes<4>>> {
            Ok(None)
        }

        async fn evaluate_request(
            &self,
            _request: EvaluationRequest,
            _limits: EvaluationLimits,
        ) -> Result<RequestEvaluation, OrderPricingError> {
            Err(anyhow::anyhow!("mock backend does not evaluate requests").into())
        }

        async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(OrderProcessProgress::Completed(ProcessedOrder {
                backend_id: self.id.clone(),
                order_id: cmd.order_id,
                submission_path: SubmissionPath::Batched,
                compressed: false,
            }))
        }

        async fn cancel_order(&self, _cmd: CancelOrder) -> Result<()> {
            self.cancel_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn batch_processor(&self) -> Option<BatchProcessorObj> {
            self.batch_processor.clone().map(|p| -> BatchProcessorObj { p })
        }

        async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan> {
            self.ensure_backend_id(&cmd.backend_id)?;
            self.fulfillment_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SubmissionPlan {
                verifier_updates: Vec::new(),
                failed_orders: Vec::new(),
                orders: cmd
                    .orders
                    .into_iter()
                    .map(|order| OrderFulfillmentArtifact {
                        order_id: order.order_id,
                        request: order.request,
                        fulfillment: MarketFulfillment {
                            fulfillmentData: Default::default(),
                            fulfillmentDataType: FulfillmentDataType::None,
                            claimDigest: Default::default(),
                            seal: Default::default(),
                        },
                    })
                    .collect(),
                assessor: super::super::types::SubmissionAssessorArtifact {
                    seal: Default::default(),
                    selectors: Vec::new(),
                    callbacks: Vec::new(),
                },
            })
        }

        async fn verifier_update_applied(&self, _update: &VerifierUpdate) -> Result<bool> {
            Ok(false)
        }

        async fn apply_verifier_update(
            &self,
            _update: &VerifierUpdate,
        ) -> Result<(), VerifierUpdateError> {
            Ok(())
        }
    }

    fn selector(byte: u8) -> FixedBytes<4> {
        FixedBytes::from([byte; 4])
    }

    const TEST_ORDER_ID: &str = "test-order";

    fn test_request(selector: FixedBytes<4>) -> ProofRequest {
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
        request
    }

    fn process_cmd(selector: FixedBytes<4>) -> ProcessOrder {
        ProcessOrder {
            order_id: TEST_ORDER_ID.to_string(),
            request: test_request(selector),
            image_id: None,
            input_id: None,
            backend_state: None,
        }
    }

    fn cancel_cmd(selector: FixedBytes<4>) -> CancelOrder {
        CancelOrder {
            order_id: TEST_ORDER_ID.to_string(),
            selector,
            backend_state: None,
            is_proving: true,
        }
    }

    #[tokio::test]
    async fn router_routes_supported_selector() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router =
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap();

        let progress = router.process_order(process_cmd(selector(1))).await.unwrap();

        assert_eq!(backend.calls(), 1);
        assert_eq!(
            progress,
            OrderProcessProgress::Completed(ProcessedOrder {
                backend_id: BackendId::new("mock_a"),
                order_id: TEST_ORDER_ID.to_string(),
                submission_path: SubmissionPath::Batched,
                compressed: false,
            })
        );
    }

    #[tokio::test]
    async fn router_keeps_backend_lifecycle_calls_separate() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_b", vec![selector(2)]));
        let backend_a_id = BackendId::new("mock_a");
        let backend_b_id = BackendId::new("mock_b");
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(backend_a.clone()))
            .unwrap()
            .register_backend(BackendEntry::new(backend_b.clone()))
            .unwrap();

        let progress_a = router.process_order(process_cmd(selector(1))).await.unwrap();
        let progress_b = router.process_order(process_cmd(selector(2))).await.unwrap();

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
                    existing_orders: Vec::new(),
                    pending_orders: Vec::new(),
                },
            )
            .await
            .unwrap();
        router
            .estimate_batch_size(
                &backend_b_id,
                BatchSizeEstimateRequest {
                    state: None,
                    existing_orders: Vec::new(),
                    pending_orders: Vec::new(),
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
                    existing_orders: Vec::new(),
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
                    existing_orders: Vec::new(),
                    state: None,
                    new_orders: Vec::new(),
                    finalize: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(backend_a.update_calls(), 1);
        assert_eq!(backend_b.update_calls(), 1);
        assert_eq!(update_a.state.0["backend_id"], "mock_a");
        assert_eq!(update_b.state.0["backend_id"], "mock_b");

        router
            .build_fulfillments(FulfillmentBatch {
                backend_id: backend_a_id.clone(),
                state: None,
                eip712_domain: eip712_domain(Address::ZERO, 1),
                orders: Vec::new(),
            })
            .await
            .unwrap();
        router
            .build_fulfillments(FulfillmentBatch {
                backend_id: backend_b_id.clone(),
                state: None,
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

        let err = router.process_order(process_cmd(selector(2))).await.unwrap_err();

        assert_eq!(backend.calls(), 0);
        assert!(err.to_string().contains("no backend registered for selector"));
    }

    #[tokio::test]
    async fn router_routes_cancellation() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router =
            BackendRouter::new().register_backend(BackendEntry::new(backend.clone())).unwrap();

        router.cancel_order(cancel_cmd(selector(1))).await.unwrap();

        assert_eq!(backend.cancel_calls(), 1);
    }

    #[tokio::test]
    async fn router_reports_missing_batch_processor() {
        let backend =
            Arc::new(MockBackend::new("mock_a", vec![selector(1)]).without_batch_processor());
        let backend_id = BackendId::new("mock_a");
        let router = BackendRouter::new().register_backend(BackendEntry::new(backend)).unwrap();

        let err = router
            .estimate_batch_size(
                &backend_id,
                BatchSizeEstimateRequest {
                    state: None,
                    existing_orders: Vec::new(),
                    pending_orders: Vec::new(),
                },
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("does not support batching"));
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
    fn router_register_backend_is_transactional_on_partial_conflict() {
        let backend_x = Arc::new(MockBackend::new("mock_x", vec![selector(2)]));
        let router = BackendRouter::new().register_backend(BackendEntry::new(backend_x)).unwrap();

        let backend_y =
            Arc::new(MockBackend::new("mock_y", vec![selector(1), selector(2), selector(3)]));
        let err = router.clone().register_backend(BackendEntry::new(backend_y)).err().unwrap();
        assert!(err.to_string().contains("selector"));

        let supported = router.supported_selectors();
        assert_eq!(supported.proof_type(selector(1)), None);
        assert_eq!(supported.proof_type(selector(2)), Some(ProofType::Any));
        assert_eq!(supported.proof_type(selector(3)), None);
        assert_eq!(router.backend_ids(), vec![BackendId::new("mock_x")]);
    }

    #[test]
    fn router_rejects_empty_selector_set() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![]));

        let err = BackendRouter::new().register_backend(BackendEntry::new(backend)).err().unwrap();

        assert!(err.to_string().contains("must register at least one selector"));
    }

    #[test]
    fn router_derives_supported_selector_metadata_from_backends() {
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(Arc::new(MockBackend::with_proof_types(
                "mock_a",
                vec![(selector(1), ProofType::Any), (selector(2), ProofType::Groth16)],
            ))))
            .unwrap()
            .register_backend(BackendEntry::new(Arc::new(MockBackend::with_proof_types(
                "mock_b",
                vec![(selector(3), ProofType::Blake3Groth16)],
            ))))
            .unwrap();

        let supported = router.supported_selectors();

        assert_eq!(supported.proof_type(selector(1)), Some(ProofType::Any));
        assert_eq!(supported.proof_type(selector(2)), Some(ProofType::Groth16));
        assert_eq!(supported.proof_type(selector(3)), Some(ProofType::Blake3Groth16));
        assert_eq!(supported.proof_type(selector(4)), None);
    }
}
