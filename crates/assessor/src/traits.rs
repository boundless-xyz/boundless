// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Pluggable [`Assessor`] trait for the broker's per-batch assessor receipt.
//!
//! The trait owns the input encoding and the proving step. The broker hands
//! the trait the structural [`Fulfillment`] records plus EIP-712 domain and
//! prover address; it gets back an opaque proof id, then later fetches the
//! assessor journal bytes through the same trait.
//!
//! Today's only impl is the R0 assessor in `boundless-r0-backend`. The
//! on-chain side stays R0-shaped (the assessor receipt still flows through
//! `RiscZeroVerifierRouter`); a separate `IBoundlessAssessor` interface is
//! the contract-side follow-up handled by the BoundlessRouter epic.

use std::sync::Arc;

use alloy_primitives::Address;
use async_trait::async_trait;
use boundless_market::contracts::EIP712DomainSaltless;
use thiserror::Error;

use crate::Fulfillment;

/// Errors returned from [`Assessor`] operations.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum AssessorError {
    /// Input encoding failed.
    #[error("invalid assessor input: {0}")]
    InvalidInput(String),
    /// Proof generation failed.
    #[error("proof generation failed: {0}")]
    ProvingFailed(String),
    /// Journal fetch failed.
    #[error("journal fetch failed: {0}")]
    JournalFetch(String),
    /// Other backend failure.
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Pluggable assessor used by the broker to prove a batch's assessor receipt.
#[async_trait]
pub trait Assessor: Send + Sync {
    /// Display name used in logs.
    fn name(&self) -> &str;

    /// Program identity of the assessor guest, in the byte form the on-chain
    /// verifier expects. The broker uses this when computing the assessor
    /// claim digest.
    fn program_id(&self) -> [u8; 32];

    /// Build the assessor input from `fills` + `domain` + `prover_address`,
    /// upload it, and prove the assessor STARK. Returns an opaque proof id
    /// the broker stores on the batch for later journal retrieval.
    async fn prove(
        &self,
        fills: &[Fulfillment],
        domain: EIP712DomainSaltless,
        prover_address: Address,
    ) -> Result<String, AssessorError>;

    /// Fetch the assessor journal bytes for `proof_id`. The broker
    /// ABI-decodes these into an `AssessorJournal` at submission time.
    async fn get_journal(&self, proof_id: &str) -> Result<Vec<u8>, AssessorError>;
}

/// `Arc`-wrapped trait object alias.
pub type AssessorObj = Arc<dyn Assessor>;

/// Deterministic [`Assessor`] for tests that exercise broker plumbing
/// without paying the cost of real proving. Outputs are stable hashes of
/// the inputs and are NOT valid assessor receipts; on-chain submission
/// using these would revert.
///
/// The mock also acts as a sanity check on the [`Assessor`] trait shape:
/// a non-R0 implementation that compiles and round-trips through every
/// method shows the trait does not over-fit to R0.
#[cfg(feature = "test-utils")]
pub mod mock {
    use super::{async_trait, Address, Assessor, AssessorError, EIP712DomainSaltless, Fulfillment};
    use sha2::{Digest as _, Sha256};
    use std::sync::Mutex;

    /// Mock assessor: deterministic, R0-free, suitable for unit tests.
    #[derive(Debug, Default)]
    pub struct MockAssessor {
        program_id: [u8; 32],
        journals: Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl MockAssessor {
        /// Construct a new [`MockAssessor`] with the given program id.
        pub fn new(program_id: [u8; 32]) -> Self {
            Self { program_id, journals: Mutex::new(std::collections::HashMap::new()) }
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

    #[async_trait]
    impl Assessor for MockAssessor {
        fn name(&self) -> &str {
            "mock"
        }

        fn program_id(&self) -> [u8; 32] {
            self.program_id
        }

        async fn prove(
            &self,
            fills: &[Fulfillment],
            _domain: EIP712DomainSaltless,
            prover_address: Address,
        ) -> Result<String, AssessorError> {
            let mut h = Sha256::new();
            h.update((fills.len() as u64).to_le_bytes());
            for f in fills {
                h.update(f.signature.as_slice());
            }
            h.update(prover_address.as_slice());
            let digest: [u8; 32] = h.finalize().into();
            let proof_id = format!("mock-assessor-{}", hex::encode(digest));

            let journal = hash_bytes(&[b"mock-assessor-journal", &digest]).to_vec();
            self.journals.lock().unwrap().insert(proof_id.clone(), journal);
            Ok(proof_id)
        }

        async fn get_journal(&self, proof_id: &str) -> Result<Vec<u8>, AssessorError> {
            self.journals
                .lock()
                .unwrap()
                .get(proof_id)
                .cloned()
                .ok_or_else(|| AssessorError::JournalFetch(format!("unknown proof_id {proof_id}")))
        }
    }
}

#[cfg(all(test, feature = "test-utils"))]
mod tests {
    use super::mock::MockAssessor;
    use super::{Assessor, AssessorObj};
    use alloy_primitives::Address;
    use boundless_market::contracts::eip712_domain;
    use std::sync::Arc;

    /// Round-trips every [`Assessor`] method through the mock to confirm
    /// the trait shape is satisfiable without R0 dependencies.
    #[tokio::test]
    async fn mock_round_trips_every_method() {
        let program_id = [0x11u8; 32];
        let assessor: AssessorObj = Arc::new(MockAssessor::new(program_id));
        assert_eq!(assessor.name(), "mock");
        assert_eq!(assessor.program_id(), program_id);

        let domain = eip712_domain(Address::ZERO, 1);
        let proof_id = assessor.prove(&[], domain, Address::ZERO).await.unwrap();
        assert!(proof_id.starts_with("mock-assessor-"));

        let journal = assessor.get_journal(&proof_id).await.unwrap();
        assert_eq!(journal.len(), 32);
    }

    /// Same inputs produce the same proof id.
    #[tokio::test]
    async fn mock_is_deterministic() {
        let assessor = MockAssessor::new([0u8; 32]);
        let domain = eip712_domain(Address::ZERO, 1);
        let a = assessor.prove(&[], domain.clone(), Address::ZERO).await.unwrap();
        let b = assessor.prove(&[], domain, Address::ZERO).await.unwrap();
        assert_eq!(a, b);
    }

    /// `get_journal` errors on an unknown id.
    #[tokio::test]
    async fn mock_get_journal_unknown_id_errors() {
        let assessor = MockAssessor::new([0u8; 32]);
        let result = assessor.get_journal("does-not-exist").await;
        assert!(result.is_err());
    }
}
