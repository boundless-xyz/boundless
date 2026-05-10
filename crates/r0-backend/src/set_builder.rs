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

//! RISC Zero set-builder implementation of the [`Aggregator`] trait.

use alloy_primitives::FixedBytes;
use anyhow::{anyhow, Context as _};
use async_trait::async_trait;
use boundless_market::{
    aggregator::{AggregationState, Aggregator, AggregatorError},
    backend_provider::BackendProviderObj,
    program::{ProgramId, PublicOutput},
    prover_utils::prover::ProverObj,
    selector::SelectorExt,
};
use risc0_aggregation::{
    merkle_path, merkle_root, GuestState, SetInclusionReceipt,
    SetInclusionReceiptVerifierParameters,
};
use risc0_zkvm::{
    sha::{Digest, Digestible, Impl as ShaImpl, Sha256},
    MaybePruned, ReceiptClaim,
};
use serde::{Deserialize, Serialize};

use crate::input::GuestEnvBuilderExt;

/// State the R0 set-builder aggregator persists between rounds.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct PersistedState {
    pub(crate) guest_state: GuestState,
    pub(crate) claim_digests: Vec<Digest>,
}

impl PersistedState {
    pub(crate) fn encode(&self) -> Vec<u8> {
        bincode::serialize(self).expect("PersistedState bincode serialization is infallible")
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, AggregatorError> {
        bincode::deserialize(bytes)
            .map_err(|e| AggregatorError::InvalidState(format!("bincode decode: {e}")))
    }
}

/// RISC Zero set-builder [`Aggregator`].
pub struct R0SetBuilderAggregator {
    prover: ProverObj,
    set_builder_img_id: Digest,
    provider: BackendProviderObj,
}

impl R0SetBuilderAggregator {
    /// Construct a new R0 set-builder aggregator.
    pub fn new(
        prover: ProverObj,
        set_builder_img_id: Digest,
        provider: BackendProviderObj,
    ) -> Self {
        Self { prover, set_builder_img_id, provider }
    }

    async fn validate_and_extract_claim(
        &self,
        proof_id: &str,
    ) -> Result<ReceiptClaim, AggregatorError> {
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

    async fn aggregate_inner(
        &self,
        state: Option<&AggregationState>,
        proof_ids: &[String],
        finalize: bool,
    ) -> Result<AggregationState, AggregatorError> {
        let prior = state.map(|s| PersistedState::decode(&s.state)).transpose()?;

        let mut claims = Vec::<ReceiptClaim>::with_capacity(proof_ids.len());
        let mut valid_proof_ids = Vec::<String>::with_capacity(proof_ids.len());
        for proof_id in proof_ids {
            match self.validate_and_extract_claim(proof_id).await {
                Ok(claim) => {
                    claims.push(claim);
                    valid_proof_ids.push(proof_id.clone());
                }
                Err(e) => {
                    tracing::error!("Excluding invalid proof {proof_id} from aggregation: {e:?}");
                }
            }
        }

        if claims.is_empty() {
            return Err(AggregatorError::ProvingFailed(
                "no valid proofs found for aggregation".into(),
            ));
        }

        let input = prior
            .as_ref()
            .map_or(GuestState::initial(self.set_builder_img_id), |s| s.guest_state.clone())
            .into_input(claims.clone(), finalize)
            .map_err(|e| anyhow!("Failed to build set builder input: {e}"))?;

        let assumption_ids: Vec<String> = state
            .map(|s| s.proof_id.clone())
            .into_iter()
            .chain(valid_proof_ids.iter().cloned())
            .collect();

        let input_data = boundless_market::input::GuestEnv::builder()
            .write(&input)
            .map_err(|e| anyhow!("Failed to encode set-builder input: {e}"))?
            .stdin;
        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .map_err(|e| anyhow!("Failed to upload set-builder input: {e}"))?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark(
                &self.set_builder_img_id.to_string(),
                &input_id,
                assumption_ids,
            )
            .await
            .map_err(|e| {
                AggregatorError::ProvingFailed(format!("set-builder proving failed: {e}"))
            })?;

        let receipt = self
            .prover
            .get_receipt(&proof_res.id)
            .await
            .map_err(|e| anyhow!("Failed to get set-builder receipt: {e}"))?
            .ok_or_else(|| anyhow!("Set-builder receipt missing for {}", proof_res.id))?;
        receipt.verify(self.set_builder_img_id).map_err(|e| {
            AggregatorError::ProvingFailed(format!("set-builder receipt verify failed: {e}"))
        })?;

        let journal = receipt.journal.bytes;
        let guest_state = GuestState::decode(&journal)
            .map_err(|e| anyhow!("Failed to decode set-builder journal: {e}"))?;
        let claim_digests: Vec<Digest> = prior
            .map(|s| s.claim_digests)
            .unwrap_or_default()
            .into_iter()
            .chain(claims.into_iter().map(|claim| claim.digest()))
            .collect();

        let new_state = PersistedState { guest_state, claim_digests };

        Ok(AggregationState {
            state: new_state.encode(),
            proof_id: proof_res.id,
            compressed_proof_id: None,
        })
    }
}

#[async_trait]
impl Aggregator for R0SetBuilderAggregator {
    fn name(&self) -> &str {
        "risc0-set-builder"
    }

    async fn update(
        &self,
        state: Option<&AggregationState>,
        proof_ids: &[String],
    ) -> Result<AggregationState, AggregatorError> {
        self.aggregate_inner(state, proof_ids, false).await
    }

