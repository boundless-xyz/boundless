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

//! Broker-facing backend boundary.
//!
//! This module starts as a thin adapter over the existing RISC Zero broker code.
//! The broker keeps DB state, retry policy, cancellation, and batch lifecycle
//! orchestration; the backend owns zkVM-specific proof processing.

use std::{collections::HashMap, fmt, str::FromStr, sync::Arc};

use alloy::primitives::{Address, Bytes, FixedBytes, B256};
use alloy::sol_types::{SolStruct, SolValue};
use async_trait::async_trait;
use blake3_groth16::Blake3Groth16Receipt;
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{
    contracts::{
        eip712_domain, encode_seal, AssessorCallback, AssessorJournal, AssessorReceipt,
        AssessorSelector, EIP712DomainSaltless, Fulfillment as MarketFulfillment, FulfillmentData,
        FulfillmentDataImageIdAndJournal, FulfillmentDataType, PredicateType, ProofRequest,
        UNSPECIFIED_SELECTOR,
    },
    input::GuestEnv,
    selector::{is_blake3_groth16_selector, is_groth16_selector},
};
use hex::FromHex;
use risc0_aggregation::{GuestState, SetInclusionReceipt, SetInclusionReceiptVerifierParameters};
use risc0_zkvm::{
    sha::{Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::{AggregationOrder, DbObj},
    futures_retry::retry_with_context,
    is_dev_mode,
    provers::{self, ProverObj},
    requestor_monitor::PriorityRequestors,
    storage::{upload_image_uri, upload_input_uri},
    utils::prune_receipt_claim_journal,
    AggregationState, CompressionType, ConfigurableDownloader, Order, OrderStatus,
};
use anyhow::{Context, Result};

/// Stable broker-side identity for a backend registration.
///
/// This is intentionally distinct from a verifier selector: many selectors may
/// route to the same backend, and broker policy/batching is keyed by backend.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BackendId(String);

impl BackendId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.trim().is_empty() {
            anyhow::bail!("backend id cannot be empty");
        }
        Ok(Self(value))
    }
}

