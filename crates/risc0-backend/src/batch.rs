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

use boundless_market::contracts::{AssessorCallback, AssessorSelector};
use risc0_zkvm::sha::Sha256;
use serde::{Deserialize, Serialize};

use super::*;

/// Assessor receipt data the risc0 backend extracts from a proven assessor journal.
/// Mirrors the fields `BoundlessMarket.sol::fulfill` needs from the assessor.
pub struct AssessorArtifact {
    pub claim_digest: ClaimDigest,
    pub selectors: Vec<AssessorSelector>,
    pub callbacks: Vec<AssessorCallback>,
}

/// Prunes the journal of a [`ReceiptClaim`] to its digest.
pub fn prune_receipt_claim_journal(mut claim: ReceiptClaim) -> ReceiptClaim {
    if let MaybePruned::Value(Some(output)) = &mut claim.output {
        let digest = match &output.journal {
            MaybePruned::Value(bytes) => Some(*risc0_zkvm::sha::Impl::hash_bytes(bytes)),
            MaybePruned::Pruned(_) => None,
        };

        if let Some(digest) = digest {
            output.journal = MaybePruned::Pruned(digest);
        }
    }

    claim
}

/// Current schema version of the serialized [`Risc0BatchState`].
///
/// [`Risc0BatchState::from_backend_state`] rejects a newer-than-known version. State written
/// before versioning defaults to 1.
const RISC0_BATCH_STATE_VERSION: u32 = 1;