    async fn finalize(
        &self,
        state: Option<&AggregationState>,
        proof_ids: &[String],
    ) -> Result<AggregationState, AggregatorError> {
        self.aggregate_inner(state, proof_ids, true).await
    }

    async fn root(&self, state: &AggregationState) -> Result<[u8; 32], AggregatorError> {
        let p = PersistedState::decode(&state.state)?;
        if p.claim_digests.is_empty() {
            return Err(AggregatorError::InvalidState("no claim digests".into()));
        }
        if !p.guest_state.mmr.is_finalized() {
            return Err(AggregatorError::InvalidState("MMR not finalized".into()));
        }
        let root = merkle_root(&p.claim_digests);
        let finalized_root = p
            .guest_state
            .mmr
            .clone()
            .finalized_root()
            .ok_or_else(|| AggregatorError::InvalidState("missing finalized root".into()))?;
        if finalized_root != root {
            return Err(AggregatorError::InvalidState(
                "guest state finalized root inconsistent with claim digests".into(),
            ));
        }
        Ok(root.into())
    }

    async fn inclusion_proof(
        &self,
        state: &AggregationState,
        program_id: &ProgramId,
        public_output: &PublicOutput,
        claim_digest: &[u8; 32],
    ) -> Result<Vec<u8>, AggregatorError> {
        let p = PersistedState::decode(&state.state)?;
        let claim = Digest::from_bytes(*claim_digest);
        let idx = p.claim_digests.iter().position(|c| *c == claim).ok_or_else(|| {
            AggregatorError::InvalidState(format!("claim {claim:x?} not in batch"))
        })?;
        let path = merkle_path(&p.claim_digests, idx);
        let img = Digest::from_bytes(*program_id.as_bytes());
        let public_output_digest = *ShaImpl::hash_bytes(public_output.as_bytes());
        let receipt_claim = ReceiptClaim::ok(img, MaybePruned::Pruned(public_output_digest));
        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_img_id };
        let receipt = SetInclusionReceipt::from_path_with_verifier_params(
            receipt_claim,
            path,
            inclusion_params.digest(),
        );
        receipt
            .abi_encode_seal()
            .map_err(|e| AggregatorError::Other(anyhow!("abi encode set inclusion seal: {e}")))
    }

    async fn encode_compressed_seal(
        &self,
        state: &AggregationState,
    ) -> Result<Vec<u8>, AggregatorError> {
        let compressed_proof_id = state.compressed_proof_id.as_ref().ok_or_else(|| {
            AggregatorError::InvalidState("compressed_proof_id not present".into())
        })?;
        let selector = FixedBytes::from((SelectorExt::groth16_latest() as u32).to_be_bytes());
        self.provider
            .encode_seal(compressed_proof_id, selector)
            .await
            .map_err(|e| AggregatorError::Other(anyhow!("encode compressed seal: {e}")))
    }

    async fn compress(&self, state: &AggregationState) -> Result<String, AggregatorError> {
        let selector = FixedBytes::from((SelectorExt::groth16_latest() as u32).to_be_bytes());
        let program_id = ProgramId::new(
            self.set_builder_img_id.as_bytes().try_into().expect("R0 Digest is 32 bytes"),
        );
        self.provider
            .compress_proof(&program_id, &state.proof_id, selector)
            .await
            .map_err(|e| AggregatorError::ProvingFailed(format!("compress: {e}")))
    }
}

fn prune_receipt_claim_journal(mut claim: ReceiptClaim) -> ReceiptClaim {
    use risc0_zkvm::sha::{Impl as ShaImpl, Sha256};
    if let MaybePruned::Value(Some(output)) = &mut claim.output {
        let digest = match &output.journal {
            MaybePruned::Value(bytes) => Some(*ShaImpl::hash_bytes(bytes)),
            MaybePruned::Pruned(_) => None,
        };
        if let Some(digest) = digest {
            output.journal = MaybePruned::Pruned(digest);
        }
    }
    claim
}

#[cfg(feature = "test-utils")]
pub mod test_utils {
    //! Test helpers.

    use super::PersistedState;
    use boundless_market::aggregator::AggregatorError;
    use risc0_aggregation::GuestState;
    use risc0_zkvm::sha::Digest;

    /// Decoded view of an [`AggregationState::state`] blob.
    pub struct SetBuilderView {
        /// Set-builder MMR and commitment state.
        pub guest_state: GuestState,
        /// Claim digests in insertion order.
        pub claim_digests: Vec<Digest>,
    }

    /// Encode an [`AggregationState::state`] blob from a `(guest_state,
    /// claim_digests)` pair.
    ///
    /// [`AggregationState::state`]: boundless_market::aggregator::AggregationState::state
    pub fn encode_set_builder_state(
        guest_state: GuestState,
        claim_digests: Vec<Digest>,
    ) -> Vec<u8> {
        PersistedState { guest_state, claim_digests }.encode()
    }

    /// Decode an [`AggregationState::state`] blob.
    ///
    /// [`AggregationState::state`]: boundless_market::aggregator::AggregationState::state
    pub fn decode_set_builder_state(state: &[u8]) -> Result<SetBuilderView, AggregatorError> {
        let p = PersistedState::decode(state)?;
        Ok(SetBuilderView { guest_state: p.guest_state, claim_digests: p.claim_digests })
    }
}
