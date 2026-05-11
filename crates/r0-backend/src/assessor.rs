// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! RISC Zero implementation of the [`Assessor`] trait.

use alloy_primitives::Address;
use anyhow::Context as _;
use async_trait::async_trait;
use boundless_assessor::{Assessor, AssessorError, AssessorInput, Fulfillment};
use boundless_market::{
    contracts::EIP712DomainSaltless, input::GuestEnv, prover_utils::prover::ProverObj,
};
use risc0_zkvm::sha::Digest;

/// R0 implementation of [`Assessor`]. Holds a broker `Prover` (Bonsai/Bento)
/// and the assessor guest's image id; encodes the assessor input as an R0
/// `GuestEnv` frame internally.
pub struct R0Assessor {
    prover: ProverObj,
    assessor_guest_id: Digest,
}

impl R0Assessor {
    /// Construct a new [`R0Assessor`] over `prover` and `assessor_guest_id`.
    pub fn new(prover: ProverObj, assessor_guest_id: Digest) -> Self {
        Self { prover, assessor_guest_id }
    }
}

#[async_trait]
impl Assessor for R0Assessor {
    fn name(&self) -> &str {
        "risc0"
    }

    fn program_id(&self) -> [u8; 32] {
        self.assessor_guest_id.as_bytes().try_into().expect("R0 Digest is 32 bytes")
    }

    async fn prove(
        &self,
        fills: &[Fulfillment],
        domain: EIP712DomainSaltless,
        prover_address: Address,
    ) -> Result<String, AssessorError> {
        let input = AssessorInput { fills: fills.to_vec(), domain, prover_address };
        let stdin = GuestEnv::builder().write_frame(&input.encode()).stdin;

        let input_id = self
            .prover
            .upload_input(stdin)
            .await
            .context("Failed to upload assessor input")
            .map_err(AssessorError::Other)?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark(&self.assessor_guest_id.to_string(), &input_id, vec![])
            .await
            .map_err(|e| AssessorError::ProvingFailed(format!("{e}")))?;

        tracing::debug!(
            "Assessor proof completed, proof id: {} fills: {} cycles: {:?} time: {}",
            proof_res.id,
            fills.len(),
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time
        );

        Ok(proof_res.id)
    }

    async fn get_journal(&self, proof_id: &str) -> Result<Vec<u8>, AssessorError> {
        let journal = self
            .prover
            .get_journal(proof_id)
            .await
            .map_err(|e| AssessorError::JournalFetch(format!("{e}")))?
            .ok_or_else(|| {
                AssessorError::JournalFetch(format!("journal missing for {proof_id}"))
            })?;
        Ok(journal)
    }
}
