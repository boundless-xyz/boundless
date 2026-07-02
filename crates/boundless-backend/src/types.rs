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

use std::{fmt, sync::Arc};

use alloy::primitives::{Address, Bytes, FixedBytes, B256};
use async_trait::async_trait;
use boundless_market::prover_utils::{prover::ProverError, FulfillmentType};
use boundless_market::prover_utils::{
    EvaluationLimits, EvaluationRequest, OrderPricingError, RequestEvaluation,
};
use boundless_market::{
    contracts::{
        AssessorCallback, AssessorSelector, EIP712DomainSaltless, Fulfillment as MarketFulfillment,
        ProofRequest,
    },
    selector::ProofType,
};
use serde::{Deserialize, Serialize};

use anyhow::Result;

macro_rules! string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }
    };
}

string_id!(
    BackendId,
    "Stable broker-side identity for a backend registration. Many selectors may route to one backend; broker policy and batching are keyed by backend."
);
string_id!(ProofId, "Durable backend proof handle.");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Build from a backend's native 32-byte digest (e.g. `risc0_zkvm::sha::Digest`).
    pub fn from_native<T: Into<[u8; 32]>>(value: T) -> Self {
        Self(value.into())
    }

    /// Convert into a backend's native 32-byte digest type. Inverse of [`Digest::from_native`].
    pub fn to_native<T: From<[u8; 32]>>(self) -> T {
        T::from(self.0)
    }
}

impl From<[u8; 32]> for Digest {
    fn from(value: [u8; 32]) -> Self {
        Self(value)
    }
}

impl From<Digest> for [u8; 32] {
    fn from(value: Digest) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProgramId(pub Digest);

impl ProgramId {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Build from a backend's native 32-byte digest. See [`Digest::from_native`].
    pub fn from_native<T: Into<[u8; 32]>>(value: T) -> Self {
        Self(Digest::from_native(value))
    }

    /// Convert into a backend's native 32-byte digest type.
    pub fn to_native<T: From<[u8; 32]>>(self) -> T {
        self.0.to_native()
    }
}

impl From<[u8; 32]> for ProgramId {
    fn from(value: [u8; 32]) -> Self {
        Self(Digest::from(value))
    }
}

impl From<ProgramId> for [u8; 32] {
    fn from(value: ProgramId) -> Self {
        value.0.into()
    }
}

/// Backend-neutral input for processing one order.
#[derive(Clone, Debug)]
pub struct ProcessOrder {
    pub order_id: String,
    pub request: ProofRequest,
    pub image_id: Option<String>,
    pub input_id: Option<String>,
    pub backend_state: Option<BackendOrderState>,
}

/// Backend-neutral input for cancelling an in-flight order.
#[derive(Clone, Debug)]
pub struct CancelOrder {
    pub order_id: String,
    /// Verifier selector, used by the router to dispatch to the owning backend.
    pub selector: FixedBytes<4>,
    pub backend_state: Option<BackendOrderState>,
    /// Whether the broker believes a proof is currently in flight for this order.
    pub is_proving: bool,
}

/// Backend-opaque per-order durable state. The backend writes it; the broker persists it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendOrderState(pub serde_json::Value);

/// Durable progress returned while an order is being processed.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderProcessProgress {
    InProgress { state: BackendOrderState },
    Completed(ProcessedOrder),
}

/// How a finished proof reaches on-chain submission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubmissionPath {
    /// Needs set-builder aggregation before submission.
    Batched,
    /// Already a compressed seal; submit as-is.
    Direct,
}

/// Completed backend processing for one broker order.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedOrder {
    pub backend_id: BackendId,
    pub order_id: String,
    pub submission_path: SubmissionPath,
    /// Whether proving produced a compressed seal.
    pub compressed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimDigest(pub Digest);

impl ClaimDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }

    /// Build from a backend's native 32-byte claim digest. See [`Digest::from_native`].
    pub fn from_native<T: Into<[u8; 32]>>(value: T) -> Self {
        Self(Digest::from_native(value))
    }

    /// Convert into a backend's native 32-byte digest type.
    pub fn to_native<T: From<[u8; 32]>>(self) -> T {
        self.0.to_native()
    }
}

