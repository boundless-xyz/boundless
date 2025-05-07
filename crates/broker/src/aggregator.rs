// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::{utils, Address};
use anyhow::{Context, Result};
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::{contracts::eip712_domain, input::InputBuilder};
use chrono::Utc;
use risc0_aggregation::GuestState;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::{AggregationOrder, DbObj},
    errors::CodedError,
    impl_coded_debug, now_timestamp,
    provers::{self, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    AggregationState, Batch, BatchStatus,
};
use thiserror::Error;

#[derive(Error)]
pub enum AggregatorErr {
    #[error("{code} Unexpected error: {0}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl_coded_debug!(AggregatorErr);

impl CodedError for AggregatorErr {
    fn code(&self) -> &str {
        match self {
            AggregatorErr::UnexpectedErr(_) => "[B-AGG-500]",
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

    async fn prove_set_builder(
        &self,
        aggregation_state: Option<&AggregationState>,
        proofs: &[String],
        finalize: bool,
    ) -> Result<AggregationState> {
        // TODO(#268): Handle failure to get an individual order.
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        for proof_id in proofs {
            let receipt = self
                .prover
                .get_receipt(proof_id)
                .await
                .with_context(|| format!("Failed to get proof receipt for {proof_id}"))?
                .with_context(|| format!("Proof receipt not found for {proof_id}"))?;
            let claim = receipt
                .claim()
                .with_context(|| format!("Receipt for {proof_id} missing claim"))?
                .value()
                .with_context(|| format!("Receipt for {proof_id} claims pruned"))?;
            claims.push(claim);
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
            .chain(proofs.iter().cloned())
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

        tracing::debug!("Starting proving of set-builder");
        let proof_res = self
            .prover
            .prove_and_monitor_stark(
                &self.set_builder_guest_id.to_string(),
                &input_id,
                assumption_ids,
            )
            .await
            .context("Failed to prove set-builder")?;
        tracing::debug!(
            "completed proving of set-builder cycles: {} time: {}",
            proof_res.stats.total_cycles,
            proof_res.elapsed_time
        );

        let journal = self
            .prover
            .get_journal(&proof_res.id)
            .await
            .with_context(|| format!("Failed to get set-builder journal from {}", proof_res.id))?
            .with_context(|| format!("set-builder journal missing from {}", proof_res.id))?;

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
        let mut assumptions = vec![];

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

            assumptions.push(proof_id.clone());

            let journal = self
                .prover
                .get_journal(&proof_id)
                .await
                .with_context(|| format!("Failed to get {proof_id} journal"))?
                .with_context(|| format!("{proof_id} journal missing"))?;

            fills.push(Fulfillment {
                request: order.request.clone(),
                signature: order.client_sig.clone().to_vec(),
                journal,
            })
        }

        let order_count = fills.len();
        let input = AssessorInput {
            fills,
            domain: eip712_domain(self.market_addr, self.chain_id),
            prover_address: self.prover_addr,
        };
        let stdin = InputBuilder::new().write_frame(&input.encode()).stdin;

        let input_id =
            self.prover.upload_input(stdin).await.context("Failed to upload assessor input")?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark(&self.assessor_guest_id.to_string(), &input_id, assumptions)
            .await
            .context("Failed to prove assesor stark")?;

        tracing::debug!(
            "Assessor proof completed, count: {} cycles: {} time: {}",
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

            journal_size += journal.len();
        }

        Ok(journal_size)
    }

    /// Check if we should finalize the batch
    ///
    /// Checks current min-deadline, batch timer, and current block.
    async fn check_finalize(
        &mut self,
        batch_id: usize,
        batch: &Batch,
        pending_orders: &[AggregationOrder],
    ) -> Result<bool> {
        let (conf_batch_size, conf_batch_time, conf_batch_fees, conf_max_journal_bytes) = {
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
            )
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
        if let Some(batch_target_size) = conf_batch_size {
            if batch_size >= batch_target_size as usize {
                tracing::debug!(
                    "Finalizing batch {batch_id}: size target hit {} - {}",
                    batch_size,
                    batch_target_size
                );
                return Ok(true);
            } else {
                tracing::debug!(
                    "Batch {batch_id} below size target hit {} - {}",
                    batch_size,
                    batch_target_size
                );
            }
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
        if let Some(batch_time) = conf_batch_time {
            let time_delta = Utc::now() - batch.start_time;
            if time_delta.num_seconds() as u64 >= batch_time {
                tracing::debug!(
                    "Finalizing batch {batch_id}: time limit hit {} - {}",
                    time_delta.num_seconds(),
                    batch.start_time
                );
                return Ok(true);
            } else {
                tracing::debug!("Batch {batch_id} below time limit");
            }
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

    async fn aggregate_proofs(
        &mut self,
        batch_id: usize,
        batch: &Batch,
        new_proofs: &[AggregationOrder],
        groth16_proofs: &[AggregationOrder],
        finalize: bool,
    ) -> Result<String> {
        let assessor_proof_id = if finalize {
            let assessor_order_ids: Vec<String> = batch
                .orders
                .iter()
                .chain(new_proofs.iter().map(|p| &p.order_id))
                .chain(groth16_proofs.iter().map(|p| &p.order_id))
                .cloned()
                .collect();

            tracing::debug!(
                "Running assessor for batch {batch_id} with orders {:x?}",
                assessor_order_ids
            );

            let assessor_proof_id =
                self.prove_assessor(&assessor_order_ids).await.with_context(|| {
                    format!("Failed to prove assessor with orders {:x?}", assessor_order_ids)
                })?;

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

        tracing::debug!("Running set builder for {batch_id} with proofs {:x?}", proof_ids);
        let aggregation_state = self
            .prove_set_builder(batch.aggregation_state.as_ref(), &proof_ids, finalize)
            .await
            .context("Failed to prove set builder for batch {batch_id}")?;

        tracing::debug!("Completed aggregation into batch {batch_id} of proofs {:x?}", proof_ids);

        self.db
            .update_batch(
                batch_id,
                &aggregation_state,
                &[new_proofs, groth16_proofs].concat(),
                assessor_proof_id,
            )
            .await
            .with_context(|| format!("Failed to update batch {batch_id} in the DB"))?;

        Ok(aggregation_state.proof_id)
    }

    async fn aggregate(&mut self) -> Result<(), AggregatorErr> {
        // Get the current batch. This aggregator service works on one batch at a time, including
        // any proofs ready for aggregation into the current batch.
        let batch_id = self.db.get_current_batch().await.context("Failed to get current batch")?;
        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        let (aggregation_proof_id, compress) = match batch.status {
            BatchStatus::Aggregating => {
                // Fetch all proofs that are pending aggregation from the DB.
                let new_proofs = self
                    .db
                    .get_aggregation_proofs()
                    .await
                    .context("Failed to get aggregation proofs")?;
                // Fetch all groth16 proofs that are ready to be submitted from the DB.
                let new_groth16_proofs =
                    self.db.get_groth16_proofs().await.context("Failed to get groth16 proofs")?;

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
                    .await?;
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
            tracing::debug!("Starting groth16 compression proof for batch {batch_id}");
            let compress_proof_id = self
                .prover
                .compress(&aggregation_proof_id)
                .await
                .context("Failed to complete compression")?;
            tracing::debug!("Completed groth16 compression for batch {batch_id}");

            self.db
                .complete_batch(batch_id, &compress_proof_id)
                .await
                .context("Failed to set batch as complete")?;
        }

        Ok(())
    }
}

impl RetryTask for AggregatorService {
    type Error = AggregatorErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let mut self_clone = self.clone();

        Box::pin(async move {
            tracing::debug!("Starting Aggregator service");
            loop {
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
        primitives::U256,
        providers::{ext::AnvilApi, Provider, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements,
    };
    use boundless_market_test_utils::{
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
            config.batcher.min_batch_size = Some(2);
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
        let _handle = tokio::spawn(chain_monitor.spawn());
        let chain_id = provider.get_chain_id().await.unwrap();
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let mut aggregator = AggregatorService::new(
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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

        // Second order
        let order_request = ProofRequest::new(
            RequestId::new(customer_signer.address(), 1),
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

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
            config.batcher.min_batch_size = Some(2);
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
        let _handle = tokio::spawn(chain_monitor.spawn());
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let mut aggregator = AggregatorService::new(
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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 1200,
                lockTimeout: 1200,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 1200,
                lockTimeout: 1200,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

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
            config.batcher.min_batch_size = Some(2);
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
        let mut aggregator = AggregatorService::new(
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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                biddingStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

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
            config.batcher.min_batch_size = Some(2);
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

        let _handle = tokio::spawn(chain_monitor.spawn());

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let mut aggregator = AggregatorService::new(
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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                biddingStart: now_timestamp(),
                timeout: 50,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };
        db.add_order(order.clone()).await.unwrap();

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
            config.batcher.min_batch_size = Some(10);
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

        let _handle = tokio::spawn(chain_monitor.spawn());

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let mut aggregator = AggregatorService::new(
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
            Requirements::new(
                image_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                biddingStart: now_timestamp(),
                timeout: 50,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
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
        };

        // add first order and aggregate
        db.add_order(order.clone()).await.unwrap();
        aggregator.aggregate().await.unwrap();
        assert!(logs_contain("journal size below limit 20 < 30"));

        // batch is not finalized at this point

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
        };

        db.add_order(order2.clone()).await.unwrap();
        aggregator.aggregate().await.unwrap();
        assert!(logs_contain("journal size target hit 40 >= 30"));

        let (_, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert_eq!(batch.orders.len(), 2);
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }
}
