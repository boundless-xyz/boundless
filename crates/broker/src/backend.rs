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

//! Broker-facing backend boundary.
//!
//! This module starts as a thin adapter over the existing RISC Zero broker code.
//! The broker keeps DB state, retry policy, cancellation, and batch lifecycle
//! orchestration; the backend owns zkVM-specific proof processing.

use alloy::primitives::{Address, FixedBytes};
use alloy::sol_types::SolValue;
use async_trait::async_trait;
use blake3_groth16::Blake3Groth16Receipt;
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{
    contracts::{eip712_domain, encode_seal, AssessorJournal, FulfillmentData, PredicateType},
    input::GuestEnv,
    selector::{is_blake3_groth16_selector, is_groth16_selector},
};
use hex::FromHex;
use risc0_aggregation::GuestState;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::DbObj,
    futures_retry::retry_with_context,
    is_dev_mode,
    provers::{self, ProverObj},
    requestor_monitor::PriorityRequestors,
    storage::{upload_image_uri, upload_input_uri},
    utils::prune_receipt_claim_journal,
    AggregationState, CompressionType, ConfigurableDownloader, Order, OrderStatus,
};
use anyhow::{Context, Result};

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
    pub order_id: String,
    pub proof_id: String,
    pub compressed_proof_id: Option<String>,
    pub next_status: OrderStatus,
}

#[async_trait]
pub trait Backend: Send + Sync {
    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress>;
}

#[derive(Clone)]
pub struct Risc0Backend {
    prover: ProverObj,
    snark_prover: ProverObj,
    downloader: ConfigurableDownloader,
    priority_requestors: PriorityRequestors,
}

impl Risc0Backend {
    pub fn new(
        prover: ProverObj,
        snark_prover: ProverObj,
        downloader: ConfigurableDownloader,
        priority_requestors: PriorityRequestors,
    ) -> Self {
        Self { prover, snark_prover, downloader, priority_requestors }
    }

