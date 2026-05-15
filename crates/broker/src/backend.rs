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

use alloy::primitives::{Address, FixedBytes};
use alloy::sol_types::SolValue;
use async_trait::async_trait;
use blake3_groth16::Blake3Groth16Receipt;
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{
    contracts::{
        eip712_domain, encode_seal, AssessorCallback, AssessorJournal, AssessorSelector,
        FulfillmentData, PredicateType, UNSPECIFIED_SELECTOR,
    },
    input::GuestEnv,
    selector::{is_blake3_groth16_selector, is_groth16_selector},
};
use hex::FromHex;
use risc0_aggregation::GuestState;
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

        async fn encode_batch_seal(&self, cmd: EncodeBatchSeal) -> Result<Vec<u8>> {
            self.ensure_backend_id(&cmd.backend_id)?;
            Ok(cmd.proof_id.into_bytes())
        }

        async fn encode_order_seal(&self, cmd: EncodeOrderSeal) -> Result<Vec<u8>> {
            self.ensure_backend_id(&cmd.backend_id)?;
            Ok(cmd.proof_id.into_bytes())
        }

        fn compute_claim_digest(&self, cmd: ComputeClaimDigest) -> Result<ClaimDigest> {
            self.ensure_backend_id(&cmd.backend_id)?;
            Ok(ClaimDigest(cmd.program_id))
        }

        async fn fetch_assessor_receipt(
            &self,
            cmd: FetchAssessorReceipt,
        ) -> Result<AssessorArtifact> {
            self.ensure_backend_id(&cmd.backend_id)?;
            Ok(AssessorArtifact {
                claim_digest: ClaimDigest::from(Digest::ZERO),
                selectors: Vec::new(),
                callbacks: Vec::new(),
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

        assert!(router.supports(selector(1)));
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

pub struct EncodeBatchSeal {
    pub backend_id: BackendId,
    pub proof_id: String,
}

pub struct EncodeOrderSeal {
    pub backend_id: BackendId,
    pub selector: FixedBytes<4>,
    pub proof_id: String,
}

pub struct ComputeClaimDigest {
    pub backend_id: BackendId,
    pub program_id: [u8; 32],
    pub public_output_digest: [u8; 32],
}

pub struct FetchAssessorReceipt {
    pub backend_id: BackendId,
    pub proof_id: String,
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
pub trait BatchProcessor: Send + Sync {
    async fn estimate_batch_size(&self, order_ids: &[String]) -> Result<BatchSizeEstimate>;

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate>;

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, provers::ProverError>;
}

pub type BatchProcessorObj = Arc<dyn BatchProcessor>;

#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> &BackendId;

    fn supports(&self, selector: FixedBytes<4>) -> bool;

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;

    async fn cancel_order(&self, order: &Order) -> Result<()>;

    async fn encode_batch_seal(&self, cmd: EncodeBatchSeal) -> Result<Vec<u8>>;

    async fn encode_order_seal(&self, cmd: EncodeOrderSeal) -> Result<Vec<u8>>;

    fn compute_claim_digest(&self, cmd: ComputeClaimDigest) -> Result<ClaimDigest>;

    async fn fetch_assessor_receipt(&self, cmd: FetchAssessorReceipt) -> Result<AssessorArtifact>;
}

pub type BackendObj = Arc<dyn Backend>;

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
    batch_processor: Option<BatchProcessorObj>,
}

impl BackendEntry {
    pub fn new(selectors: impl IntoIterator<Item = FixedBytes<4>>, backend: BackendObj) -> Self {
        let id = backend.id().clone();
        Self { id, selectors: selectors.into_iter().collect(), backend, batch_processor: None }
    }

    pub fn with_batch_processor(mut self, batch_processor: BatchProcessorObj) -> Self {
        self.batch_processor = Some(batch_processor);
        self
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

    fn backend_for_order(&self, order: &Order) -> Result<BackendObj> {
        let selector = order.request.requirements.selector;
        let backend_id = self
            .routes
            .get(&selector)
            .cloned()
            .with_context(|| format!("no backend registered for selector {selector:?}"))?;
        self.backends
            .get(&backend_id)
            .map(|entry| entry.backend.clone())
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    fn backend_for_id(&self, backend_id: &BackendId) -> Result<BackendObj> {
        self.backends
            .get(backend_id)
            .map(|entry| entry.backend.clone())
            .with_context(|| format!("backend {backend_id} is not registered"))
    }

    pub fn batch_processors(&self) -> Result<Vec<(BackendId, BatchProcessorObj)>> {
        self.backends
            .values()
            .map(|entry| {
                let batch_processor = entry
                    .batch_processor
                    .clone()
                    .with_context(|| format!("backend {} has no batch processor", entry.id()))?;
                Ok((entry.id.clone(), batch_processor))
            })
            .collect()
    }
}

#[async_trait]
impl Backend for BackendRouter {
    fn id(&self) -> &BackendId {
        static ROUTER_ID: std::sync::OnceLock<BackendId> = std::sync::OnceLock::new();
        ROUTER_ID.get_or_init(|| BackendId::new("backend_router").expect("static backend id"))
    }

    fn supports(&self, selector: FixedBytes<4>) -> bool {
        self.routes.contains_key(&selector)
    }

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let backend = self.backend_for_order(&cmd.order)?;
        backend.process_order(cmd).await
    }

    async fn cancel_order(&self, order: &Order) -> Result<()> {
        let backend = self.backend_for_order(order)?;
        backend.cancel_order(order).await
    }

    async fn encode_batch_seal(&self, cmd: EncodeBatchSeal) -> Result<Vec<u8>> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.encode_batch_seal(cmd).await
    }

    async fn encode_order_seal(&self, cmd: EncodeOrderSeal) -> Result<Vec<u8>> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.encode_order_seal(cmd).await
    }

    fn compute_claim_digest(&self, cmd: ComputeClaimDigest) -> Result<ClaimDigest> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.compute_claim_digest(cmd)
    }

    async fn fetch_assessor_receipt(&self, cmd: FetchAssessorReceipt) -> Result<AssessorArtifact> {
        let backend = self.backend_for_id(&cmd.backend_id)?;
        backend.fetch_assessor_receipt(cmd).await
    }
}

