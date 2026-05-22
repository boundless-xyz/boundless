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

use std::{fmt, str::FromStr, sync::Arc};

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
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.trim().is_empty() {
                    anyhow::bail!(concat!(stringify!($name), " cannot be empty"));
                }
                Ok(Self(value))
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

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <String as serde::Deserialize>::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = anyhow::Error;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl TryFrom<&str> for $name {
            type Error = anyhow::Error;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::new(value)
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

pub struct OrderFulfillmentFailure {
    pub order_id: String,
    pub error: anyhow::Error,
}

pub enum OrderFulfillmentResult {
    Fulfilled(OrderFulfillmentArtifact),
    Failed(OrderFulfillmentFailure),
}

pub struct SubmissionPlan {
    pub verifier_updates: Vec<VerifierUpdate>,
    pub orders: Vec<OrderFulfillmentResult>,
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

    /// Returns the batch processor for this backend, or `None` if the backend
    /// does not support batching.
    fn batch_processor(&self) -> Option<BatchProcessorObj>;

    async fn build_fulfillments(&self, cmd: FulfillmentBatch) -> Result<SubmissionPlan>;

    /// Returns whether the verifier already reflects `update` (idempotency check).
    async fn verifier_update_applied(&self, update: &VerifierUpdate) -> Result<bool>;

    /// Applies `update` to the verifier as a standalone transaction.
    async fn apply_verifier_update(&self, update: &VerifierUpdate) -> Result<()>;
}

pub(crate) type BackendObj = Arc<dyn Backend>;