impl From<[u8; 32]> for ClaimDigest {
    fn from(value: [u8; 32]) -> Self {
        Self(Digest::from(value))
    }
}

impl From<Digest> for ClaimDigest {
    fn from(value: Digest) -> Self {
        Self(value)
    }
}

impl From<ClaimDigest> for [u8; 32] {
    fn from(value: ClaimDigest) -> Self {
        value.0.into()
    }
}

pub struct FulfillmentOrder {
    pub order_id: String,
    pub request: ProofRequest,
    pub program_id: ProgramId,
    /// Opaque backend state the backend authored; the broker stores and replays it without reading it.
    pub backend_state: Option<BackendOrderState>,
}

/// Per-order data the broker hands a backend. `backend_state` is the opaque blob the backend
/// authored; the rest is broker-owned order data.
#[derive(Clone, Debug)]
pub struct OrderProvingData {
    pub order_id: String,
    pub request: ProofRequest,
    pub client_sig: Bytes,
    pub image_id: Option<String>,
    pub backend_state: Option<BackendOrderState>,
}

pub struct FulfillmentBatch {
    pub backend_id: BackendId,
    pub state: Option<BackendBatchState>,
    pub eip712_domain: EIP712DomainSaltless,
    pub orders: Vec<FulfillmentOrder>,
}

/// Verifier-side work the broker must execute before or alongside fulfillment submission.
///
/// Today the only variant is submitting an RISC0 set-verifier root.
pub enum VerifierUpdate {
    SubmitMerkleRoot { verifier: Address, root: B256, seal: Bytes },
}

pub struct OrderFulfillmentArtifact {
    pub order_id: String,
    /// The full request being fulfilled. The broker derives the on-chain `SlimRequest` from this
    /// and keys order tracking on its id (the `Fulfillment` no longer carries the request id).
    pub request: ProofRequest,
    pub fulfillment: MarketFulfillment,
}

pub struct FailedFulfillmentOrder {
    pub order_id: String,
    pub error: anyhow::Error,
}

pub struct SubmissionPlan {
    pub verifier_updates: Vec<VerifierUpdate>,
    pub failed_orders: Vec<FailedFulfillmentOrder>,
    pub orders: Vec<OrderFulfillmentArtifact>,
    pub assessor: SubmissionAssessorArtifact,
}

#[derive(Clone, Debug)]
pub struct BatchSizeEstimateRequest {
    /// Current backend-owned batch state, if a batch has already been started.
    pub state: Option<BackendBatchState>,
    /// Orders already recorded in the broker batch.
    pub existing_orders: Vec<OrderProvingData>,
    /// Candidate orders that would be added before checking the size limit.
    pub pending_orders: Vec<OrderProvingData>,
}

#[derive(Clone, Debug)]
pub struct BatchSizeEstimate {
    pub size: usize,
}

/// Backend-opaque per-batch durable state. Same role as
/// [`BackendOrderState`] but scoped to a batch's aggregation work.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendBatchState(pub serde_json::Value);

#[derive(Clone, Debug)]
pub struct BatchOrder {
    pub proving: OrderProvingData,
    pub expiration: u64,
    pub fee: alloy::primitives::U256,
    pub fulfillment_type: FulfillmentType,
    pub request_id: alloy::primitives::U256,
    pub lock_expiration: u64,
}

pub struct UpdateBatch {
    pub batch_id: usize,
    pub existing_orders: Vec<OrderProvingData>,
    pub state: Option<BackendBatchState>,
    pub new_orders: Vec<BatchOrder>,
    pub finalize: bool,
}

pub struct BatchUpdate {
    pub state: BackendBatchState,
    /// Echoes the requested `finalize`; the broker flips the batch to `PendingCompression` on it.
    pub finalize: bool,
    pub batch_update_secs: Option<f64>,
    pub assessor_secs: Option<f64>,
}