fn risc0_batch_state_version() -> u32 {
    RISC0_BATCH_STATE_VERSION
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Risc0BatchState {
    /// Schema version of the persisted state. See [`RISC0_BATCH_STATE_VERSION`].
    #[serde(default = "risc0_batch_state_version")]
    version: u32,
    pub(super) guest_state: GuestState,
    pub(super) claim_digests: Vec<Risc0Digest>,
    /// Set-builder stark proof produced by [`Risc0BatchProcessor::prove_set_builder`].
    #[serde(default)]
    pub(super) proof_id: Option<String>,
    /// Groth16 compression of the set-builder proof.
    #[serde(default)]
    pub(super) compressed_proof_id: Option<String>,
    #[serde(default)]
    pub(super) assessor_proof_id: Option<String>,
}

impl Risc0BatchState {
    pub(super) fn from_backend_state(state: &BackendBatchState) -> Result<Self> {
        let decoded: Self = serde_json::from_value(state.0.clone())
            .context("Failed to decode RISC0 batch state")?;
        anyhow::ensure!(
            decoded.version <= RISC0_BATCH_STATE_VERSION,
            "RISC0 batch state schema version {} is newer than this broker supports ({}); \
             the broker may have been downgraded while a batch was in flight",
            decoded.version,
            RISC0_BATCH_STATE_VERSION,
        );
        Ok(decoded)
    }

    pub(super) fn into_backend_state(self) -> Result<BackendBatchState> {
        Ok(BackendBatchState(
            serde_json::to_value(self).context("Failed to encode RISC0 batch state")?,
        ))
    }
}

#[derive(Clone)]
pub(super) struct Risc0BatchProcessor {
    proof_retry: ProofRetryPolicy,
    prover: ProverObj,
    set_builder_guest_id: Risc0Digest,
    assessor_guest_id: Risc0Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
    /// Router snapshot + derived policy, shared with the backend.
    /// [`Risc0BatchProcessor::skip_assessor_guest`] consults it per batch to decide whether to skip
    /// proving the assessor guest (on-chain assessor) or prove it (R0 guest).
    router_policy: RouterPolicy,
}

#[derive(Clone)]
pub(super) struct Risc0Submission {
    prover: ProverObj,
}

impl Risc0Submission {
    pub fn new(prover: ProverObj) -> Self {
        Self { prover }
    }

    pub async fn encode_seal_for_selector(
        &self,
        selector: FixedBytes<4>,
        proof_id: &str,
    ) -> Result<Vec<u8>> {
        match super::compression_type_for_selector(selector) {
            super::CompressionType::Groth16 => self.encode_groth16_seal(proof_id).await,
            super::CompressionType::Blake3Groth16 => {
                self.encode_blake3_groth16_seal(proof_id).await
            }
            super::CompressionType::None => {
                anyhow::bail!("selector {selector:?} does not identify a compressed proof seal")
            }
        }
    }

    pub async fn encode_groth16_seal(&self, proof_id: &str) -> Result<Vec<u8>> {
        let groth16_receipt = self
            .prover
            .get_compressed_receipt(proof_id)
            .await
            .context("Failed to fetch g16 receipt")?
            .context("Groth16 receipt missing")?;

        let groth16_receipt: Receipt =
            bincode::deserialize(&groth16_receipt).context("Failed to deserialize g16 receipt")?;

        encode_seal(&groth16_receipt).context("Failed to encode g16 receipt seal")
    }

    pub async fn encode_blake3_groth16_seal(&self, proof_id: &str) -> Result<Vec<u8>> {
        let blake3_receipt = self
            .prover
            .get_blake3_groth16_receipt(proof_id)
            .await
            .context("Failed to fetch blake3 groth16 receipt")?
            .context("Blake3 Groth16 receipt missing")?;

        let blake3_receipt: Blake3Groth16Receipt = bincode::deserialize(&blake3_receipt)
            .context("Failed to deserialize Blake3 Groth16 receipt")?;

        let mut encoded_seal = encode_seal(&blake3_receipt.into())
            .context("Failed to encode Blake3 Groth16 receipt seal")?;
        if is_dev_mode() {
            let fake_selector = &[0xFFu8, 0xFF, 0x00, 0x00];
            encoded_seal.splice(0..4, fake_selector.iter().cloned());
        }

        Ok(encoded_seal)
    }

    pub fn claim_digest(&self, image_id: Risc0Digest, journal_digest: Risc0Digest) -> Risc0Digest {
        ReceiptClaim::ok(image_id, MaybePruned::Pruned(journal_digest)).digest()
    }

    pub async fn assessor_receipt(&self, proof_id: &str) -> Result<AssessorArtifact> {
        let receipt = self
            .prover
            .get_receipt(proof_id)
            .await
            .context("Failed to get assessor receipt")?
            .context("Assessor receipt missing")?;
        let claim_digest = receipt
            .claim()
            .with_context(|| format!("Receipt for assessor {proof_id} missing claim"))?
            .value()
            .with_context(|| format!("Receipt for assessor {proof_id} claims pruned"))?
            .digest();
        let journal = AssessorJournal::abi_decode(&receipt.journal.bytes)
            .with_context(|| format!("Failed to decode assessor journal for {proof_id}"))?;

        Ok(AssessorArtifact {
            claim_digest: ClaimDigest::from_native(claim_digest),
            selectors: journal.selectors,
            callbacks: journal.callbacks,
        })
    }
}

impl Risc0BatchProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        proof_retry: ProofRetryPolicy,
        prover: ProverObj,
        set_builder_guest_id: Risc0Digest,
        assessor_guest_id: Risc0Digest,
        market_addr: Address,
        prover_addr: Address,
        chain_id: u64,
        router_policy: RouterPolicy,
    ) -> Self {
        Self {
            proof_retry,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
            router_policy,
        }
    }

    /// Whether to skip proving the assessor guest for this batch: true when the batch's verifier
    /// class selects the on-chain assessor (sealed by a prover signature in `build_fulfillments`,
    /// not a STARK proof), false for the R0 guest. A batch is single assessor group, so any order's
    /// signed selector determines it; a selector that resolves to no supported assessor is an error
    /// (such an order can never be sealed and must not have been batched).
    fn skip_assessor_guest(&self, cmd: &UpdateBatch) -> Result<bool> {
        let signed = cmd
            .new_orders
            .first()
            .map(|o| o.proving.request.requirements.selector)
            .or_else(|| cmd.existing_orders.first().map(|o| o.request.requirements.selector));
        let Some(signed) = signed else {
            return Ok(false);
        };
        let selector =
            self.router_policy.assessor_selector_for_signed(signed).with_context(|| {
                format!("signed verifier selector {signed} resolves to no supported assessor")
            })?;
        Ok(selector == ONCHAIN_ASSESSOR_SELECTOR)
    }

    async fn validate_and_extract_claim(&self, proof_id: &str) -> Result<ReceiptClaim> {
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

    pub async fn prove_set_builder(
        &self,
        backend_state: Option<&BackendBatchState>,
        proofs: &[ProofId],
        finalize: bool,
        all_orders: &[String],
    ) -> Result<BackendBatchState> {
        let aggregation_state =
            backend_state.map(Risc0BatchState::from_backend_state).transpose()?;
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        let mut valid_proof_ids = Vec::<String>::with_capacity(proofs.len());

        for proof_id in proofs {
            match self.validate_and_extract_claim(proof_id.as_str()).await {
                Ok(claim) => {
                    claims.push(claim);
                    valid_proof_ids.push(proof_id.as_str().to_string());
                }
                Err(e) => {
                    tracing::error!(
                        "Error fetching proof from batch: {e:?} containing orders {:?}, excluding",
                        all_orders
                    );
                }
            }
        }

        if claims.is_empty() {
            anyhow::bail!("No valid proofs found in batch with orders {:?}", all_orders);
        }

        if valid_proof_ids.len() < proofs.len() {
            tracing::warn!(
                "Excluded {} invalid proofs from batch with orders {:?}. Valid: {}/{}",
                proofs.len() - valid_proof_ids.len(),
                all_orders,
                valid_proof_ids.len(),
                proofs.len()
            );
        }

        let input = aggregation_state
            .as_ref()
            .map_or(GuestState::initial(self.set_builder_guest_id), |s| s.guest_state.clone())
            .into_input(claims.clone(), finalize)
            .context("Failed to build set builder input")?;

        let assumption_ids: Vec<String> = aggregation_state
            .as_ref()
            .and_then(|s| s.proof_id.clone())
            .into_iter()
            .chain(valid_proof_ids.iter().cloned())
            .collect();

        let input_data =
            provers::encode_input(&input).context("Failed to encode set-builder proof input")?;
        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .context("Failed to upload set-builder input")?;

        let (retry_count, sleep_ms) = (self.proof_retry)();

        tracing::debug!("Starting proving of set-builder with orders {:?}", all_orders);
        let (proof_res, journal) = retry_with_context(
            retry_count,
            sleep_ms,
            || async {
                let proof_res = self
                    .prover
                    .prove_and_monitor_stark_high(
                        &self.set_builder_guest_id.to_string(),
                        &input_id,
                        assumption_ids.clone(),
                    )
                    .await
                    .map_err(|e| {
                        provers::ProverError::ProverInternalError(format!(
                            "Failed to prove set-builder: {e}"
                        ))
                    })?;

                tracing::debug!(
                    "Set-builder proof complete with orders {:?}, proof id: {} cycles: {:?} time: {}",
                    all_orders,
                    proof_res.id,
                    proof_res.stats.as_ref().map(|s| s.total_cycles),
                    proof_res.elapsed_time
                );

                let receipt = self
                    .prover
                    .get_receipt(&proof_res.id)
                    .await
                    .map_err(|e| {
                        provers::ProverError::ProverInternalError(format!(
                            "Failed to get receipt for set-builder: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        provers::ProverError::NotFound(format!(
                            "Receipt missing for set-builder: {}",
                            proof_res.id
                        ))
                    })?;

                receipt.verify(self.set_builder_guest_id).map_err(|e| {
                    provers::ProverError::ProverInternalError(format!(
                        "Set builder proof produced invalid receipt: {e}"
                    ))
                })?;

                let journal = receipt.journal.bytes;

                Ok::<_, provers::ProverError>((proof_res, journal))
            },
            "set_builder_prove_and_get_journal",
            &format!("orders {:?}", all_orders),
        )
        .await?;

        let guest_state = GuestState::decode(&journal).context("Failed to decode guest output")?;
        let claim_digests = aggregation_state
            .map(|s| s.claim_digests.clone())
            .unwrap_or_default()
            .into_iter()
            .chain(claims.into_iter().map(|claim| claim.digest()))
            .collect();

        Risc0BatchState {
            version: RISC0_BATCH_STATE_VERSION,
            guest_state,
            claim_digests,
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            assessor_proof_id: None,
        }
        .into_backend_state()
    }

    pub async fn prove_assessor(&self, orders: &[&OrderProvingData]) -> Result<String> {
        let mut fills = vec![];

        for order in orders {
            let order_id = &order.order_id;
            let backend_state = order
                .backend_state
                .as_ref()
                .with_context(|| format!("Missing backend state for order: {order_id}"))?;
            let state = super::Risc0OrderState::decode(backend_state)?;

            let journal = self
                .prover
                .get_journal(&state.proof_id)
                .await
                .with_context(|| format!("Failed to get {} journal", state.proof_id))?
                .with_context(|| format!("{} journal missing", state.proof_id))?;

            let fulfillment_data = match order.request.requirements.predicate.predicateType {
                PredicateType::ClaimDigestMatch => FulfillmentData::None,
                _ => FulfillmentData::from_image_id_and_journal(
                    Risc0Digest::from_hex(
                        order
                            .image_id
                            .as_deref()
                            .with_context(|| format!("Missing image_id for order: {order_id}"))?,
                    )?,
                    journal,
                ),
            };

            fills.push(Fulfillment {
                request: order.request.clone(),
                signature: order.client_sig.clone().to_vec(),
                fulfillment_data,
            })
        }

        let order_count = fills.len();
        let input = AssessorInput {
            fills,
            domain: eip712_domain(self.market_addr, self.chain_id),
            prover_address: self.prover_addr,
        };
        let stdin = GuestEnv::builder().write_frame(&input.encode()).stdin;

        let input_id =
            self.prover.upload_input(stdin).await.context("Failed to upload assessor input")?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark_high(&self.assessor_guest_id.to_string(), &input_id, vec![])
            .await
            .context("Failed to prove assesor stark")?;

        tracing::debug!(
            "Assessor proof completed, proof id: {} count: {} cycles: {:?} time: {}",
            proof_res.id,
            order_count,
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time
        );

        Ok(proof_res.id)
    }

    pub async fn compress_batch_proof(
        &self,
        batch_id: usize,
        aggregation_proof_id: &str,
        orders: &[String],
    ) -> Result<String, provers::ProverError> {
        let (retry_count, sleep_ms) = (self.proof_retry)();

        let context = format!("batch {batch_id} with orders {:?}", orders);
        retry_with_context(
            retry_count,
            sleep_ms,
            || async {
                let proof_id = self.prover.compress_high(aggregation_proof_id).await?;
                provers::verify_groth16_receipt(&self.prover, &proof_id).await?;
                Ok::<String, provers::ProverError>(proof_id)
            },
            "compress_and_verify",
            &context,
        )
        .await
    }
}