impl serde::Serialize for BackendId {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for BackendId {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use chrono::Utc;
    use risc0_zkvm::sha::Digest;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            OnceLock,
        },
        vec,
    };

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
                proof_id: "proof".to_string(),
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
                assessor_receipt: AssessorReceipt {
                    seal: Default::default(),
                    selectors: Vec::new(),
                    prover: cmd.prover_address,
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
                proof_id: "proof".to_string(),
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

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BackendId {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for BackendId {
    type Error = anyhow::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<&str> for BackendId {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

/// Command for processing one accepted broker order.
#[derive(Clone, Debug)]
pub struct ProcessOrder {
    pub order: Order,
}

/// Durable progress returned while an order is being processed.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderProcessProgress {
    Started { proof_id: String },
    Compressed { proof_id: String, compressed_proof_id: String },
    Completed(ProcessedOrder),
}

/// Completed backend processing for one broker order.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedOrder {
    pub backend_id: BackendId,
    pub order_id: String,
    pub proof_id: String,
    pub compressed_proof_id: Option<String>,
    pub next_status: OrderStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClaimDigest(pub [u8; 32]);

impl ClaimDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<Digest> for ClaimDigest {
    fn from(value: Digest) -> Self {
        Self(value.into())
    }
}

impl From<ClaimDigest> for Digest {
    fn from(value: ClaimDigest) -> Self {
        Self::from(value.0)
    }
}

impl From<ClaimDigest> for [u8; 32] {
    fn from(value: ClaimDigest) -> Self {
        value.0
    }
}

pub struct AssessorArtifact {
    pub claim_digest: ClaimDigest,
    pub selectors: Vec<AssessorSelector>,
    pub callbacks: Vec<AssessorCallback>,
}

pub struct FulfillmentOrder {
    pub order_id: String,
    pub request: ProofRequest,
    pub proof_id: String,
    pub compressed_proof_id: Option<String>,
    pub program_id: [u8; 32],
}

pub struct FulfillmentBatch {
    pub backend_id: BackendId,
    pub aggregation_state: Option<AggregationState>,
    pub assessor_proof_id: Option<String>,
    pub eip712_domain: EIP712DomainSaltless,
    pub prover_address: Address,
    pub orders: Vec<FulfillmentOrder>,
}

pub struct RootSubmission {
    pub root: B256,
    pub seal: Bytes,
}

pub struct OrderFulfillmentArtifact {
    pub order_id: String,
    pub fulfillment: MarketFulfillment,
}

pub struct OrderFulfillmentFailure {
    pub order_id: String,
    pub error: anyhow::Error,
}

pub enum OrderFulfillmentResult {
    Fulfilled(OrderFulfillmentArtifact),
    Failed(OrderFulfillmentFailure),
}

pub struct FulfillmentArtifacts {
    pub root_submission: Option<RootSubmission>,
    pub orders: Vec<OrderFulfillmentResult>,
    pub assessor_receipt: AssessorReceipt,
}

#[derive(Clone, Debug)]
pub struct BatchSizeEstimate {
    pub size: usize,
}

pub struct UpdateBatch {
    pub batch_id: usize,
    pub existing_order_ids: Vec<String>,
    pub aggregation_state: Option<AggregationState>,
    pub new_proofs: Vec<AggregationOrder>,
    pub new_compressed_proofs: Vec<AggregationOrder>,
    pub finalize: bool,
}

pub struct BatchUpdate {
    pub aggregation_state: AggregationState,
    pub assessor_proof_id: Option<String>,
    pub set_builder_proving_secs: Option<f64>,
    pub assessor_proving_secs: Option<f64>,
}

pub struct CloseBatch {
    pub batch_id: usize,
    pub aggregation_proof_id: String,
    pub order_ids: Vec<String>,
}

pub struct BatchClose {
    pub compressed_proof_id: String,
    pub compression_secs: f64,
}

#[async_trait]
pub(crate) trait BatchProcessor: Send + Sync {
    async fn estimate_batch_size(&self, order_ids: &[String]) -> Result<BatchSizeEstimate>;

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate>;

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, provers::ProverError>;
}

pub(crate) type BatchProcessorObj = Arc<dyn BatchProcessor>;

#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> &BackendId;

    fn supports(&self, selector: FixedBytes<4>) -> bool;

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;

    async fn cancel_order(&self, order: &Order) -> Result<()>;

    async fn estimate_batch_size(&self, order_ids: &[String]) -> Result<BatchSizeEstimate>;

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate>;

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, provers::ProverError>;

    async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<FulfillmentArtifacts>;
}

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

#[derive(Clone)]
pub struct Risc0Backend {
    id: BackendId,
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: ConfigurableDownloader,
    priority_requestors: PriorityRequestors,
    set_builder_program_id: Option<Digest>,
    batch_processor: Option<BatchProcessorObj>,
}

impl Risc0Backend {
    pub fn new(
        id: BackendId,
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Self {
        Self {
            id,
            prover,
            snark_prover,
            downloader,
            priority_requestors,
            set_builder_program_id: None,
            batch_processor: None,
        }
    }

    pub fn with_set_builder_program_id(mut self, set_builder_program_id: Digest) -> Self {
        self.set_builder_program_id = Some(set_builder_program_id);
        self
    }

    pub(crate) fn with_batch_processor(mut self, batch_processor: BatchProcessorObj) -> Self {
        self.batch_processor = Some(batch_processor);
        self
    }

    fn batch_processor(&self) -> Result<&BatchProcessorObj> {
        self.batch_processor.as_ref().context("RISC0 backend is missing batch processor")
    }

    async fn start_order(&self, order: &Order) -> Result<String> {
        let order_id = order.id();

        if let Some(existing_proof_id) = order.proof_id.clone() {
            tracing::debug!("Using existing proof {existing_proof_id} for order {order_id}");
            return Ok(existing_proof_id);
        }

        tracing::info!("Proving order {order_id}");

        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_image_uri(&self.prover, &order.request, &self.downloader)
                .await
                .with_context(|| format!("Failed to upload image for order {order_id}"))?,
        };

        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_input_uri(
                &self.prover,
                &order.request,
                &self.downloader,
                &self.priority_requestors,
            )
            .await
            .with_context(|| format!("Failed to upload input for order {order_id}"))?,
        };

        let proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
            .await
            .with_context(|| format!("Failed to prove customer proof STARK order {order_id}"))?;

        tracing::debug!("Order {order_id} being proved, proof id: {proof_id}");
        Ok(proof_id)
    }

    async fn compress_order_proof(
        &self,
        order_id: &str,
        stark_proof_id: &str,
        compression_type: CompressionType,
    ) -> Result<String> {
        let proof_id = match compression_type {
            CompressionType::Groth16 => self
                .snark_prover
                .compress(stark_proof_id)
                .await
                .with_context(|| format!("Failed to compress proof for order {order_id}"))?,
            CompressionType::Blake3Groth16 => {
                self.snark_prover.compress_blake3_groth16(stark_proof_id).await.with_context(
                    || format!("Failed to compress blake3 groth16 proof for order {order_id}"),
                )?
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        };

        match compression_type {
            CompressionType::Groth16 => {
                tracing::trace!(
                    "Verifying compressed Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::Blake3Groth16 => {
                tracing::trace!(
                    "Verifying compressed Blake3 Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_blake3_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        }

        Ok(proof_id)
    }
}

#[async_trait]
impl Backend for Risc0Backend {
    fn id(&self) -> &BackendId {
        &self.id
    }

    fn supports(&self, selector: FixedBytes<4>) -> bool {
        selector == UNSPECIFIED_SELECTOR
            || is_groth16_selector(selector)
            || is_blake3_groth16_selector(selector)
    }

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let order = cmd.order;
        let order_id = order.id();

        let Some(stark_proof_id) = order.proof_id.clone() else {
            return Ok(OrderProcessProgress::Started { proof_id: self.start_order(&order).await? });
        };

        let proof_res = self
            .prover
            .wait_for_stark(&stark_proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        let compression_type = order.compression_type();
        tracing::debug!(
            "Order {order_id} has compression_type: {compression_type:?}, snark_proof_id: {:?}",
            order.compressed_proof_id
        );

        if compression_type != CompressionType::None && order.compressed_proof_id.is_none() {
            let compressed_proof_id =
                self.compress_order_proof(&order_id, &stark_proof_id, compression_type).await?;
            return Ok(OrderProcessProgress::Compressed {
                proof_id: stark_proof_id,
                compressed_proof_id,
            });
        }

        let next_status = match compression_type {
            CompressionType::None => OrderStatus::PendingAgg,
            CompressionType::Groth16 | CompressionType::Blake3Groth16 => {
                OrderStatus::SkipAggregation
            }
        };

        tracing::info!(
            "Customer Proof complete for proof_id: {stark_proof_id}, order_id: {order_id} cycles: {:?} time: {}",
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time,
        );

        Ok(OrderProcessProgress::Completed(ProcessedOrder {
            backend_id: self.id.clone(),
            order_id,
            proof_id: stark_proof_id,
            compressed_proof_id: order.compressed_proof_id,
            next_status,
        }))
    }

    async fn cancel_order(&self, order: &Order) -> Result<()> {
        if let Some(proof_id) = order.proof_id.as_ref() {
            if matches!(order.status, OrderStatus::Proving) {
                tracing::debug!("Cancelling proof {} for order {}", proof_id, order.id());
                self.prover
                    .cancel_stark(proof_id)
                    .await
                    .with_context(|| format!("Failed to cancel proof {proof_id}"))?;
            }
        }

        Ok(())
    }

    async fn estimate_batch_size(&self, order_ids: &[String]) -> Result<BatchSizeEstimate> {
        self.batch_processor()?.estimate_batch_size(order_ids).await
    }

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate> {
        self.batch_processor()?.update_batch(cmd).await
    }

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, provers::ProverError> {
        match self.batch_processor() {
            Ok(batch_processor) => batch_processor.close_batch(cmd).await,
            Err(err) => Err(provers::ProverError::ProverInternalError(err.to_string())),
        }
    }

    async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<FulfillmentArtifacts> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot build fulfillments for backend {}",
            self.id,
            cmd.backend_id
        );

        let aggregation_state = cmd
            .aggregation_state
            .as_ref()
            .context("Cannot submit batch with no recorded aggregation state")?;
        let assessor_proof_id = cmd
            .assessor_proof_id
            .as_ref()
            .context("Cannot submit batch with no assessor receipt")?;
        let set_builder_program_id = self
            .set_builder_program_id
            .context("RISC0 backend is missing set-builder program id")?;

        let submission = Risc0Submission::new(self.snark_prover.clone());
        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: set_builder_program_id };
        let groth16_proof_id = aggregation_state
            .groth16_proof_id
            .as_ref()
            .context("Cannot submit batch with no recorded Groth16 proof ID")?;
        anyhow::ensure!(
            !aggregation_state.claim_digests.is_empty(),
            "Cannot submit batch with no claim digests"
        );
        anyhow::ensure!(
            aggregation_state.guest_state.mmr.is_finalized(),
            "Cannot submit guest state that is not finalized"
        );

        let batch_root = risc0_aggregation::merkle_root(&aggregation_state.claim_digests);
        anyhow::ensure!(
            aggregation_state.guest_state.mmr.clone().finalized_root().unwrap() == batch_root,
            "Guest state finalized root is inconsistent with claim digests"
        );
        let root_submission = RootSubmission {
            root: B256::from_slice(batch_root.as_bytes()),
            seal: submission.encode_groth16_seal(groth16_proof_id).await?.into(),
        };

        let mut orders = Vec::with_capacity(cmd.orders.len());
        for order in cmd.orders {
            let order_id = order.order_id.clone();
            let res = async {
                let order_img_id = Digest::from(order.program_id);
                let order_journal = self
                    .prover
                    .get_journal(&order.proof_id)
                    .await
                    .context("Failed to get order journal from prover")?
                    .context("Order proof Journal missing")?;
                let order_journal_digest = order_journal.digest();
                let order_claim_digest =
                    submission.claim_digest(order_img_id, order_journal_digest);

                let seal = if is_groth16_selector(order.request.requirements.selector)
                    || is_blake3_groth16_selector(order.request.requirements.selector)
                {
                    let compressed_proof_id = order.compressed_proof_id.with_context(|| {
                        format!(
                            "Order {} missing compressed proof ID for submission",
                            order.order_id
                        )
                    })?;
                    submission
                        .encode_seal_for_selector(
                            order.request.requirements.selector,
                            &compressed_proof_id,
                        )
                        .await
                        .with_context(|| {
                            format!("Failed to encode seal for order {}", order.order_id)
                        })?
                } else {
                    let order_claim =
                        ReceiptClaim::ok(order_img_id, MaybePruned::Pruned(order_journal_digest));
                    let order_claim_index = aggregation_state
                        .claim_digests
                        .iter()
                        .position(|claim| *claim == order_claim_digest)
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Failed to find order claim {order_claim:x?} in aggregated claims"
                            )
                        })?;
                    let order_path = risc0_aggregation::merkle_path(
                        &aggregation_state.claim_digests,
                        order_claim_index,
                    );
                    tracing::debug!(
                        "Merkle path for order {} : {:x?} : {order_path:x?}",
                        order.order_id,
                        order_claim_digest
                    );
                    let set_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
                        order_claim,
                        order_path,
                        inclusion_params.digest(),
                    );
                    set_inclusion_receipt.abi_encode_seal().context("Failed to encode seal")?
                };

                tracing::debug!(
                    "Seal for order {} : {}",
                    order.order_id,
                    hex::encode(seal.clone())
                );

                let request_digest =
                    order.request.eip712_signing_hash(&cmd.eip712_domain.alloy_struct());
                let request_id = order.request.id;
                let predicate_type = order.request.requirements.predicate.predicateType;

                let (claim_digest, fulfillment_data, fulfillment_data_type) = match predicate_type {
                    PredicateType::ClaimDigestMatch => (
                        ClaimDigest(
                            order
                                .request
                                .requirements
                                .predicate
                                .data
                                .0
                                .as_ref()
                                .try_into()
                                .context("claim digest predicate has invalid length")?,
                        ),
                        vec![],
                        FulfillmentDataType::None,
                    ),
                    PredicateType::PrefixMatch | PredicateType::DigestMatch => (
                        ClaimDigest::from(order_claim_digest),
                        FulfillmentDataImageIdAndJournal {
                            imageId: <[u8; 32]>::from(order_img_id).into(),
                            journal: order_journal.into(),
                        }
                        .abi_encode(),
                        FulfillmentDataType::ImageIdAndJournal,
                    ),
                    _ => anyhow::bail!("Invalid predicate type: {predicate_type:?}"),
                };

                Ok(OrderFulfillmentArtifact {
                    order_id: order.order_id,
                    fulfillment: MarketFulfillment {
                        id: request_id,
                        requestDigest: request_digest,
                        fulfillmentData: fulfillment_data.into(),
                        fulfillmentDataType: fulfillment_data_type,
                        claimDigest: <[u8; 32]>::from(claim_digest).into(),
                        seal: seal.into(),
                    },
                })
            }
            .await;

            orders.push(match res {
                Ok(artifact) => OrderFulfillmentResult::Fulfilled(artifact),
                Err(error) => {
                    OrderFulfillmentResult::Failed(OrderFulfillmentFailure { order_id, error })
                }
            });
        }

        let assessor = submission.assessor_receipt(assessor_proof_id).await?;
        let assessor_claim = Digest::from(assessor.claim_digest);
        let assessor_claim_index = aggregation_state
            .claim_digests
            .iter()
            .position(|claim| *claim == assessor_claim)
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to find order claim assessor claim in aggregated claims")
            })?;
        let assessor_path =
            risc0_aggregation::merkle_path(&aggregation_state.claim_digests, assessor_claim_index);
        tracing::debug!("Merkle path for assessor : {:x?} : {assessor_path:x?}", assessor_claim);

        let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
            // TODO: Set inclusion proofs, when ABI encoded, currently don't contain anything
            // derived from the claim. So instead of constructing the journal, we simply use the
            // zero digest. We should either plumb through the data for the assessor journal, or we
            // should make an explicit way to encode an inclusion proof without the claim.
            ReceiptClaim::ok(Digest::ZERO, MaybePruned::Pruned(Digest::ZERO)),
            assessor_path,
            inclusion_params.digest(),
        );
        let assessor_seal =
            assessor_seal.abi_encode_seal().context("ABI encode assessor set inclusion receipt")?;

        Ok(FulfillmentArtifacts {
            root_submission: Some(root_submission),
            orders,
            assessor_receipt: AssessorReceipt {
                seal: assessor_seal.into(),
                selectors: assessor.selectors,
                prover: cmd.prover_address,
                callbacks: assessor.callbacks,
            },
        })
    }
}

