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
use boundless_market::contracts::{
    AssessorCallback, AssessorReceipt, AssessorSelector, EIP712DomainSaltless,
    Fulfillment as MarketFulfillment, ProofRequest,
};
use risc0_zkvm::sha::Digest;

use crate::{db::AggregationOrder, provers, AggregationState, Order, OrderStatus};
use anyhow::Result;

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