#[async_trait]
impl BatchProcessor for Risc0BatchProcessor {
    async fn estimate_batch_size(
        &self,
        cmd: BatchSizeEstimateRequest,
    ) -> Result<BatchSizeEstimate> {
        let BatchSizeEstimateRequest { state: _current_state, existing_orders, pending_orders } =
            cmd;
        let mut size = 0;
        for order in existing_orders.iter().chain(pending_orders.iter()) {
            let order_id = &order.order_id;
            let backend_state = order
                .backend_state
                .as_ref()
                .with_context(|| format!("Missing backend state for order {order_id}"))?;
            let state = super::Risc0OrderState::decode(backend_state)?;

            let journal = self
                .prover
                .get_journal(&state.proof_id)
                .await
                .with_context(|| format!("Failed to get journal for {}", state.proof_id))?
                .with_context(|| format!("Journal for {} missing", state.proof_id))?;

            // For RISC0 claim-digest match orders, the journal is not included in calldata.
            if !matches!(
                order.request.requirements.predicate.predicateType,
                PredicateType::ClaimDigestMatch
            ) {
                size += journal.len();
            }
        }

        Ok(BatchSizeEstimate { size })
    }

    async fn update_batch(&self, cmd: UpdateBatch) -> Result<BatchUpdate> {
        let all_orders: Vec<String> = cmd
            .existing_orders
            .iter()
            .map(|o| o.order_id.clone())
            .chain(cmd.new_orders.iter().map(|o| o.proving.order_id.clone()))
            .collect();
        let new_order_fee = cmd.new_orders.iter().fold(U256::ZERO, |sum, order| sum + order.fee);
        let earliest_expiration = cmd.new_orders.iter().map(|order| order.expiration).min();
        let earliest_lock_expiration =
            cmd.new_orders.iter().map(|order| order.lock_expiration).min();
        let request_ids = cmd.new_orders.iter().map(|order| order.request_id).collect::<Vec<_>>();
        let fulfillment_types =
            cmd.new_orders.iter().map(|order| order.fulfillment_type).collect::<Vec<_>>();
        tracing::trace!(
            "Updating RISC0 batch {} with {} new orders, fee {new_order_fee}, earliest_expiration {:?}, earliest_lock_expiration {:?}, request_ids {:?}, fulfillment_types {:?}",
            cmd.batch_id,
            cmd.new_orders.len(),
            earliest_expiration,
            earliest_lock_expiration,
            request_ids,
            fulfillment_types,
        );

        let mut assessor_secs = None;
        // With the on-chain assessor, skip proving + aggregating the assessor guest entirely; the
        // broker signs the batch in `build_fulfillments` instead. Decided per batch from its
        // verifier class.
        let skip_assessor_guest = self.skip_assessor_guest(&cmd)?;
        let assessor_proof_id: Option<String> = if cmd.finalize && !skip_assessor_guest {
            tracing::debug!(
                "Running assessor for batch {} with orders {:?}",
                cmd.batch_id,
                all_orders
            );

            let assessor_orders: Vec<&OrderProvingData> = cmd
                .existing_orders
                .iter()
                .chain(cmd.new_orders.iter().map(|o| &o.proving))
                .collect();

            let assessor_start = std::time::Instant::now();
            let assessor_proof_id = self
                .prove_assessor(&assessor_orders)
                .await
                .with_context(|| format!("Failed to prove assessor with orders {all_orders:?}"))?;
            assessor_secs = Some(assessor_start.elapsed().as_secs_f64());

            tracing::debug!(
                "Assessor proof complete for batch {} with orders {:?}, proof id: {}",
                cmd.batch_id,
                all_orders,
                assessor_proof_id
            );

            Some(assessor_proof_id)
        } else {
            None
        };

        let mut proof_ids: Vec<ProofId> = Vec::with_capacity(cmd.new_orders.len() + 1);
        for order in &cmd.new_orders {
            let backend_state = order.proving.backend_state.as_ref().with_context(|| {
                format!(
                    "Order {} missing backend state at update_batch for batch {}",
                    order.proving.order_id, cmd.batch_id
                )
            })?;
            let state = super::Risc0OrderState::decode(backend_state)?;
            // Direct-submit orders (those with a compressed proof already) skip set-builder.
            if state.compressed_proof_id.is_none() {
                proof_ids.push(ProofId::new(state.proof_id.clone()));
            }
        }
        proof_ids.extend(assessor_proof_id.iter().map(|proof_id| ProofId::new(proof_id.clone())));

        tracing::debug!(
            "Running set builder for batch {} of orders {:?} and proofs {:?}",
            cmd.batch_id,
            all_orders,
            proof_ids.iter().map(|proof_id| proof_id.as_str()).collect::<Vec<_>>()
        );
        let set_builder_start = std::time::Instant::now();
        // The set-builder only has work when there are proofs to aggregate: set-inclusion app
        // proofs and/or the R0 assessor guest. An all-direct-submit batch (groth16/blake3) under
        // the on-chain assessor aggregates nothing, so skip the set-builder entirely and carry the
        // batch state forward (or start empty). `build_fulfillments` then submits no merkle root.
        let aggregation_state = if proof_ids.is_empty() {
            let state = match cmd.state.as_ref() {
                Some(state) => Risc0BatchState::from_backend_state(state)?,
                None => Risc0BatchState {
                    version: RISC0_BATCH_STATE_VERSION,
                    guest_state: GuestState::initial(self.set_builder_guest_id),
                    claim_digests: vec![],
                    proof_id: None,
                    compressed_proof_id: None,
                    assessor_proof_id: None,
                },
            };
            state.into_backend_state()?
        } else {
            self.prove_set_builder(cmd.state.as_ref(), &proof_ids, cmd.finalize, &all_orders)
                .await
                .with_context(|| {
                    format!(
                        "Failed to prove set builder for batch {} with orders {:?}",
                        cmd.batch_id, all_orders
                    )
                })?
        };
        let batch_update_secs = Some(set_builder_start.elapsed().as_secs_f64());

        tracing::debug!(
            "Completed aggregation into batch {} of orders {:?} and proofs {:?}",
            cmd.batch_id,
            all_orders,
            proof_ids.iter().map(|proof_id| proof_id.as_str()).collect::<Vec<_>>()
        );

        // Stash the assessor proof id inside the backend-opaque state.
        let mut decoded = Risc0BatchState::from_backend_state(&aggregation_state)?;
        decoded.assessor_proof_id = assessor_proof_id;
        let aggregation_state = decoded.into_backend_state()?;

        Ok(BatchUpdate {
            state: aggregation_state,
            finalize: cmd.finalize,
            batch_update_secs,
            assessor_secs,
        })
    }

