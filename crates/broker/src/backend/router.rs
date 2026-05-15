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

use std::{collections::HashMap, sync::Arc};

use alloy::primitives::FixedBytes;
use anyhow::{Context, Result};

use crate::{provers, Order};

use super::types::{
    Backend, BackendId, BatchClose, BatchSizeEstimate, BatchUpdate, CloseBatch,
    FulfillmentArtifacts, FulfillmentBatch, OrderProcessProgress, ProcessOrder, UpdateBatch,
};

type BackendObj = Arc<dyn Backend>;

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
    pub fn new(selectors: impl IntoIterator<Item = FixedBytes<4>>, backend: BackendObj) -> Self {
        let id = backend.id().clone();
        Self { id, selectors: selectors.into_iter().collect(), backend }
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

        for selector in &entry.selectors {
            if !entry.backend.supports(*selector) {
                anyhow::bail!("backend {} does not support selector {selector:?}", entry.id());
            }
            if let Some(existing_backend) = self.routes.get(selector) {
                anyhow::bail!(
                    "selector {selector:?} is already registered to backend {existing_backend}"
                );
            }
        }

        for selector in &entry.selectors {
            self.routes.insert(*selector, entry.id.clone());
        }
        self.backends.insert(entry.id.clone(), entry);
        Ok(self)
    }

    fn backend_for_order(&self, order: &Order) -> Result<&BackendObj> {
        let selector = order.request.requirements.selector;
        let backend_id = self
            .routes
            .get(&selector)
            .with_context(|| format!("no backend registered for selector {selector:?}"))?;
        self.backends
            .get(backend_id)
            .map(|entry| &entry.backend)
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    fn backend_for_id(&self, backend_id: &BackendId) -> Result<&BackendObj> {
        self.backends
            .get(backend_id)
            .map(|entry| &entry.backend)
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    pub fn backend_ids(&self) -> Vec<BackendId> {
        self.backends.keys().cloned().collect()
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
        order_ids: &[String],
    ) -> Result<BatchSizeEstimate> {
        let backend = self.backend_for_id(backend_id)?;
        backend.estimate_batch_size(order_ids).await
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
    ) -> Result<BatchClose, provers::ProverError> {
        let backend = self
            .backend_for_id(backend_id)
            .map_err(|err| provers::ProverError::ProverInternalError(err.to_string()))?;
        backend.close_batch(cmd).await
    }

    pub async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<FulfillmentArtifacts> {
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
        Fulfillment as MarketFulfillment, FulfillmentDataType, Offer, Predicate, ProofRequest,
        RequestId, RequestInput, RequestInputType, Requirements,
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
    }

    impl MockBackend {
        fn new(id: &str, supported: Vec<FixedBytes<4>>) -> Self {
            Self {
                id: BackendId::new(id).unwrap(),
                supported,
                calls: AtomicUsize::new(0),
                cancel_calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }

        fn cancel_calls(&self) -> usize {
            self.cancel_calls.load(Ordering::SeqCst)
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

        fn supports(&self, selector: FixedBytes<4>) -> bool {
            self.supported.contains(&selector)
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

        async fn estimate_batch_size(&self, _order_ids: &[String]) -> Result<BatchSizeEstimate> {
            Ok(BatchSizeEstimate { size: 0 })
        }

        async fn update_batch(&self, _cmd: UpdateBatch) -> Result<BatchUpdate> {
            anyhow::bail!("mock backend does not update batches")
        }

        async fn close_batch(&self, _cmd: CloseBatch) -> Result<BatchClose, provers::ProverError> {
            Err(provers::ProverError::ProverInternalError(
                "mock backend does not close batches".to_string(),
            ))
        }

        async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<FulfillmentArtifacts> {
            self.ensure_backend_id(&cmd.backend_id)?;
            Ok(FulfillmentArtifacts {
                root_submission: None,
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
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(vec![selector(1)], backend.clone()))
            .unwrap();

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
    async fn router_rejects_unsupported_selector_before_backend_call() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(vec![selector(1)], backend.clone()))
            .unwrap();

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
        let router = BackendRouter::new()
            .register_backend(BackendEntry::new(vec![selector(1)], backend.clone()))
            .unwrap();

        router.cancel_order(&test_order(selector(1))).await.unwrap();

        assert_eq!(backend.cancel_calls(), 1);
    }

    #[test]
    fn router_rejects_duplicate_backend_id() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_a", vec![selector(2)]));

        let err = BackendRouter::new()
            .register_backend(BackendEntry::new(vec![selector(1)], backend_a))
            .unwrap()
            .register_backend(BackendEntry::new(vec![selector(2)], backend_b))
            .err()
            .unwrap();

        assert!(err.to_string().contains("backend mock_a is already registered"));
    }

    #[test]
    fn router_rejects_duplicate_selector() {
        let backend_a = Arc::new(MockBackend::new("mock_a", vec![selector(1)]));
        let backend_b = Arc::new(MockBackend::new("mock_b", vec![selector(1)]));

        let err = BackendRouter::new()
            .register_backend(BackendEntry::new(vec![selector(1)], backend_a))
            .unwrap()
            .register_backend(BackendEntry::new(vec![selector(1)], backend_b))
            .err()
            .unwrap();

        assert!(err.to_string().contains("is already registered to backend mock_a"));
    }

    #[test]
    fn router_rejects_empty_selector_set() {
        let backend = Arc::new(MockBackend::new("mock_a", vec![]));

        let err = BackendRouter::new()
            .register_backend(BackendEntry::new(Vec::<FixedBytes<4>>::new(), backend))
            .err()
            .unwrap();

        assert!(err.to_string().contains("must register at least one selector"));
    }
}
