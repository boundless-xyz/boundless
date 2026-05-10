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

//! Aggregator trait for batched proof aggregation.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::program::{ProgramId, PublicOutput};

/// Persistent state the broker stores between aggregation rounds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AggregationState {
    /// State owned by the [`Aggregator`] implementation. Format is
    /// impl-defined.
    pub state: Vec<u8>,
    /// Proof id for the proof attesting to the current root.
    pub proof_id: String,
    /// Proof id for the compressed form of the root, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressed_proof_id: Option<String>,
}

/// Errors returned from [`Aggregator`] operations.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AggregatorError {
    /// State bytes were not in the expected format.
    #[error("invalid aggregator state: {0}")]
    InvalidState(String),
    /// Proof generation failed.
    #[error("proof generation failed: {0}")]
    ProvingFailed(String),
    /// Other backend failure.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Aggregator over per-order proofs.
#[async_trait]
pub trait Aggregator: Send + Sync {
    /// Display name used in logs.
    fn name(&self) -> &str;

    /// Fold `proof_ids` into the aggregation.
    ///
    /// `state` is `None` for the first round of a batch and the previous
    /// round's [`AggregationState`] thereafter.
    async fn update(
        &self,
        state: Option<&AggregationState>,
        proof_ids: &[String],
    ) -> Result<AggregationState, AggregatorError>;

    /// Fold `proof_ids` into the aggregation and close it. The returned
    /// state is ready for on-chain submission.
    async fn finalize(
        &self,
        state: Option<&AggregationState>,
        proof_ids: &[String],
    ) -> Result<AggregationState, AggregatorError>;

    /// Compress (wrap) the finalized aggregation root and return the
    /// compressed proof id.
    async fn compress(&self, state: &AggregationState) -> Result<String, AggregatorError>;

    /// Encode the compressed-proof seal for on-chain submission.
    async fn encode_compressed_seal(
        &self,
        state: &AggregationState,
    ) -> Result<Vec<u8>, AggregatorError>;

    /// Merkle root of the finalized aggregation state, in the byte form
    /// the on-chain set verifier expects.
    async fn root(&self, state: &AggregationState) -> Result<[u8; 32], AggregatorError>;

    /// Inclusion proof for `claim_digest` against the aggregation state,
    /// in the byte form the on-chain verifier expects (encoded calldata).
    async fn inclusion_proof(
        &self,
        state: &AggregationState,
        program_id: &ProgramId,
        public_output: &PublicOutput,
        claim_digest: &[u8; 32],
    ) -> Result<Vec<u8>, AggregatorError>;
}

/// `Arc`-wrapped trait object alias.
pub type AggregatorObj = Arc<dyn Aggregator>;