    async fn close_batch(&self, cmd: CloseBatch) -> Result<BatchClose, BackendError> {
        let start = std::time::Instant::now();
        let backend_state = cmd
            .state
            .as_ref()
            .with_context(|| {
                format!("Batch {} has no recorded backend state at close-batch time", cmd.batch_id)
            })
            .map_err(BackendError::operation)?;
        let mut state =
            Risc0BatchState::from_backend_state(backend_state).map_err(BackendError::operation)?;
        // No set-builder proof means the batch aggregated nothing (all direct-submit orders under
        // the on-chain assessor), so there is no merkle root to compress.
        if let Some(proof_id) = state.proof_id.clone() {
            let compressed_proof_id = self
                .compress_batch_proof(cmd.batch_id, &proof_id, &cmd.order_ids)
                .await
                .map_err(BackendError::from)?;
            state.compressed_proof_id = Some(compressed_proof_id);
        }

        Ok(BatchClose {
            state: state.into_backend_state().map_err(BackendError::operation)?,
            compression_secs: start.elapsed().as_secs_f64(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provers::DefaultProver;
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_backend::{BatchOrder, OrderProvingData};
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use boundless_market::prover_utils::FulfillmentType;
    use std::sync::Arc;

    fn sample_state() -> Risc0BatchState {
        Risc0BatchState {
            version: RISC0_BATCH_STATE_VERSION,
            guest_state: GuestState::initial(Risc0Digest::ZERO),
            claim_digests: vec![],
            proof_id: None,
            compressed_proof_id: None,
            assessor_proof_id: None,
        }
    }

    fn backend_state(data: serde_json::Value) -> BackendBatchState {
        BackendBatchState(data)
    }

    #[test]
    fn from_backend_state_accepts_current_version() {
        let state = backend_state(serde_json::to_value(sample_state()).unwrap());
        let decoded = Risc0BatchState::from_backend_state(&state).unwrap();
        assert_eq!(decoded.version, RISC0_BATCH_STATE_VERSION);
    }

    #[test]
    fn from_backend_state_defaults_missing_version() {
        // State persisted before the `version` field existed still decodes.
        let data = serde_json::json!({
            "guest_state": GuestState::initial(Risc0Digest::ZERO),
            "claim_digests": Vec::<Risc0Digest>::new(),
        });
        let decoded = Risc0BatchState::from_backend_state(&backend_state(data)).unwrap();
        assert_eq!(decoded.version, RISC0_BATCH_STATE_VERSION);
    }

    #[test]
    fn from_backend_state_rejects_future_version() {
        // A future schema version is rejected.
        let mut data = serde_json::to_value(sample_state()).unwrap();
        data["version"] = serde_json::json!(RISC0_BATCH_STATE_VERSION + 1);
        let err = match Risc0BatchState::from_backend_state(&backend_state(data)) {
            Ok(_) => panic!("future schema version must be rejected"),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("newer than this broker supports"),
            "unexpected error: {err}"
        );
    }

    fn test_request(seed: u32) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(Address::ZERO, seed),
            Requirements::new(Predicate::prefix_match(Risc0Digest::ZERO, Bytes::default())),
            "test",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                rampUpStart: 0,
                timeout: 1000,
                lockTimeout: 1000,
                rampUpPeriod: 1,
                lockCollateral: U256::ZERO,
            },
        )
    }

    fn test_processor(include_onchain_assessor: bool) -> Risc0BatchProcessor {
        let policy = Risc0Backend::router_policy(
            boundless_test_utils::market::test_router_registry(
                Risc0Digest::ZERO,
                include_onchain_assessor,
            ),
            boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO),
            true,
        );
        let prover: crate::provers::ProverObj = Arc::new(DefaultProver::new());
        Risc0BatchProcessor::new(
            Arc::new(|| (0, 0)),
            prover,
            Risc0Digest::ZERO,
            Risc0Digest::ZERO,
            Address::ZERO,
            Address::ZERO,
            1,
            policy,
        )
    }