#[derive(Clone)]
pub struct Risc0BatchProcessor {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    set_builder_guest_id: Digest,
    assessor_guest_id: Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
}

#[derive(Clone)]
pub struct Risc0Submission {
    prover: ProverObj,
}

impl Risc0Submission {
    pub fn new(prover: ProverObj) -> Self {
        Self { prover }
    }

    pub async fn encode_seal_for_selector(
        &self,
        selector: FixedBytes<4>,
        proof_id: &str,
    ) -> Result<Vec<u8>> {
        if is_groth16_selector(selector) {
            self.encode_groth16_seal(proof_id).await
        } else if is_blake3_groth16_selector(selector) {
            self.encode_blake3_groth16_seal(proof_id).await
        } else {
            anyhow::bail!("selector {selector:?} does not identify a compressed proof seal")
        }
    }

    pub async fn encode_groth16_seal(&self, proof_id: &str) -> Result<Vec<u8>> {
        let groth16_receipt = self
            .prover
            .get_compressed_receipt(proof_id)
            .await
            .context("Failed to fetch g16 receipt")?
            .context("Groth16 receipt missing")?;

        let groth16_receipt: Receipt =
            bincode::deserialize(&groth16_receipt).context("Failed to deserialize g16 receipt")?;

        encode_seal(&groth16_receipt).context("Failed to encode g16 receipt seal")
    }

