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

/// Deterministic [`Aggregator`] for tests that exercise broker plumbing
/// without paying the cost of real proving. Outputs are stable hashes of
/// the inputs and are NOT valid aggregation receipts; on-chain submission
/// using these would revert.
///
/// The mock also acts as a sanity check on the [`Aggregator`] trait
/// shape: a non-R0 implementation that compiles and round-trips through
/// every method shows the trait does not over-fit to R0.
#[cfg(feature = "test-utils")]
pub mod mock {
    use super::{
        async_trait, AggregationState, Aggregator, AggregatorError, ProgramId, PublicOutput,
    };
    use sha2::{Digest as _, Sha256};

    /// Mock aggregator: deterministic, R0-free, suitable for unit tests.
    #[derive(Debug, Default, Clone)]
    pub struct MockAggregator;

    impl MockAggregator {
        /// Construct a new [`MockAggregator`].
        pub fn new() -> Self {
            Self
        }
    }

    fn hash_bytes(parts: &[&[u8]]) -> [u8; 32] {
        let mut h = Sha256::new();
        for p in parts {
            h.update((p.len() as u64).to_le_bytes());
            h.update(p);
        }
        h.finalize().into()
    }

    fn fold_state(prev: Option<&AggregationState>, proof_ids: &[String]) -> AggregationState {
        let prev_state = prev.map(|s| s.state.as_slice()).unwrap_or(&[]);
        let mut h = Sha256::new();
        h.update(prev_state);
        for id in proof_ids {
            h.update((id.len() as u64).to_le_bytes());
            h.update(id.as_bytes());
        }
        let digest: [u8; 32] = h.finalize().into();
        AggregationState {
            state: digest.to_vec(),
            proof_id: format!("mock-agg-{}", hex::encode(digest)),
            compressed_proof_id: None,
        }
    }

    #[async_trait]
    impl Aggregator for MockAggregator {
        fn name(&self) -> &str {
            "mock"
        }

        async fn update(
            &self,
            state: Option<&AggregationState>,
            proof_ids: &[String],
        ) -> Result<AggregationState, AggregatorError> {
            Ok(fold_state(state, proof_ids))
        }

        async fn finalize(
            &self,
            state: Option<&AggregationState>,
            proof_ids: &[String],
        ) -> Result<AggregationState, AggregatorError> {
            Ok(fold_state(state, proof_ids))
        }

        async fn compress(&self, state: &AggregationState) -> Result<String, AggregatorError> {
            Ok(format!("mock-compressed-{}", state.proof_id))
        }

        async fn encode_compressed_seal(
            &self,
            state: &AggregationState,
        ) -> Result<Vec<u8>, AggregatorError> {
            Ok(hash_bytes(&[b"mock-seal", &state.state]).to_vec())
        }

        async fn root(&self, state: &AggregationState) -> Result<[u8; 32], AggregatorError> {
            Ok(hash_bytes(&[b"mock-root", &state.state]))
        }

        async fn inclusion_proof(
            &self,
            state: &AggregationState,
            program_id: &ProgramId,
            public_output: &PublicOutput,
            claim_digest: &[u8; 32],
        ) -> Result<Vec<u8>, AggregatorError> {
            Ok(hash_bytes(&[
                b"mock-inclusion",
                &state.state,
                program_id.as_bytes(),
                public_output.as_bytes(),
                claim_digest,
            ])
            .to_vec())
        }
    }
}

#[cfg(all(test, feature = "test-utils"))]
mod tests {
    use super::mock::MockAggregator;
    use super::{Aggregator, AggregatorObj, ProgramId, PublicOutput};
    use std::sync::Arc;

    /// Round-trips every [`Aggregator`] method through the mock to confirm
    /// the trait shape is satisfiable without R0 dependencies.
    #[tokio::test]
    async fn mock_round_trips_every_method() {
        let agg: AggregatorObj = Arc::new(MockAggregator::new());
        assert_eq!(agg.name(), "mock");

        let proofs = vec!["proof-a".to_string(), "proof-b".to_string()];
        let s1 = agg.update(None, &proofs).await.unwrap();
        let s2 = agg.update(Some(&s1), &proofs).await.unwrap();
        assert_ne!(s1.state, s2.state);

        let final_state = agg.finalize(Some(&s2), &proofs).await.unwrap();

        let compressed = agg.compress(&final_state).await.unwrap();
        assert!(compressed.starts_with("mock-compressed-"));

        let seal = agg.encode_compressed_seal(&final_state).await.unwrap();
        assert_eq!(seal.len(), 32);

        let root = agg.root(&final_state).await.unwrap();
        let program_id = ProgramId::from([0u8; 32]);
        let output = PublicOutput::from(vec![1u8, 2, 3]);
        let proof = agg.inclusion_proof(&final_state, &program_id, &output, &root).await.unwrap();
        assert_eq!(proof.len(), 32);
    }

    /// Same inputs produce the same outputs; the mock is deterministic.
    #[tokio::test]
    async fn mock_is_deterministic() {
        let agg = MockAggregator::new();
        let proofs = vec!["x".to_string()];
        let a = agg.update(None, &proofs).await.unwrap();
        let b = agg.update(None, &proofs).await.unwrap();
        assert_eq!(a.state, b.state);
        assert_eq!(a.proof_id, b.proof_id);
    }
}