pub struct CloseBatch {
    pub batch_id: usize,
    pub order_ids: Vec<String>,
    /// The batch's opaque backend state.
    pub state: Option<BackendBatchState>,
}

pub struct BatchClose {
    pub state: BackendBatchState,
    pub compression_secs: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("backend operation failed: {0:#}")]
    Operation(#[source] anyhow::Error),
}

impl BackendError {
    pub fn operation(err: impl Into<anyhow::Error>) -> Self {
        Self::Operation(err.into())
    }
}

impl From<ProverError> for BackendError {
    fn from(err: ProverError) -> Self {
        Self::operation(err)
    }
}

/// Failure from applying a [`VerifierUpdate`].
#[derive(Debug, thiserror::Error)]
pub enum VerifierUpdateError {
    /// The verifier-update transaction was broadcast but its confirmation could not be
    /// observed (e.g. it timed out waiting for inclusion). Transient; safe to retry.
    #[error("verifier update transaction confirmation failed: {0:#}")]
    TxnConfirmation(#[source] anyhow::Error),
    /// Any other failure while applying the verifier update.
    #[error("verifier update failed: {0:#}")]
    Other(#[source] anyhow::Error),
}

pub struct SubmissionAssessorArtifact {
    pub seal: Bytes,
    pub selectors: Vec<AssessorSelector>,
    pub callbacks: Vec<AssessorCallback>,
}

#[async_trait]
pub trait BatchProcessor: Send + Sync {
    async fn estimate_batch_size(&self, cmd: BatchSizeEstimateRequest)
        -> Result<BatchSizeEstimate>;

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate>;

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, BackendError>;
}

pub type BatchProcessorObj = Arc<dyn BatchProcessor>;

#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> &BackendId;

    fn supported_selectors(&self) -> Vec<FixedBytes<4>>;

    fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType>;

    /// The assessor grouping key for an order with this signed verifier selector: orders that
    /// return the same key may share a batch, since a batch carries a single assessor seal and
    /// must therefore be single assessor class. `Ok(None)` means the backend does not distinguish
    /// assessor classes, so all of its orders may batch together. `Err` means the backend groups
    /// but cannot resolve this order (e.g. no supported assessor is registered for its verifier
    /// class) — such an order can never be sealed and must not be batched.
    fn assessor_group(&self, selector: FixedBytes<4>) -> Result<Option<FixedBytes<4>>>;

    /// Estimated on-chain gas to fulfill `request` via the path and assessor this backend would
    /// actually use, resolved from its router policy. Excludes journal calldata (added separately,
    /// selector-agnostically, during pricing) and lock gas. Required: every backend models its own
    /// cost — there is no safe default.
    fn fulfillment_gas(&self, request: &ProofRequest) -> Result<u64>;

    /// Extra proving cycles beyond the order's own guest cycles for the path this backend would
    /// take to fulfill `request` (direct groth16 → its groth16 wrap only; batched → assessor STARK
    /// + set builder). Required: every backend models its own cost — there is no safe default.
    fn additional_proof_cycles(&self, request: &ProofRequest) -> Result<u64>;

    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError>;

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;

    async fn cancel_order(&self, cmd: CancelOrder) -> Result<()>;

    /// `None` if this backend does not batch its proofs.
    fn batch_processor(&self) -> Option<BatchProcessorObj>;

    async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan>;

    /// Returns whether the verifier already reflects `update` (idempotency check).
    async fn verifier_update_applied(&self, update: &VerifierUpdate) -> Result<bool>;

    /// Applies `update` to the verifier as a standalone transaction.
    async fn apply_verifier_update(
        &self,
        update: &VerifierUpdate,
    ) -> Result<(), VerifierUpdateError>;
}

pub type BackendObj = Arc<dyn Backend>;

#[derive(Clone)]
pub struct BackendEntry {
    pub(crate) id: BackendId,
    pub(crate) selectors: Vec<FixedBytes<4>>,
    pub(crate) backend: BackendObj,
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