    pub async fn encode_blake3_groth16_seal(&self, proof_id: &str) -> Result<Vec<u8>> {
        let blake3_receipt = self
            .prover
            .get_blake3_groth16_receipt(proof_id)
            .await
            .context("Failed to fetch blake3 groth16 receipt")?
            .context("Blake3 Groth16 receipt missing")?;

        let blake3_receipt: Blake3Groth16Receipt = bincode::deserialize(&blake3_receipt)
            .context("Failed to deserialize Blake3 Groth16 receipt")?;

        let mut encoded_seal = encode_seal(&blake3_receipt.into())
            .context("Failed to encode Blake3 Groth16 receipt seal")?;
        if is_dev_mode() {
            let fake_selector = &[0xFFu8, 0xFF, 0x00, 0x00];
            encoded_seal.splice(0..4, fake_selector.iter().cloned());
        }

        Ok(encoded_seal)
    }

    pub fn claim_digest(&self, image_id: Digest, journal_digest: Digest) -> Digest {
        ReceiptClaim::ok(image_id, MaybePruned::Pruned(journal_digest)).digest()
    }

    pub async fn assessor_receipt(&self, proof_id: &str) -> Result<AssessorArtifact> {
        let receipt = self
            .prover
            .get_receipt(proof_id)
            .await
            .context("Failed to get assessor receipt")?
            .context("Assessor receipt missing")?;
        let claim_digest = receipt
            .claim()
            .with_context(|| format!("Receipt for assessor {proof_id} missing claim"))?
            .value()
            .with_context(|| format!("Receipt for assessor {proof_id} claims pruned"))?
            .digest();
        let journal = AssessorJournal::abi_decode(&receipt.journal.bytes)
            .with_context(|| format!("Failed to decode assessor journal for {proof_id}"))?;

        Ok(AssessorArtifact {
            claim_digest: claim_digest.into(),
            selectors: journal.selectors,
            callbacks: journal.callbacks,
        })
    }
}

