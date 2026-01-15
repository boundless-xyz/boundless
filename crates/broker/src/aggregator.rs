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

use alloy::primitives::{utils, Address};
use anyhow::{Context, Result};
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{
    contracts::{eip712_domain, FulfillmentData, PredicateType},
    input::GuestEnv,
};
use chrono::Utc;
use hex::FromHex;
use risc0_aggregation::GuestState;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::{AggregationOrder, DbObj},
    errors::CodedError,
    futures_retry::retry_with_context,
    impl_coded_debug, now_timestamp,
    provers::{self, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    utils::prune_receipt_claim_journal,
    AggregationState, Batch, BatchStatus,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

#[derive(Error)]
pub enum AggregatorErr {
    #[error("{code} Compression error: {0}", code = self.code())]
    CompressionErr(crate::provers::ProverError),
    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(AggregatorErr);

impl CodedError for AggregatorErr {
    fn code(&self) -> &str {
        match self {
            AggregatorErr::UnexpectedErr(_) => "[B-AGG-500]",
            AggregatorErr::CompressionErr(_) => "[B-AGG-400]",
        }
    }
}

#[derive(Clone)]
pub struct AggregatorService {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    set_builder_guest_id: Digest,
    assessor_guest_id: Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
}

impl AggregatorService {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        db: DbObj,
        chain_id: u64,
        set_builder_guest_id: Digest,
        assessor_guest_id: Digest,
        market_addr: Address,
        prover_addr: Address,
        config: ConfigLock,
        prover: ProverObj,
    ) -> Result<Self> {
        Ok(Self {
            db,
            config,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
        })
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

    async fn prove_set_builder(
        &self,
        aggregation_state: Option<&AggregationState>,
        proofs: &[String],
        finalize: bool,
        all_orders: &[String],
    ) -> Result<AggregationState> {
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        let mut valid_proof_ids = Vec::<String>::with_capacity(proofs.len());

        // Verify each proof and collect only valid ones
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

        // Gather the proof IDs for the assumptions we will need: any pending proofs, and the proof
        // for the current aggregation state.
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

        // TODO: we should run this on a different stream in the prover
        // aka make a few different priority streams for each level of the proving

        // TODO: Need to set a timeout here to handle stuck or even just alert on delayed proving if
        // the proving cluster is overloaded

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
                    .prove_and_monitor_stark(
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
                    "Set-builder proof complete with orders {:?}, proof id: {} cycles: {} time: {}",
                    all_orders,
                    proof_res.id,
                    proof_res.stats.total_cycles,
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

    async fn prove_assessor(&self, order_ids: &[String]) -> Result<String> {
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
            .prove_and_monitor_stark(&self.assessor_guest_id.to_string(), &input_id, vec![])
            .await
            .context("Failed to prove assesor stark")?;

        tracing::debug!(
            "Assessor proof completed, proof id: {} count: {} cycles: {} time: {}",
            proof_res.id,
            order_count,
            proof_res.stats.total_cycles,
            proof_res.elapsed_time
        );

        Ok(proof_res.id)
    }

    /// Get the sum of the size of the journals for proofs in a batch
    async fn get_combined_journal_size(&self, order_ids: &[String]) -> Result<usize> {
        let mut journal_size = 0;
        for order_id in order_ids {
            let order = self
                .db
                .get_order(order_id)
                .await
                .with_context(|| format!("Failed to get order {order_id}"))?
                .with_context(|| format!("Order {order_id} missing from DB"))?;

            let proof_id =
                order.proof_id.with_context(|| format!("Missing proof_id for order {order_id}"))?;

            let journal = self
                .prover
                .get_journal(&proof_id)
                .await
                .with_context(|| format!("Failed to get journal for {proof_id}"))?
                .with_context(|| format!("Journal for {proof_id} missing"))?;

            // For claim digest match predicate orders, the journal is not included in the calldata.
            if !matches!(
                order.request.requirements.predicate.predicateType,
                PredicateType::ClaimDigestMatch
            ) {
                journal_size += journal.len();
            }
        }

        Ok(journal_size)
    }

    /// Check if we should finalize the batch
    ///
    /// Checks current min-deadline, batch timer, and current block.
    async fn check_finalize(
        &self,
        batch_id: usize,
        batch: &Batch,
        pending_orders: &[AggregationOrder],
    ) -> Result<bool> {
        let (
            conf_batch_size,
            conf_batch_time,
            conf_batch_fees,
            conf_max_journal_bytes,
            conf_max_concurrent_proofs,
        ) = {
            let config = self.config.lock_all().context("Failed to lock config")?;

            // TODO: Move this parse into config
            let batch_max_fees = match config.batcher.batch_max_fees.as_ref() {
                Some(elm) => {
                    Some(utils::parse_ether(elm).context("Failed to parse batch max fees")?)
                }
                None => None,
            };
            (
                config.batcher.min_batch_size,
                config.batcher.batch_max_time,
                batch_max_fees,
                config.batcher.batch_max_journal_bytes,
                config.market.max_concurrent_proofs,
            )
        };

        // Check that the min batch size is not greater than the max concurrent proofs
        let conf_batch_size = if conf_batch_size > conf_max_concurrent_proofs {
            tracing::warn!(
                "Configured min_batch_size ({}) exceeds max_concurrent_proofs ({}). \
                 Setting min_batch_size to max_concurrent_proofs.",
                conf_batch_size,
                conf_max_concurrent_proofs
            );
            conf_max_concurrent_proofs
        } else {
            conf_batch_size
        };

        // Skip finalization checks if we have nothing in this batch
        let is_initial_state =
            batch.aggregation_state.as_ref().map(|s| s.guest_state.is_initial()).unwrap_or(true);
        if is_initial_state && pending_orders.is_empty() {
            return Ok(false);
        }

        // Finalize the batch whenever it exceeds a target size.
        // Add any pending jobs into the batch along with the finalization run.
        let batch_size = batch.orders.len() + pending_orders.len();
        if batch_size >= conf_batch_size as usize {
            tracing::debug!(
                "Finalizing batch {batch_id}: size target hit {} - {}",
                batch_size,
                conf_batch_size
            );
            return Ok(true);
        } else {
            tracing::debug!(
                "Batch {batch_id} below size target hit {} - {}",
                batch_size,
                conf_batch_size
            );
        }

        // Finalize the batch if the journal size is already above the max
        let batch_journal_size = self.get_combined_journal_size(&batch.orders).await?;
        let pending_order_ids: Vec<_> = pending_orders.iter().map(|o| o.order_id.clone()).collect();
        let pending_journal_size = self.get_combined_journal_size(&pending_order_ids).await?;
        let journal_size = batch_journal_size + pending_journal_size;
        if journal_size >= conf_max_journal_bytes {
            tracing::debug!(
                "Finalizing batch {batch_id}: journal size target hit {} >= {}",
                journal_size,
                conf_max_journal_bytes
            );
            return Ok(true);
        } else {
            tracing::debug!(
                "Batch {batch_id} journal size below limit {} < {}",
                journal_size,
                conf_max_journal_bytes
            );
        }

        // Finalize the batch whenever the current batch exceeds a certain age (e.g. one hour).
        let time_delta = Utc::now() - batch.start_time;
        if time_delta.num_seconds() as u64 >= conf_batch_time {
            tracing::debug!(
                "Finalizing batch {batch_id}: time limit hit {} - {}",
                time_delta.num_seconds(),
                batch.start_time
            );
            return Ok(true);
        } else {
            tracing::debug!("Batch {batch_id} below time limit");
        }

        // Finalize whenever a batch hits the target fee total.
        if let Some(batch_target_fees) = conf_batch_fees {
            let fees =
                pending_orders.iter().map(|order| order.fee).fold(batch.fees, |sum, fee| sum + fee);

            if fees >= batch_target_fees {
                tracing::debug!("Finalizing batch {batch_id}: fee target hit");
                return Ok(true);
            } else {
                tracing::debug!("Batch {batch_id} below fee target");
            }
        }

        // Finalize whenever a deadline is approaching.
        let conf_deadline_buf_secs = {
            let config = self.config.lock_all().context("Failed to lock config")?;
            config.batcher.block_deadline_buffer_secs
        };
        let now = now_timestamp();

        let deadline = pending_orders
            .iter()
            .map(|order| order.expiration)
            .chain(batch.deadline)
            .reduce(u64::min);

        if let Some(deadline) = deadline {
            let remaining_secs = deadline.saturating_sub(now);
            if remaining_secs <= conf_deadline_buf_secs {
                tracing::debug!(
                    "Finalizing batch {batch_id}: getting close to deadline {remaining_secs}"
                );
                return Ok(true);
            } else {
                tracing::debug!("Batch {batch_id} not too close to deadline {remaining_secs}");
            }
        } else {
            tracing::warn!("Batch {batch_id} does not yet have a block_deadline");
        };

        Ok(false)
    }

    /// Filter out expired orders and mark them as failed
    async fn filter_expired_orders(
        &self,
        orders: Vec<AggregationOrder>,
        current_time: u64,
    ) -> Result<Vec<AggregationOrder>, AggregatorErr> {
        let mut valid_orders = Vec::with_capacity(orders.len());

        for order in orders {
            if order.expiration < current_time {
                tracing::warn!(
                    "[B-AGG-600] Order {} has expired during aggregation, marking as failed",
                    order.order_id
                );

                if let Err(err) =
                    self.db.set_order_failure(&order.order_id, "Expired before aggregation").await
                {
                    tracing::error!(
                        "Failed to set order {} as failed before aggregation: {err}",
                        order.order_id,
                    );
                }
            } else {
                valid_orders.push(order);
            }
        }

        Ok(valid_orders)
    }

    /// Get all pending proofs, filter expired orders, and return both aggregation and groth16 proofs separately
    async fn get_filtered_pending_proofs(
        &self,
    ) -> Result<(Vec<AggregationOrder>, Vec<AggregationOrder>), AggregatorErr> {
        let current_time = crate::now_timestamp();

        // Get both types of proofs
        let new_proofs =
            self.db.get_aggregation_proofs().await.context("Failed to get aggregation proofs")?;
        let groth16_proofs =
            self.db.get_groth16_proofs().await.context("Failed to get groth16 proofs")?;

        // Filter expired orders from both lists
        let valid_new_proofs = self.filter_expired_orders(new_proofs, current_time).await?;
        let valid_groth16_proofs = self.filter_expired_orders(groth16_proofs, current_time).await?;

        Ok((valid_new_proofs, valid_groth16_proofs))
    }

    async fn aggregate_proofs(
        &self,
        batch_id: usize,
        batch: &Batch,
        new_proofs: &[AggregationOrder],
        new_groth16_proofs: &[AggregationOrder],
        finalize: bool,
    ) -> Result<String> {
        let all_orders: Vec<String> = batch
            .orders
            .iter()
            .chain(new_proofs.iter().map(|p| &p.order_id))
            .chain(new_groth16_proofs.iter().map(|p| &p.order_id))
            .cloned()
            .collect();
        let assessor_proof_id = if finalize {
            let assessor_order_ids: Vec<String> = all_orders.clone();

            tracing::debug!(
                "Running assessor for batch {batch_id} with orders {:?}",
                assessor_order_ids
            );

            let assessor_proof_id =
                self.prove_assessor(&assessor_order_ids).await.with_context(|| {
                    format!("Failed to prove assessor with orders {assessor_order_ids:?}")
                })?;

            tracing::debug!(
                "Assessor proof complete for batch {batch_id} with orders {:?}, proof id: {}",
                assessor_order_ids,
                assessor_proof_id
            );

            Some(assessor_proof_id)
        } else {
            None
        };

        let proof_ids: Vec<String> = new_proofs
            .iter()
            .cloned()
            .map(|proof| proof.proof_id.clone())
            .chain(assessor_proof_id.iter().cloned())
            .collect();

        tracing::debug!(
            "Running set builder for batch {batch_id} of orders {:?} and proofs {:?}",
            all_orders,
            proof_ids
        );
        let aggregation_state = self
            .prove_set_builder(batch.aggregation_state.as_ref(), &proof_ids, finalize, &all_orders)
            .await
            .context(format!(
                "Failed to prove set builder for batch {batch_id} with orders {:?}",
                all_orders
            ))?;

        tracing::debug!(
            "Completed aggregation into batch {batch_id} of orders {:?} and proofs {:?}",
            all_orders,
            proof_ids
        );

        self.db
            .update_batch(
                batch_id,
                &aggregation_state,
                &[new_proofs, new_groth16_proofs].concat(),
                assessor_proof_id,
            )
            .await
            .with_context(|| {
                format!("Failed to update batch {batch_id} with orders {:?} in the DB", all_orders)
            })?;

        Ok(aggregation_state.proof_id)
    }

    async fn aggregate(&self) -> Result<(), AggregatorErr> {
        // Get the current batch. This aggregator service works on one batch at a time, including
        // any proofs ready for aggregation into the current batch.
        let batch_id = self.db.get_current_batch().await.context("Failed to get current batch")?;
        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        let (aggregation_proof_id, compress) = match batch.status {
            BatchStatus::Aggregating => {
                // Get and filter all pending proofs
                let (new_proofs, new_groth16_proofs) = self.get_filtered_pending_proofs().await?;

                // Finalize the current batch before adding any new orders if the finalization conditions
                // are already met.
                let finalize = self
                    .check_finalize(
                        batch_id,
                        &batch,
                        &[new_proofs.clone(), new_groth16_proofs.clone()].concat(),
                    )
                    .await?;

                // If we don't need to finalize, and there are no new proofs, there is no work to do.
                if !finalize && new_proofs.is_empty() {
                    tracing::trace!("No aggregation work to do for batch {batch_id}");
                    return Ok(());
                }

                let aggregation_proof_id = self
                    .aggregate_proofs(batch_id, &batch, &new_proofs, &new_groth16_proofs, finalize)
                    .await
                    .context(format!(
                        "Failed to aggregate proofs for batch {batch_id} with orders {:?}",
                        batch.orders
                    ))?;
                (aggregation_proof_id, finalize)
            }
            BatchStatus::PendingCompression => {
                let aggregation_state = batch.aggregation_state.with_context(|| format!("Batch {batch_id} in inconsistent state: status is PendingCompression but aggregation_state is None"))?;
                (aggregation_state.proof_id, true)
            }
            status => {
                return Err(AggregatorErr::UnexpectedErr(anyhow::anyhow!(
                    "Unexpected batch status {status:?}"
                )))
            }
        };

        if compress {
            let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;
            tracing::debug!(
                "Starting groth16 compression proof for batch {batch_id} with orders {:?}",
                batch.orders
            );

            let (retry_count, sleep_ms) = {
                let config = self.config.lock_all().context("Failed to lock config")?;
                (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
            };

            let context = format!("batch {batch_id} with orders {:?}", batch.orders);
            let compress_proof_id = match retry_with_context(
                retry_count,
                sleep_ms,
                || async {
                    let proof_id = self.prover.compress(&aggregation_proof_id).await?;
                    provers::verify_groth16_receipt(&self.prover, &proof_id).await?;
                    Ok::<String, provers::ProverError>(proof_id)
                },
                "compress_and_verify",
                &context,
            )
            .await
            {
                Ok(id) => id,
                Err(err) => {
                    self.db
                        .set_batch_failure(batch_id, err.to_string())
                        .await
                        .map_err(|e| AggregatorErr::UnexpectedErr(e.into()))?;
                    return Err(AggregatorErr::CompressionErr(err));
                }
            };
            tracing::debug!(
                "Completed groth16 compression for batch {batch_id} with orders {:?}",
                batch.orders
            );

            self.db.complete_batch(batch_id, &compress_proof_id).await.with_context(|| {
                format!("Failed to set batch {batch_id} with orders {:?} as complete", batch.orders)
            })?;
        }

        Ok(())
    }
}

impl RetryTask for AggregatorService {
    type Error = AggregatorErr;
    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let self_clone = self.clone();

        Box::pin(async move {
            tracing::debug!("Starting Aggregator service");
            loop {
                if cancel_token.is_cancelled() {
                    tracing::debug!("Aggregator service received cancellation");
                    break;
                }

                let conf_poll_time_ms = {
                    let config = self_clone
                        .config
                        .lock_all()
                        .context("Failed to lock config")
                        .map_err(AggregatorErr::UnexpectedErr)
                        .map_err(SupervisorErr::Recover)?;
                    config.batcher.batch_poll_time_ms.unwrap_or(1000)
                };

                self_clone.aggregate().await.map_err(SupervisorErr::Recover)?;
                tokio::time::sleep(tokio::time::Duration::from_millis(conf_poll_time_ms)).await;
            }

            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        chain_monitor::ChainMonitorService,
        db::SqliteDb,
        now_timestamp,
        provers::{encode_input, DefaultProver, Prover},
        BatchStatus, FulfillmentType, Order, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Bytes, U256},
        providers::{ext::AnvilApi, Provider, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use boundless_test_utils::guests::{
        ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ECHO_ELF, ECHO_ID, SET_BUILDER_ELF, SET_BUILDER_ID,
    };
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn aggregate_order_one_shot() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let prover_addr = signer.address();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.min_batch_size = 2;
        }

        let prover: ProverObj = Arc::new(DefaultProver::new());

        // Pre-prove the echo aka app guest:
        let image_id = Digest::from(ECHO_ID);
        let image_id_str = image_id.to_string();
        prover.upload_image(&image_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();
        let proof_res_1 =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();
        let proof_res_2 =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        let _handle = tokio::spawn(chain_monitor.spawn(CancellationToken::new()));
        let chain_id = provider.get_chain_id().await.unwrap();
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let aggregator = AggregatorService::new(
            db.clone(),
            chain_id,
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let min_price = 2;

        // First order
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 0),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res_1.id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        // Second order
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 1),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes()
            .into();
        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            proof_id: Some(proof_res_2.id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig,
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let (_batch_id, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert!(!batch.orders.is_empty());
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }

    #[tokio::test]
    #[traced_test]
    async fn aggregate_order_incremental() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let prover_addr = signer.address();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.min_batch_size = 2;
            config.market.max_concurrent_proofs = 2;
        }

        let prover: ProverObj = Arc::new(DefaultProver::new());

        // Pre-prove the echo aka app guest:
        let image_id = Digest::from(ECHO_ID);
        let image_id_str = image_id.to_string();
        prover.upload_image(&image_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();
        let proof_res_1 =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();
        let proof_res_2 =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());
        let _handle = tokio::spawn(chain_monitor.spawn(CancellationToken::new()));
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let aggregator = AggregatorService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();
        let min_price = 2;

        // First order
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 0),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                rampUpStart: now_timestamp(),
                timeout: 1200,
                lockTimeout: 1200,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res_1.id),
            compressed_proof_id: None,
            expire_timestamp: Some(order_request.expires_at()),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            request: order_request,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        // Aggregate the first order. Should not finalize.
        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let option_batch = db.get_complete_batch().await.unwrap();
        assert!(option_batch.is_none());

        let aggregating_batch_id = db.get_current_batch().await.unwrap();
        let aggregating_batch = db.get_batch(aggregating_batch_id).await.unwrap();
        assert_eq!(aggregating_batch.orders, vec![order.id()]);
        assert!(aggregating_batch.aggregation_state.is_some());
        assert!(!aggregating_batch.aggregation_state.unwrap().guest_state.mmr.is_finalized());

        // Second order
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 1),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                rampUpStart: now_timestamp(),
                timeout: 1200,
                lockTimeout: 1200,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes()
            .into();
        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            proof_id: Some(proof_res_2.id),
            compressed_proof_id: None,
            expire_timestamp: Some(order_request.expires_at()),
            client_sig,
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            request: order_request,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let (_batch_id, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert!(!batch.orders.is_empty());
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }

    #[tokio::test]
    #[traced_test]
    async fn fee_finalize() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let prover_addr = signer.address();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.min_batch_size = 2;
            config.batcher.batch_max_fees = Some("0.1".into());
        }

        let prover: ProverObj = Arc::new(DefaultProver::new());

        // Pre-prove the echo aka app guest:
        let image_id = Digest::from(ECHO_ID);
        let image_id_str = image_id.to_string();
        prover.upload_image(&image_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();
        let proof_res =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let aggregator = AggregatorService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 0),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let (_batch_id, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert!(!batch.orders.is_empty());
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }

    #[tokio::test]
    #[traced_test]
    async fn deadline_finalize() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.min_batch_size = 2;
            config.market.max_concurrent_proofs = 2;
            config.batcher.block_deadline_buffer_secs = 100;
        }

        let prover: ProverObj = Arc::new(DefaultProver::new());

        // Pre-prove the echo aka app guest:
        let image_id = Digest::from(ECHO_ID);
        let image_id_str = image_id.to_string();
        prover.upload_image(&image_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();
        let proof_res =
            prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());

        let _handle = tokio::spawn(chain_monitor.spawn(CancellationToken::new()));

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let aggregator = AggregatorService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            signer.address(),
            config.clone(),
            prover,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 0),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        provider.anvil_mine(Some(51), Some(2)).await.unwrap();

        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let (_batch_id, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert!(!batch.orders.is_empty());
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
        assert!(logs_contain("getting close to deadline"));
    }

    #[tokio::test]
    #[traced_test]
    async fn journal_size_finalize() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.min_batch_size = 10;
            config.market.max_concurrent_proofs = 10;
            // set config such that the batch max journal size is exceeded
            // if two ECHO sized journals are included in a batch
            config.market.max_journal_bytes = 20;
            config.batcher.batch_max_journal_bytes = 30;
        }

        let mock_prover = DefaultProver::new();

        // Pre-prove the echo aka app guest:
        let image_id = Digest::from(ECHO_ID);
        let image_id_str = image_id.to_string();
        mock_prover.upload_image(&image_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = mock_prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();
        let proof_res =
            mock_prover.prove_and_monitor_stark(&image_id_str, &input_id, vec![]).await.unwrap();

        let prover: ProverObj = Arc::new(mock_prover);

        let chain_monitor = Arc::new(ChainMonitorService::new(provider.clone()).await.unwrap());

        let _handle = tokio::spawn(chain_monitor.spawn(CancellationToken::new()));

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let aggregator = AggregatorService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            signer.address(),
            config.clone(),
            prover,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 0),
            Requirements::new(Predicate::prefix_match(image_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request.clone(),
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.clone().id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 1000),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };

        // add first order and aggregate
        db.add_order(&order).await.unwrap();
        aggregator.aggregate().await.unwrap();
        assert!(logs_contain("journal size below limit 20 < 30"));

        let batch_res = db.get_complete_batch().await.unwrap();
        assert!(batch_res.is_none());

        // Add another order, this should cross the journal limit threshold and
        // trigger the batch to be finalized
        let mut order_request_2 = order_request.clone();
        order_request_2.id = RequestId::new(customer_signer.address(), 1).into();

        let client_sig_2 = order_request_2
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let order2 = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request_2,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 1000),
            client_sig: client_sig_2.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };

        db.add_order(&order2).await.unwrap();
        aggregator.aggregate().await.unwrap();
        assert!(logs_contain("journal size target hit 40 >= 30"));

        let (_, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert_eq!(batch.orders.len(), 2);
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_expired_orders() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());

        let aggregator_service = AggregatorService::new(
            db.clone(),
            1,
            Digest::ZERO,
            Digest::ZERO,
            Address::ZERO,
            Address::ZERO,
            config,
            prover,
        )
        .await
        .unwrap();

        let current_time = crate::now_timestamp();

        // Add orders to DB first so we can mark them as failed
        let expired_order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: ProofRequest::new(
                RequestId::new(Address::ZERO, 999),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "h/risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    rampUpStart: 0,
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockCollateral: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: Some(current_time - 100),
            client_sig: Bytes::new(),
            lock_price: Some(U256::from(1)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&expired_order).await.unwrap();

        let valid_order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: ProofRequest::new(
                RequestId::new(Address::ZERO, 1000),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "http://risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    rampUpStart: 0,
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockCollateral: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: Some(current_time + 100),
            client_sig: Bytes::new(),
            lock_price: Some(U256::from(1)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        db.add_order(&valid_order).await.unwrap();

        // Create test orders with the correct IDs
        let orders = vec![
            AggregationOrder {
                order_id: expired_order.id(),
                proof_id: "proof1".to_string(),
                expiration: current_time - 100,
                fee: U256::from(10),
            },
            AggregationOrder {
                order_id: valid_order.id(),
                proof_id: "proof2".to_string(),
                expiration: current_time + 100,
                fee: U256::from(20),
            },
        ];

        // Filter expired orders
        let valid_orders =
            aggregator_service.filter_expired_orders(orders, current_time).await.unwrap();

        // Should only have one valid order
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, valid_order.id());

        // Check that expired order was marked as failed
        let db_expired_order = db.get_order(&expired_order.id()).await.unwrap().unwrap();
        assert_eq!(db_expired_order.status, OrderStatus::Failed);
        assert_eq!(db_expired_order.error_msg, Some("Expired before aggregation".to_string()));

        // Check that valid order is unchanged
        let db_valid_order = db.get_order(&valid_order.id()).await.unwrap().unwrap();
        assert_eq!(db_valid_order.status, OrderStatus::PendingAgg);
        assert!(db_valid_order.error_msg.is_none());
    }
}
