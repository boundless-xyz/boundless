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

//! Per-selector backend post-processing: claim-digest formulas, STARK→SNARK
//! compression, on-chain seal encoding. Pairs with `Prover` (the operational
//! job runner): `Prover` produces primitive proofs; `BackendProvider` makes
//! them satisfy a Boundless request on-chain.
//!
//! - `compress_proof` takes a primitive STARK id + selector and produces
//!   the SNARK id required by the selector (e.g. Groth16, Blake3-Groth16).
//! - `encode_seal` packs the compressed proof into the on-chain seal with
//!   the 4-byte selector prefix.
//! - `compute_claim_digest` binds `(program_id, public_output)` to the
//!   32-byte commitment the on-chain verifier checks.

use std::sync::Arc;

use async_trait::async_trait;

use crate::{ProgramId, PublicOutput};

/// Errors returned from [`BackendProvider`] operations.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BackendProviderError {
    /// The provider does not advertise the requested selector.
    #[error("unsupported selector: {0:02x?}")]
    UnsupportedSelector(crate::VerifierSelector),

    /// Selector and produced artifact disagree.
    #[error("selector / artifact mismatch")]
    SelectorMismatch,

    /// Catch-all for backend-specific failures.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Trait implemented by each zkVM backend to turn primitive proofs into
/// on-chain seal bytes for the Boundless market verifier.
///
/// All async methods MUST be drop-cancel safe. If the caller drops the
/// future, the provider should clean up any in-flight remote-prover sessions.
#[async_trait]
pub trait BackendProvider: Send + Sync {
    /// Compress a primitive STARK proof into the SNARK form the selector
    /// requires for settlement.
    ///
    /// Returns [`BackendProviderError::UnsupportedSelector`] for selectors
    /// that need no compression (e.g. set-inclusion / pass-through).
    async fn compress_proof(
        &self,
        program_id: &ProgramId,
        primitive_proof_id: &str,
        selector: crate::VerifierSelector,
    ) -> Result<String, BackendProviderError>;

    /// Encode the on-chain seal bytes for a compressed proof.
    ///
    /// The first 4 bytes of the returned seal SHOULD equal `selector`. A
    /// backend may substitute an equivalent development/testing selector
    /// when explicitly configured to do so; production paths must preserve
    /// the requested selector.
    async fn encode_seal(
        &self,
        compressed_proof_id: &str,
        selector: crate::VerifierSelector,
    ) -> Result<Vec<u8>, BackendProviderError>;

    /// Compute the 32-byte claim digest binding `(program_id, public_output)`
    /// under the given selector. Different selectors under the same
    /// backend may use different claim-digest formulas.
    fn compute_claim_digest(
        &self,
        selector: crate::VerifierSelector,
        program_id: &ProgramId,
        public_output: &PublicOutput,
    ) -> Result<[u8; 32], BackendProviderError>;
}

/// `Arc`-wrapped trait object alias.
pub type BackendProviderObj = Arc<dyn BackendProvider>;