impl Risc0BatchProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        set_builder_guest_id: Digest,
        assessor_guest_id: Digest,
        market_addr: Address,
        prover_addr: Address,
        chain_id: u64,
    ) -> Self {
        Self {
            db,
            config,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
        }
    }

    async fn validate_and_extract_claim(&self, proof_id: &str) -> Result<ReceiptClaim> {
        let receipt = self
            .prover
            .get_receipt(proof_id)
            .await
            .with_context(|| format!("Failed to fetch receipt for {proof_id}"))?
            .with_context(|| format!("Receipt not found for {proof_id}"))?;

        receipt
            .verify_integrity_with_context(&Default::default())
            .with_context(|| format!("Receipt verification failed for {proof_id}"))?;

        let claim = receipt
            .claim()
            .with_context(|| format!("Failed to get claim for {proof_id}"))?
            .value()
            .with_context(|| format!("Failed to extract claim value for {proof_id}"))?;

        Ok(prune_receipt_claim_journal(claim))
    }

    pub async fn prove_set_builder(
        &self,
        aggregation_state: Option<&AggregationState>,
        proofs: &[String],
        finalize: bool,
        all_orders: &[String],
    ) -> Result<AggregationState> {
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        let mut valid_proof_ids = Vec::<String>::with_capacity(proofs.len());

        for proof_id in proofs {
            match self.validate_and_extract_claim(proof_id).await {
                Ok(claim) => {
                    claims.push(claim);
                    valid_proof_ids.push(proof_id.clone());
                }
                Err(e) => {
                    tracing::error!(
                        "Error fetching proof from batch: {e:?} containing orders {:?}, excluding",
                        all_orders
                    );
                }
            }
        }

        if claims.is_empty() {
            anyhow::bail!(format!("No valid proofs found in batch with orders {:?}", all_orders));
        }

        if valid_proof_ids.len() < proofs.len() {
            tracing::warn!(
                "Excluded {} invalid proofs from batch with orders {:?}. Valid: {}/{}",
                proofs.len() - valid_proof_ids.len(),
                all_orders,
                valid_proof_ids.len(),
                proofs.len()
            );
        }

        let input = aggregation_state
            .map_or(GuestState::initial(self.set_builder_guest_id), |s| s.guest_state.clone())
            .into_input(claims.clone(), finalize)
            .context("Failed to build set builder input")?;

        let assumption_ids: Vec<String> = aggregation_state
            .map(|s| s.proof_id.clone())
            .into_iter()
            .chain(valid_proof_ids.iter().cloned())
            .collect();

        let input_data =
            provers::encode_input(&input).context("Failed to encode set-builder proof input")?;
        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .context("Failed to upload set-builder input")?;

        let (retry_count, sleep_ms) = {
            let config = self.config.lock_all().context("Failed to lock config")?;
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

        tracing::debug!("Starting proving of set-builder with orders {:?}", all_orders);
        let (proof_res, journal) = retry_with_context(
            retry_count,
            sleep_ms,
            || async {
                let proof_res = self
                    .prover
                    .prove_and_monitor_stark_high(
                        &self.set_builder_guest_id.to_string(),
                        &input_id,
                        assumption_ids.clone(),
                    )
                    .await
                    .map_err(|e| {
                        provers::ProverError::ProverInternalError(format!(
                            "Failed to prove set-builder: {e}"
                        ))
                    })?;

                tracing::debug!(
                    "Set-builder proof complete with orders {:?}, proof id: {} cycles: {:?} time: {}",
                    all_orders,
                    proof_res.id,
                    proof_res.stats.as_ref().map(|s| s.total_cycles),
                    proof_res.elapsed_time
                );

                let receipt = self
                    .prover
                    .get_receipt(&proof_res.id)
                    .await
                    .map_err(|e| {
                        provers::ProverError::ProverInternalError(format!(
                            "Failed to get receipt for set-builder: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        provers::ProverError::NotFound(format!(
                            "Receipt missing for set-builder: {}",
                            proof_res.id
                        ))
                    })?;

                receipt.verify(self.set_builder_guest_id).map_err(|e| {
                    provers::ProverError::ProverInternalError(format!(
                        "Set builder proof produced invalid receipt: {e}"
                    ))
                })?;

                let journal = receipt.journal.bytes;

                Ok::<_, provers::ProverError>((proof_res, journal))
            },
            "set_builder_prove_and_get_journal",
            &format!("orders {:?}", all_orders),
        )
        .await?;

        let guest_state = GuestState::decode(&journal).context("Failed to decode guest output")?;
        let claim_digests = aggregation_state
            .map(|s| s.claim_digests.clone())
            .unwrap_or_default()
            .into_iter()
            .chain(claims.into_iter().map(|claim| claim.digest()))
            .collect();

        Ok(AggregationState {
            guest_state,
            proof_id: proof_res.id,
            claim_digests,
            groth16_proof_id: None,
        })
    }

    pub async fn prove_assessor(&self, order_ids: &[String]) -> Result<String> {
        let mut fills = vec![];

        for order_id in order_ids {
            let order = self
                .db
                .get_order(order_id)
                .await
                .with_context(|| format!("Failed to get DB order ID {order_id}"))?
                .with_context(|| format!("order ID {order_id} missing from DB"))?;

            let proof_id = order
                .proof_id
                .with_context(|| format!("Missing proof_id for order: {order_id}"))?;

            let journal = self
                .prover
                .get_journal(&proof_id)
                .await
                .with_context(|| format!("Failed to get {proof_id} journal"))?
                .with_context(|| format!("{proof_id} journal missing"))?;

            let fulfillment_data = match order.request.requirements.predicate.predicateType {
                PredicateType::ClaimDigestMatch => FulfillmentData::None,
                _ => FulfillmentData::from_image_id_and_journal(
                    Digest::from_hex(
                        order
                            .image_id
                            .with_context(|| format!("Missing image_id for order: {order_id}"))?,
                    )?,
                    journal,
                ),
            };

            fills.push(Fulfillment {
                request: order.request.clone(),
                signature: order.client_sig.clone().to_vec(),
                fulfillment_data,
            })
        }

        let order_count = fills.len();
        let input = AssessorInput {
            fills,
            domain: eip712_domain(self.market_addr, self.chain_id),
            prover_address: self.prover_addr,
        };
        let stdin = GuestEnv::builder().write_frame(&input.encode()).stdin;

        let input_id =
            self.prover.upload_input(stdin).await.context("Failed to upload assessor input")?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark_high(&self.assessor_guest_id.to_string(), &input_id, vec![])
            .await
            .context("Failed to prove assesor stark")?;

        tracing::debug!(
            "Assessor proof completed, proof id: {} count: {} cycles: {:?} time: {}",
            proof_res.id,
            order_count,
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time
        );

        Ok(proof_res.id)
    }

    pub async fn compress_batch_proof(
        &self,
        batch_id: usize,
        aggregation_proof_id: &str,
        orders: &[String],
    ) -> Result<String, provers::ProverError> {
        let (retry_count, sleep_ms) = {
            let config = self.config.lock_all().map_err(|e| {
                provers::ProverError::ProverInternalError(format!("Failed to lock config: {e}"))
            })?;
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

        let context = format!("batch {batch_id} with orders {:?}", orders);
        retry_with_context(
            retry_count,
            sleep_ms,
            || async {
                let proof_id = self.prover.compress_high(aggregation_proof_id).await?;
                provers::verify_groth16_receipt(&self.prover, &proof_id).await?;
                Ok::<String, provers::ProverError>(proof_id)
            },
            "compress_and_verify",
            &context,
        )
        .await
    }
}

#[async_trait]
impl BatchProcessor for Risc0BatchProcessor {
    async fn estimate_batch_size(&self, order_ids: &[String]) -> Result<BatchSizeEstimate> {
        let mut size = 0;
        for order_id in order_ids {
            let order = self
                .db
                .get_order(order_id)
                .await
                .with_context(|| format!("Failed to get order {order_id}"))?
                .with_context(|| format!("Order {order_id} missing from DB"))?;

            let proof_id =
                order.proof_id.with_context(|| format!("Missing proof_id for order {order_id}"))?;

            let journal = self
                .prover
                .get_journal(&proof_id)
                .await
                .with_context(|| format!("Failed to get journal for {proof_id}"))?
                .with_context(|| format!("Journal for {proof_id} missing"))?;

            // For RISC0 claim-digest match orders, the journal is not included in calldata.
            if !matches!(
                order.request.requirements.predicate.predicateType,
                PredicateType::ClaimDigestMatch
            ) {
                size += journal.len();
            }
        }

        Ok(BatchSizeEstimate { size })
    }

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate> {
        let all_orders: Vec<String> = cmd
            .existing_order_ids
            .iter()
            .chain(cmd.new_proofs.iter().map(|p| &p.order_id))
            .chain(cmd.new_compressed_proofs.iter().map(|p| &p.order_id))
            .cloned()
            .collect();

        let mut assessor_proving_secs = None;
        let assessor_proof_id = if cmd.finalize {
            tracing::debug!(
                "Running assessor for batch {} with orders {:?}",
                cmd.batch_id,
                all_orders
            );

            let assessor_start = std::time::Instant::now();
            let assessor_proof_id = self
                .prove_assessor(&all_orders)
                .await
                .with_context(|| format!("Failed to prove assessor with orders {all_orders:?}"))?;
            assessor_proving_secs = Some(assessor_start.elapsed().as_secs_f64());

            tracing::debug!(
                "Assessor proof complete for batch {} with orders {:?}, proof id: {}",
                cmd.batch_id,
                all_orders,
                assessor_proof_id
            );

            Some(assessor_proof_id)
        } else {
            None
        };

        let proof_ids: Vec<String> = cmd
            .new_proofs
            .iter()
            .map(|proof| proof.proof_id.clone())
            .chain(assessor_proof_id.iter().cloned())
            .collect();

        tracing::debug!(
            "Running set builder for batch {} of orders {:?} and proofs {:?}",
            cmd.batch_id,
            all_orders,
            proof_ids
        );
        let set_builder_start = std::time::Instant::now();
        let aggregation_state = self
            .prove_set_builder(
                cmd.aggregation_state.as_ref(),
                &proof_ids,
                cmd.finalize,
                &all_orders,
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to prove set builder for batch {} with orders {:?}",
                    cmd.batch_id, all_orders
                )
            })?;
        let set_builder_proving_secs = Some(set_builder_start.elapsed().as_secs_f64());

        tracing::debug!(
            "Completed aggregation into batch {} of orders {:?} and proofs {:?}",
            cmd.batch_id,
            all_orders,
            proof_ids
        );

        Ok(BatchUpdate {
            aggregation_state,
            assessor_proof_id,
            set_builder_proving_secs,
            assessor_proving_secs,
        })
    }

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, provers::ProverError> {
        let start = std::time::Instant::now();
        let compressed_proof_id = self
            .compress_batch_proof(cmd.batch_id, &cmd.aggregation_proof_id, &cmd.order_ids)
            .await?;

        Ok(BatchClose { compressed_proof_id, compression_secs: start.elapsed().as_secs_f64() })
    }
}