#[derive(Clone)]
pub struct Risc0Backend {
    id: BackendId,
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: ConfigurableDownloader,
    priority_requestors: PriorityRequestors,
}

impl Risc0Backend {
    pub fn new(
        id: BackendId,
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Self {
        Self { id, prover, snark_prover, downloader, priority_requestors }
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

    async fn encode_batch_seal(&self, cmd: EncodeBatchSeal) -> Result<Vec<u8>> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot encode batch seal for backend {}",
            self.id,
            cmd.backend_id
        );
        Risc0Submission::new(self.snark_prover.clone()).encode_groth16_seal(&cmd.proof_id).await
    }

    async fn encode_order_seal(&self, cmd: EncodeOrderSeal) -> Result<Vec<u8>> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot encode order seal for backend {}",
            self.id,
            cmd.backend_id
        );
        Risc0Submission::new(self.snark_prover.clone())
            .encode_seal_for_selector(cmd.selector, &cmd.proof_id)
            .await
    }

    fn compute_claim_digest(&self, cmd: ComputeClaimDigest) -> Result<ClaimDigest> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot compute claim digest for backend {}",
            self.id,
            cmd.backend_id
        );
        Ok(Risc0Submission::new(self.snark_prover.clone())
            .claim_digest(Digest::from(cmd.program_id), Digest::from(cmd.public_output_digest))
            .into())
    }

    async fn fetch_assessor_receipt(&self, cmd: FetchAssessorReceipt) -> Result<AssessorArtifact> {
        anyhow::ensure!(
            cmd.backend_id == self.id,
            "backend {} cannot fetch assessor receipt for backend {}",
            self.id,
            cmd.backend_id
        );
        Risc0Submission::new(self.snark_prover.clone()).assessor_receipt(&cmd.proof_id).await
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
