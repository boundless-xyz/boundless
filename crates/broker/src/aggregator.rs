// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use alloy::{
    network::Ethereum,
    primitives::{utils, Address, U256},
    providers::Provider,
    transports::BoxTransport,
};
use anyhow::{Context, Result};
use boundless_assessor::{AssessorInput, Fulfillment};
use boundless_market::contracts::eip712_domain;
use chrono::Utc;
use risc0_aggregation::GuestState;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    ReceiptClaim,
};

use crate::{
    chain_monitor::ChainMonitorService,
    config::ConfigLock,
    db::{AggregationOrder, DbObj},
    provers::{self, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    AggregationState, Batch,
};

#[derive(Clone)]
pub struct AggregatorService<P> {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    chain_monitor: Arc<ChainMonitorService<P>>,
    block_time: u64,
    set_builder_guest_id: Digest,
    assessor_guest_id: Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
}

impl<P> AggregatorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    pub async fn new(
        db: DbObj,
        provider: Arc<P>,
        chain_monitor: Arc<ChainMonitorService<P>>,
        set_builder_guest_id: Digest,
        set_builder_guest: Vec<u8>,
        assessor_guest_id: Digest,
        assessor_guest: Vec<u8>,
        market_addr: Address,
        prover_addr: Address,
        config: ConfigLock,
        prover: ProverObj,
        block_time: u64,
    ) -> Result<Self> {
        prover
            .upload_image(&set_builder_guest_id.to_string(), set_builder_guest)
            .await
            .context("Failed to upload set-builder guest")?;

        prover
            .upload_image(&assessor_guest_id.to_string(), assessor_guest)
            .await
            .context("Failed to upload assessor guest")?;

        let chain_id = provider.get_chain_id().await?;

        Ok(Self {
            db,
            config,
            chain_monitor,
            block_time,
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
        aggregation_state: Option<AggregationState>,
        proofs: &[String],
        finalize: bool,
    ) -> Result<AggregationState> {
        // TODO(mmr): Handle failure to get an individual order.
        let mut claims = Vec::<ReceiptClaim>::with_capacity(proofs.len());
        for proof_id in proofs {
            let receipt = self
                .prover
                .get_receipt(&proof_id)
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
            .as_ref()
            .map_or(GuestState::initial(self.set_builder_guest_id), |s| s.guest_state.clone())
            .into_input(claims.clone(), finalize)
            .context("Failed to build set builder input")?;

        // Gather the proof IDs for the assumptions we will need: any pending proofs, and the proof
        // for the current aggregation state.
        let assumption_ids: Vec<String> = aggregation_state
            .as_ref()
            .map(|s| s.proof_id.clone())
            .into_iter()
            .chain(proofs.iter().cloned())
            .collect();

        let input_data = provers::encode_input(&input)
            .with_context(|| format!("Failed to encode set-builder proof input"))?;
        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .with_context(|| format!("failed to upload set-builder input"))?;

        // TODO: we should run this on a different stream in the prover
        // aka make a few different priority streams for each level of the proving

        // TODO: Need to set a timeout here to handle stuck or even just alert on delayed proving if
        // the proving cluster is overloaded

        tracing::info!("Starting proving of set-builder");
        let proof_res = self
            .prover
            .prove_and_monitor_stark(
                &self.set_builder_guest_id.to_string(),
                &input_id,
                assumption_ids,
            )
            .await
            .with_context(|| format!("Failed to prove set-builder"))?;
        tracing::info!(
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
            .as_ref()
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

    async fn prove_assessor(&self, order_ids: &[U256]) -> Result<String> {
        let mut fills = vec![];
        let mut assumptions = vec![];

        for order_id in order_ids {
            let order = self
                .db
                .get_order(*order_id)
                .await
                .with_context(|| format!("Failed to get DB order ID {order_id:x}"))?
                .with_context(|| format!("order ID {order_id:x} missing from DB"))?;

            let proof_id = order
                .proof_id
                .with_context(|| format!("Missing proof_id for order: {order_id:x}"))?;

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
                require_payment: true,
            })
        }

        let order_count = fills.len();
        let input = AssessorInput {
            fills,
            domain: eip712_domain(self.market_addr, self.chain_id),
            prover_address: self.prover_addr,
        };
        let input_data = input.to_vec();

        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .context("Failed to upload assessor input")?;

        let proof_res = self
            .prover
            .prove_and_monitor_stark(&self.assessor_guest_id.to_string(), &input_id, assumptions)
            .await
            .with_context(|| format!("Failed to prove assesor stark"))?;

        tracing::info!(
            "Assessor proof completed, count: {} cycles: {} time: {}",
            order_count,
            proof_res.stats.total_cycles,
            proof_res.elapsed_time
        );

        Ok(proof_res.id)
    }

    /// Check if we should finalize the batch
    ///
    /// Checks current min-deadline, batch timer, and current block.
    async fn check_finalize(
        &mut self,
        batch: &Batch,
        pending_orders: &[AggregationOrder],
    ) -> Result<bool> {
        let (conf_batch_size, conf_batch_time, conf_batch_fees) = {
            let config = self.config.lock_all().context("Failed to lock config")?;

            // TODO: Move this parse into config
            let batch_max_fees = match config.batcher.batch_max_fees.as_ref() {
                Some(elm) => {
                    Some(utils::parse_ether(elm).context("Failed to parse batch max fees")?)
                }
                None => None,
            };
            (config.batcher.batch_size, config.batcher.batch_max_time, batch_max_fees)
        };

        // Skip finalization checks if we have nothing in this batch
        let is_initial_state =
            batch.aggregation_state.as_ref().map(|s| s.guest_state.is_initial()).unwrap_or(true);
        if is_initial_state && pending_orders.len() == 0 {
            return Ok(false);
        }

        // Finalize the batch whenever it exceeds a target size.
        // Add any pending jobs into the batch along with the finalization run.
        if let Some(batch_target_size) = conf_batch_size {
            if batch.orders.len() + pending_orders.len() >= batch_target_size as usize {
                tracing::info!("Batch size target hit, finalizing");
                return Ok(true);
            }
        }

        // Finalize the batch whenever the current batch exceeds a certain age (e.g. one hour).
        if let Some(batch_time) = conf_batch_time {
            let time_delta = Utc::now() - batch.start_time;
            if time_delta.num_seconds() as u64 >= batch_time {
                tracing::info!(
                    "Batch time limit hit {} - {}, finalizing",
                    time_delta.num_seconds(),
                    batch.start_time
                );
                return Ok(true);
            }
        }

        // Finalize whenever a batch hits the target fee total.
        // TODO(mmr): Count the pending orders towards the target.
        if let Some(batch_target_fees) = conf_batch_fees {
            if batch.fees >= batch_target_fees {
                tracing::info!("Batch fee target hit, finalizing");
                return Ok(true);
            }
        }

        // Finalize whenever a deadline is approaching.
        // TODO(mmr): Count the pending orders towards the deadline.
        let conf_block_deadline_buf = {
            let config = self.config.lock_all().context("Failed to lock config")?;
            config.batcher.block_deadline_buffer_secs
        };
        let block_number = self.chain_monitor.current_block_number().await?;

        if let Some(block_deadline) = batch.block_deadline {
            let remaining_secs = (block_deadline - block_number) * self.block_time;
            let buffer_secs = conf_block_deadline_buf;
            // tracing::info!(
            //     "{:?} {} {} {} {}",
            //     batch.block_deadline,
            //     block_number,
            //     self.block_time,
            //     remaining_secs,
            //     buffer_secs
            // );

            if remaining_secs <= buffer_secs {
                tracing::info!("Batch getting close to deadline {remaining_secs}, finalizing");
                return Ok(true);
            }
        } else {
            tracing::warn!("batch does not yet have a block_deadline");
        };

        Ok(false)
    }

    // TODO: If this gets into a bad state (e.g. a "bad proof" gets included in the batch) this
    // currently has no way of recovering. It will simply keep trying to aggregate the batch over
    // and over again. It would be good to have something like a failure counter or a recovery
    // routine that attempts to right the system if it goes sideways.
    async fn aggregate_proofs(&mut self) -> Result<()> {
        // Get the current batch. This aggregator service works on one batch at a time, including
        // any proofs ready for aggregation into the current batch.
        let batch_id =
            self.db.get_current_batch().await.context("Failed to get current batch ID")?;
        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        // Fetch all proofs that are pending aggregation from the DB.
        let new_proofs = self
            .db
            .get_aggregation_proofs()
            .await
            .context("Failed to get pending agg proofs from DB")?;

        // Finalize the current batch before adding any new orders if the finalization conditions
        // are already met.
        let finalize = self.check_finalize(&batch, &new_proofs).await?;

        // If we don't need to finalize, and there are no new proofs, there is no work to do.
        if !finalize && new_proofs.is_empty() {
            tracing::debug!("No aggregation work to do for {batch_id}");
            return Ok(());
        }

        // TODO(mmr): If the aggregation state is finalized already, skip adding new proofs to the
        // batch. This can happen if the groth16 compression fails.

        let assessor_proof_id = if finalize {
            let assessor_order_ids: Vec<U256> =
                batch.orders.iter().copied().chain(new_proofs.iter().map(|p| p.order_id)).collect();

            tracing::debug!(
                "Running assessor for {batch_id} with orders {:x?}",
                assessor_order_ids
            );

            let assessor_proof_id =
                self.prove_assessor(&assessor_order_ids).await.with_context(|| {
                    format!("Failed to prove assessor with orders {:?}", assessor_order_ids)
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
            .prove_set_builder(batch.aggregation_state, &proof_ids, finalize)
            .await
            .context("Failed to prove set builder for batch {batch_id}")?;

        tracing::info!("Completed aggregation into batch {batch_id} of proofs {:x?}", proof_ids);

        let assessor_claim_digest = if let Some(proof_id) = assessor_proof_id {
            let receipt = self
                .prover
                .get_receipt(&proof_id)
                .await
                .with_context(|| format!("Failed to get proof receipt for {proof_id}"))?
                .with_context(|| format!("Proof receipt not found for {proof_id}"))?;
            let claim = receipt
                .claim()
                .with_context(|| format!("Receipt for {proof_id} missing claim"))?
                .value()
                .with_context(|| format!("Receipt for {proof_id} claims pruned"))?;
            Some(claim.digest())
        } else {
            None
        };

        self.db
            .update_batch(batch_id, &aggregation_state, &new_proofs, assessor_claim_digest)
            .await
            .with_context(|| format!("Failed to update batch {batch_id} in the DB"))?;

        if finalize {
            tracing::info!("Starting groth16 compression proof for batch {batch_id}");
            let compress_proof_id = self
                .prover
                .compress(&aggregation_state.proof_id)
                .await
                .context("Failed to complete compression")?;
            tracing::info!("Completed groth16 compression");

            self.db
                .complete_batch(batch_id, compress_proof_id)
                .await
                .context("Failed to set batch as complete")?;
        }

        Ok(())
    }
}

impl<P> RetryTask for AggregatorService<P>
where
    P: Provider<BoxTransport, Ethereum> + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let mut self_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting Aggregator service");
            loop {
                let conf_poll_time_ms = {
                    let config = self_clone
                        .config
                        .lock_all()
                        .context("Failed to lock config")
                        .map_err(SupervisorErr::Fault)?;
                    config.batcher.batch_poll_time_ms.unwrap_or(1000)
                };

                self_clone.aggregate_proofs().await.map_err(SupervisorErr::Recover)?;
                tokio::time::sleep(tokio::time::Duration::from_millis(conf_poll_time_ms)).await;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        db::SqliteDb,
        provers::{encode_input, MockProver},
        BatchStatus, Order, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{Keccak256, B256, U256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID};
    use guest_set_builder::{SET_BUILDER_ELF, SET_BUILDER_ID};
    use guest_util::{ECHO_ELF, ECHO_ID};
    use tracing_test::traced_test;

    /* TODO(victor)
    #[tokio::test]
    #[traced_test]
    async fn set_order_path() {
        fn check_merkle_path(n: u8) {
            let mut leaves = Vec::new();
            for i in 0..n {
                let order_id: u32 = (i + 1).into();
                leaves.push(Node::singleton(i.to_string(), U256::from(order_id), [i; 32].into()));
            }

            // compute the Merkle root
            fn hash(a: Digest, b: Digest) -> Digest {
                let mut h = Keccak256::new();
                if a < b {
                    h.update(a);
                    h.update(b);
                } else {
                    h.update(b);
                    h.update(a);
                }
                h.finalize().0.into()
            }
            fn merkle_root(set: &[Node]) -> Node {
                match set {
                    [] => unreachable!(),
                    [n] => n.clone(),
                    _ => {
                        let (a, b) = set.split_at(set.len().next_power_of_two() / 2);
                        let (left, right) = (merkle_root(a), merkle_root(b));
                        let digest = hash(left.root(), right.root());
                        Node::join("join".to_string(), left.height() + 1, left, right, digest)
                    }
                }
            }
            let exp_root = merkle_root(&leaves);

            // verify Merkle path

            let mut outputs = vec![];
            exp_root.get_order_paths(vec![], &mut outputs).unwrap();
            for (i, (_order_id, path)) in outputs.into_iter().enumerate() {
                let root = path.into_iter().fold([i as u8; 32].into(), hash);
                assert_eq!(root, exp_root.root());
            }
        }

        check_merkle_path(1);
        check_merkle_path(2);
        check_merkle_path(5);
        check_merkle_path(128);
        check_merkle_path(255);
    }
    */

    #[tokio::test]
    #[traced_test]
    async fn aggregate_order() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let prover_addr = signer.address();
        let provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.batch_size = Some(2);
        }

        let prover: ProverObj = Arc::new(MockProver::default());

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
        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            Digest::from(SET_BUILDER_ID),
            SET_BUILDER_ELF.to_vec(),
            Digest::from(ASSESSOR_GUEST_ID),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            prover_addr,
            config,
            prover,
            2,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 2;
        // Order 0
        let order_request = ProofRequest::new(
            0,
            &customer_signer.address(),
            Requirements {
                imageId: B256::from_slice(image_id.as_bytes()),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://risczero.com/image".into(),
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
            },
        );

        let client_sig = order_request
            .sign_request(&customer_signer, Address::ZERO, chain_id)
            .await
            .unwrap()
            .as_bytes();

        // let signature = alloy::signers::Signature::try_from(client_sig.as_slice()).unwrap();
        // use alloy::sol_types::SolStruct;
        // let recovered = signature
        //     .recover_address_from_prehash(&order_request.eip712_signing_hash(
        //         &boundless_market::contracts::eip712_domain(
        //             Address::ZERO,
        //             provider.get_chain_id().await.unwrap(),
        //         ),
        //     ))
        //     .unwrap();
        // assert_eq!(recovered, customer_signer.address());

        let order = Order {
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_block: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res_1.id),
            expire_block: Some(100),
            path: None,
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            error_msg: None,
        };
        let order_id = U256::from(order.request.id);
        db.add_order(order_id, order.clone()).await.unwrap();

        // Order 1
        let order_request = ProofRequest::new(
            1,
            &customer_signer.address(),
            Requirements {
                imageId: B256::from_slice(image_id.as_bytes()),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://risczero.com/image".into(),
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
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
            target_block: None,
            request: order_request,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            proof_id: Some(proof_res_2.id),
            expire_block: Some(100),
            path: None,
            client_sig,
            lock_price: Some(U256::from(min_price)),
            error_msg: None,
        };
        let order_id = U256::from(order.request.id);
        db.add_order(order_id, order.clone()).await.unwrap();

        aggregator.aggregate_proofs().await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
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
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.batch_size = Some(2);
            config.batcher.batch_max_fees = Some("0.1".into());
        }

        let prover: ProverObj = Arc::new(MockProver::default());

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

        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            chain_monitor,
            Digest::from(SET_BUILDER_ID),
            SET_BUILDER_ELF.to_vec(),
            Digest::from(ASSESSOR_GUEST_ID),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            prover_addr,
            config,
            prover,
            2,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProofRequest::new(
            0,
            &customer_signer.address(),
            Requirements {
                imageId: B256::from_slice(image_id.as_bytes()),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://risczero.com/image".into(),
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
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
            target_block: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            expire_block: Some(100),
            path: None,
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            error_msg: None,
        };
        let order_id = U256::from(order.request.id);
        db.add_order(order_id, order.clone()).await.unwrap();

        aggregator.aggregate_proofs().await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
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
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer.clone()))
                .on_builtin(&anvil.endpoint())
                .await
                .unwrap(),
        );
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        {
            let mut config = config.load_write().unwrap();
            config.batcher.batch_size = Some(2);
            config.batcher.block_deadline_buffer_secs = 100;
        }

        let prover: ProverObj = Arc::new(MockProver::default());

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

        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            chain_monitor,
            Digest::from(SET_BUILDER_ID),
            SET_BUILDER_ELF.to_vec(),
            Digest::from(ASSESSOR_GUEST_ID),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            signer.address(),
            config.clone(),
            prover,
            2,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProofRequest::new(
            0,
            &customer_signer.address(),
            Requirements {
                imageId: B256::from_slice(image_id.as_bytes()),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://risczero.com/image".into(),
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(min_price),
                maxPrice: U256::from(250000000000000000u64),
                biddingStart: 0,
                timeout: 50,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
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
            target_block: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            expire_block: Some(100),
            path: None,
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            error_msg: None,
        };
        let order_id = U256::from(order.request.id);
        db.add_order(order_id, order.clone()).await.unwrap();

        provider.anvil_mine(Some(U256::from(51)), Some(U256::from(2))).await.unwrap();

        aggregator.aggregate_proofs().await.unwrap();

        let db_order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let (_batch_id, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert!(!batch.orders.is_empty());
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
        assert!(logs_contain("Batch getting close to deadline"));
    }
}
