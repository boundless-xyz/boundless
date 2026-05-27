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

use crate::{provers, FulfillmentType, Order, OrderStatus};
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

            pub fn into_string(self) -> String {
                self.0
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
    "Stable broker-side identity for a backend registration. This is distinct from a verifier selector: many selectors may route to the same backend, and broker policy/batching is keyed by backend."
);
string_id!(ProofId, "Durable backend proof handle.");
string_id!(CompressedProofId, "Durable backend compressed proof handle.");
string_id!(AssessorProofId, "Durable backend assessor proof handle.");

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
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

/// Command for processing one accepted broker order.
#[derive(Clone, Debug)]
pub struct ProcessOrder {
    pub order: Order,
}

/// Durable progress returned while an order is being processed.
#[derive(Clone, Debug, PartialEq)]
pub enum OrderProcessProgress {
    Started { proof_id: ProofId },
    Compressed { proof_id: ProofId, compressed_proof_id: CompressedProofId },
    Completed(ProcessedOrder),
}

/// Completed backend processing for one broker order.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessedOrder {
    pub backend_id: BackendId,
    pub order_id: String,
    pub proof_id: ProofId,
    pub compressed_proof_id: Option<CompressedProofId>,
    pub next_status: OrderStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimDigest(pub Digest);

impl ClaimDigest {
    pub fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
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

/// MVP shape; mirrors `BoundlessMarket.sol::fulfill`. Verifier-router or joint
/// verifier additions go here or as a new [`VerifierUpdate`] variant.
pub struct AssessorArtifact {
    pub claim_digest: ClaimDigest,
    pub selectors: Vec<AssessorSelector>,
    pub callbacks: Vec<AssessorCallback>,
}

pub struct FulfillmentOrder {
    pub order_id: String,
    pub request: ProofRequest,
    pub proof_id: ProofId,
    pub compressed_proof_id: Option<CompressedProofId>,
    pub program_id: ProgramId,
}

pub struct FulfillmentBatch {
    pub backend_id: BackendId,
    pub state: Option<BackendBatchState>,
    pub assessor_proof_id: Option<AssessorProofId>,
    pub eip712_domain: EIP712DomainSaltless,
    pub orders: Vec<FulfillmentOrder>,
}

/// Verifier-side work that the broker must execute before or alongside fulfillment submission.
///
/// This is intentionally shaped around the current market contracts. Backends produce the
/// verifier artifacts, while the broker owns transaction orchestration. More general verifier
/// calls belong in the future verifier-router contract adapter rather than this MVP enum. For
/// example, joint-verifier or on-chain-assessor flows should become additional backend-produced
/// submission artifacts once the contract interface supports them; today the only required
/// verifier-side preparation is submitting an RISC0 set-verifier root.
pub enum VerifierUpdate {
    SubmitMerkleRoot { verifier: Address, root: B256, seal: Bytes },
}

pub struct OrderFulfillmentArtifact {
    pub order_id: String,
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
    pub existing_order_ids: Vec<String>,
    /// Candidate orders that would be added before checking the size limit.
    pub pending_order_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct BatchSizeEstimate {
    pub size: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackendBatchState {
    pub data: serde_json::Value,
    pub proof_id: Option<ProofId>,
    pub compressed_proof_id: Option<CompressedProofId>,
}

#[derive(Clone, Debug)]
pub struct BatchOrder {
    pub order_id: String,
    pub proof_id: ProofId,
    pub compressed_proof_id: Option<CompressedProofId>,
    pub expiration: u64,
    pub fee: alloy::primitives::U256,
    pub fulfillment_type: FulfillmentType,
    pub request_id: alloy::primitives::U256,
    pub lock_expiration: u64,
}

pub struct UpdateBatch {
    pub batch_id: usize,
    pub existing_order_ids: Vec<String>,
    pub state: Option<BackendBatchState>,
    pub new_orders: Vec<BatchOrder>,
    pub finalize: bool,
}

pub struct BatchUpdate {
    pub state: BackendBatchState,
    pub assessor_proof_id: Option<AssessorProofId>,
    pub batch_update_secs: Option<f64>,
    pub assessor_secs: Option<f64>,
}

pub struct CloseBatch {
    pub batch_id: usize,
    pub proof_id: ProofId,
    pub order_ids: Vec<String>,
}

pub struct BatchClose {
    pub compressed_proof_id: CompressedProofId,
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

impl From<provers::ProverError> for BackendError {
    fn from(err: provers::ProverError) -> Self {
        Self::operation(err)
    }
}

/// Failure from applying a [`VerifierUpdate`].
///
/// The variants carry enough structure for the submitter to classify the failure for its
/// retry/alarm policy without string-matching an error message — a backend that adds context
/// around the underlying error cannot silently break that classification.
#[derive(Debug, thiserror::Error)]
pub enum VerifierUpdateError {
    /// The verifier-update transaction was broadcast but its confirmation could not be
    /// observed (e.g. it timed out waiting for inclusion). Transient — safe to retry.
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
pub(crate) trait BatchProcessor: Send + Sync {
    async fn estimate_batch_size(&self, cmd: BatchSizeEstimateRequest)
        -> Result<BatchSizeEstimate>;

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate>;

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, BackendError>;
}

pub(crate) type BatchProcessorObj = Arc<dyn BatchProcessor>;

#[async_trait]
pub trait Backend: Send + Sync {
    fn id(&self) -> &BackendId;

    fn supported_selectors(&self) -> Vec<FixedBytes<4>>;

    fn proof_type(&self, selector: FixedBytes<4>) -> Option<ProofType>;

    async fn evaluate_request(
        &self,
        request: EvaluationRequest,
        limits: EvaluationLimits,
    ) -> Result<RequestEvaluation, OrderPricingError>;

    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;

    async fn cancel_order(&self, order: &Order) -> Result<()>;

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

pub(crate) type BackendObj = Arc<dyn Backend>;

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