    fn update_batch_cmd(request: ProofRequest) -> super::super::UpdateBatch {
        super::super::UpdateBatch {
            batch_id: 0,
            existing_orders: Vec::new(),
            state: None,
            new_orders: vec![BatchOrder {
                proving: OrderProvingData {
                    order_id: "order-1".to_string(),
                    request: request.clone(),
                    client_sig: Bytes::new(),
                    image_id: None,
                    backend_state: None,
                },
                expiration: 100,
                fee: U256::ZERO,
                fulfillment_type: FulfillmentType::LockAndFulfill,
                request_id: request.id,
                lock_expiration: 100,
            }],
            finalize: true,
        }
    }

    /// The guest skip follows the batch's resolved assessor: skipped when the verifier class
    /// selects the on-chain assessor, proven when it selects the R0 guest.
    #[test]
    fn skip_assessor_guest_follows_batch_assessor() {
        let cmd = update_batch_cmd(test_request(1));
        assert!(test_processor(true).skip_assessor_guest(&cmd).unwrap());
        assert!(!test_processor(false).skip_assessor_guest(&cmd).unwrap());
    }

    #[tokio::test]
    async fn update_batch_errors_when_new_order_missing_backend_state() {
        let prover: crate::provers::ProverObj = Arc::new(DefaultProver::new());

        let request = test_request(7);
        let order_id = "order-7".to_string();

        let policy = Risc0Backend::router_policy(
            boundless_test_utils::market::test_router_registry(Risc0Digest::ZERO, false),
            boundless_test_utils::market::set_verifier_selector(Risc0Digest::ZERO),
            true,
        );
        let processor = Risc0BatchProcessor::new(
            Arc::new(|| (0, 0)),
            prover,
            Risc0Digest::ZERO,
            Risc0Digest::ZERO,
            Address::ZERO,
            Address::ZERO,
            1,
            policy,
        );

        let res = processor
            .update_batch(super::super::UpdateBatch {
                batch_id: 0,
                existing_orders: Vec::new(),
                state: None,
                new_orders: vec![BatchOrder {
                    proving: OrderProvingData {
                        order_id: order_id.clone(),
                        request: request.clone(),
                        client_sig: Bytes::new(),
                        image_id: None,
                        backend_state: None,
                    },
                    expiration: 100,
                    fee: U256::ZERO,
                    fulfillment_type: FulfillmentType::LockAndFulfill,
                    request_id: request.id,
                    lock_expiration: 100,
                }],
                finalize: false,
            })
            .await;
        let err = match res {
            Ok(_) => panic!("expected update_batch to fail when new_order has no backend state"),
            Err(err) => err,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains(&order_id), "error must name the missing order id: {msg}");
        assert!(
            msg.contains("missing backend state"),
            "error must mention missing backend state: {msg}"
        );
    }
}