    async fn start_order(&self, order: &Order) -> Result<String> {
        let order_id = order.id();

        if let Some(existing_proof_id) = order.proof_id.clone() {
            tracing::debug!("Using existing proof {existing_proof_id} for order {order_id}");
            return Ok(existing_proof_id);
        }

        tracing::info!("Proving order {order_id}");

        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_image_uri(&self.prover, &order.request, &self.downloader)
                .await
                .with_context(|| format!("Failed to upload image for order {order_id}"))?,
        };

        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => upload_input_uri(
                &self.prover,
                &order.request,
                &self.downloader,
                &self.priority_requestors,
            )
            .await
            .with_context(|| format!("Failed to upload input for order {order_id}"))?,
        };

        let proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
            .await
            .with_context(|| format!("Failed to prove customer proof STARK order {order_id}"))?;

        tracing::debug!("Order {order_id} being proved, proof id: {proof_id}");
        Ok(proof_id)
    }

    async fn compress_order_proof(
        &self,
        order_id: &str,
        stark_proof_id: &str,
        compression_type: CompressionType,
    ) -> Result<String> {
        let proof_id = match compression_type {
            CompressionType::Groth16 => self
                .snark_prover
                .compress(stark_proof_id)
                .await
                .with_context(|| format!("Failed to compress proof for order {order_id}"))?,
            CompressionType::Blake3Groth16 => {
                self.snark_prover.compress_blake3_groth16(stark_proof_id).await.with_context(
                    || format!("Failed to compress blake3 groth16 proof for order {order_id}"),
                )?
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        };

        match compression_type {
            CompressionType::Groth16 => {
                tracing::trace!(
                    "Verifying compressed Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::Blake3Groth16 => {
                tracing::trace!(
                    "Verifying compressed Blake3 Groth16 receipt locally for proof_id: {proof_id}, order {order_id}"
                );
                provers::verify_blake3_groth16_receipt(&self.snark_prover, &proof_id).await?;
            }
            CompressionType::None => unreachable!("compression type should not be None"),
        }

        Ok(proof_id)
    }
}

#[async_trait]
impl Backend for Risc0Backend {
    async fn process_order(&self, cmd: ProcessOrder) -> Result<OrderProcessProgress> {
        let order = cmd.order;
        let order_id = order.id();

        let Some(stark_proof_id) = order.proof_id.clone() else {
            return Ok(OrderProcessProgress::Started { proof_id: self.start_order(&order).await? });
        };

        let proof_res = self
            .prover
            .wait_for_stark(&stark_proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        let compression_type = order.compression_type();
        tracing::debug!(
            "Order {order_id} has compression_type: {compression_type:?}, snark_proof_id: {:?}",
            order.compressed_proof_id
        );

        if compression_type != CompressionType::None && order.compressed_proof_id.is_none() {
            let compressed_proof_id =
                self.compress_order_proof(&order_id, &stark_proof_id, compression_type).await?;
            return Ok(OrderProcessProgress::Compressed {
                proof_id: stark_proof_id,
                compressed_proof_id,
            });
        }

        let next_status = match compression_type {
            CompressionType::None => OrderStatus::PendingAgg,
            CompressionType::Groth16 | CompressionType::Blake3Groth16 => {
                OrderStatus::SkipAggregation
            }
        };

        tracing::info!(
            "Customer Proof complete for proof_id: {stark_proof_id}, order_id: {order_id} cycles: {:?} time: {}",
            proof_res.stats.as_ref().map(|s| s.total_cycles),
            proof_res.elapsed_time,
        );

        Ok(OrderProcessProgress::Completed(ProcessedOrder {
            order_id,
            proof_id: stark_proof_id,
            compressed_proof_id: order.compressed_proof_id,
            next_status,
        }))
    }
}

#[derive(Clone)]
pub struct Risc0BatchProcessor {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    set_builder_guest_id: Digest,
    assessor_guest_id: Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
}

#[derive(Clone)]
pub struct Risc0SubmissionCodec {
    prover: ProverObj,
}

pub struct Risc0AssessorReceipt {
    pub claim_digest: Digest,
    pub journal: AssessorJournal,
}

impl Risc0SubmissionCodec {
    pub fn new(prover: ProverObj) -> Self {
        Self { prover }
    }

    pub async fn encode_seal_for_selector(
        &self,
        selector: FixedBytes<4>,
        proof_id: &str,
    ) -> Result<Vec<u8>> {
        if is_groth16_selector(selector) {
            self.encode_groth16_seal(proof_id).await
        } else if is_blake3_groth16_selector(selector) {
            self.encode_blake3_groth16_seal(proof_id).await
        } else {
            anyhow::bail!("selector {selector:?} does not identify a compressed proof seal")
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

    pub fn claim_digest(&self, image_id: Digest, journal_digest: Digest) -> Digest {
        ReceiptClaim::ok(image_id, MaybePruned::Pruned(journal_digest)).digest()
    }

    pub async fn assessor_receipt(&self, proof_id: &str) -> Result<Risc0AssessorReceipt> {
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

        Ok(Risc0AssessorReceipt { claim_digest, journal })
    }
}

impl Risc0BatchProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        set_builder_guest_id: Digest,
        assessor_guest_id: Digest,
        market_addr: Address,
        prover_addr: Address,
        chain_id: u64,
    ) -> Self {
        Self {
            db,
            config,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
        }
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
        aggregation_state: Option<&AggregationState>,
        proofs: &[String],
        finalize: bool,
        all_orders: &[String],
    ) -> Result<AggregationState> {
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        let mut valid_proof_ids = Vec::<String>::with_capacity(proofs.len());

        for proof_id in proofs {
            match self.validate_and_extract_claim(proof_id).await {
                Ok(claim) => {
                    claims.push(claim);
                    valid_proof_ids.push(proof_id.clone());
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
            anyhow::bail!(format!("No valid proofs found in batch with orders {:?}", all_orders));
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
            .map_or(GuestState::initial(self.set_builder_guest_id), |s| s.guest_state.clone())
            .into_input(claims.clone(), finalize)
            .context("Failed to build set builder input")?;

        let assumption_ids: Vec<String> = aggregation_state
            .map(|s| s.proof_id.clone())
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

        let (retry_count, sleep_ms) = {
            let config = self.config.lock_all().context("Failed to lock config")?;
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

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

        Ok(AggregationState {
            guest_state,
            proof_id: proof_res.id,
            claim_digests,
            groth16_proof_id: None,
        })
    }

    pub async fn prove_assessor(&self, order_ids: &[String]) -> Result<String> {
        let mut fills = vec![];

        for order_id in order_ids {
            let order = self
                .db
                .get_order(order_id)
                .await
                .with_context(|| format!("Failed to get DB order ID {order_id}"))?
                .with_context(|| format!("order ID {order_id} missing from DB"))?;

            let proof_id = order
                .proof_id
                .with_context(|| format!("Missing proof_id for order: {order_id}"))?;

            let journal = self
                .prover
                .get_journal(&proof_id)
                .await
                .with_context(|| format!("Failed to get {proof_id} journal"))?
                .with_context(|| format!("{proof_id} journal missing"))?;

            let fulfillment_data = match order.request.requirements.predicate.predicateType {
                PredicateType::ClaimDigestMatch => FulfillmentData::None,
                _ => FulfillmentData::from_image_id_and_journal(
                    Digest::from_hex(
                        order
                            .image_id
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
        let (retry_count, sleep_ms) = {
            let config = self.config.lock_all().map_err(|e| {
                provers::ProverError::ProverInternalError(format!("Failed to lock config: {e}"))
            })?;
            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
        };

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
