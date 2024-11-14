// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{marker::PhantomData, sync::Arc};

use aggregation_set::{GuestInput, GuestOutput};
use alloy::{
    network::Ethereum,
    primitives::{utils, Address, U256},
    providers::Provider,
    sol_types::SolValue,
    transports::Transport,
};
use anyhow::{Context, Result};
use assessor::{AssessorInput, Fulfillment};
use boundless_market::contracts::eip712_domain;
use chrono::Utc;
use risc0_zkvm::sha::Digest;

use crate::{
    config::ConfigLock,
    db::DbObj,
    provers::{self, ProverObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    Node,
};

#[derive(Clone)]
pub struct AggregatorService<T, P> {
    db: DbObj,
    config: ConfigLock,
    prover: ProverObj,
    provider: Arc<P>,
    block_time: u64,
    set_builder_guest_id: Digest,
    assessor_guest_id: Digest,
    market_addr: Address,
    prover_addr: Address,
    chain_id: u64,
    _phantom_t: PhantomData<T>,
}

impl<T, P> AggregatorService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
{
    pub async fn new(
        db: DbObj,
        provider: Arc<P>,
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
            provider,
            block_time,
            prover,
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
            _phantom_t: Default::default(),
        })
    }

    async fn prove_sot(
        &self,
        job_type: &str,
        input: &impl serde::Serialize,
        assumptions: Vec<String>,
    ) -> Result<(String, GuestOutput)> {
        let input_data = provers::encode_input(&input)
            .with_context(|| format!("Failed to encode {job_type} proof input"))?;
        let input_id = self
            .prover
            .upload_input(input_data)
            .await
            .with_context(|| format!("failed to upload {job_type} input"))?;

        // TODO: we should run this on a different stream in the prover
        // aka make a few different priority streams for each level of the proving

        // TODO: Need to set a timeout here to handle stuck or even just alert on delayed proving if
        // the proving cluster is overloaded

        tracing::info!("Starting proving of {job_type}");
        let proof_res = self
            .prover
            .prove_and_monitor_stark(&self.set_builder_guest_id.to_string(), &input_id, assumptions)
            .await
            .with_context(|| format!("Failed to prove {job_type}"))?;
        tracing::info!("completed proving of {job_type} cycles: {}", proof_res.stats.total_cycles);

        let journal = self
            .prover
            .get_journal(&proof_res.id)
            .await
            .with_context(|| format!("Failed to get {job_type} journal"))?
            .with_context(|| format!("{job_type} journal missing"))?;

        Ok((
            proof_res.id,
            GuestOutput::abi_decode(&journal, false).context("Failed to decode guest output")?,
        ))
    }

    async fn prove_singleton(&self, order_id: U256, proof_id: String) -> Result<Node> {
        let receipt = self
            .prover
            .get_receipt(&proof_id)
            .await
            .context("Failed to get proof receipt")?
            .context("Proof receipt not found")?;
        let claim = receipt
            .claim()
            .context("Receipt missing claims")?
            .value()
            .context("Receipt claims pruned")?;

        let input = GuestInput::Singleton { self_image_id: self.set_builder_guest_id, claim };

        let (new_id, output) = self.prove_sot("singleton", &input, vec![proof_id]).await?;
        tracing::debug!("Singleton(order={order_id:x}): {}", output.root());

        Ok(Node::singleton(new_id, order_id, output.root()))
    }

    async fn prove_join(&self, left: Node, right: Node) -> Result<Node> {
        let input = GuestInput::Join {
            self_image_id: self.set_builder_guest_id,
            left_set_root: left.root(),
            right_set_root: right.root(),
        };

        let (new_id, output) = self
            .prove_sot(
                "join",
                &input,
                vec![left.proof_id().to_string(), right.proof_id().to_string()],
            )
            .await?;

        tracing::debug!(
            "Join(left={}, right={}, root={})",
            left.root(),
            right.root(),
            output.root()
        );

        Ok(Node::join(new_id, left.height() + 1, left, right, output.root()))
    }

    async fn prove_assessor(&self, order_ids: &[U256], root_proof_id: &str) -> Result<String> {
        let mut fills = vec![];

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
            .prove_and_monitor_stark(
                &self.assessor_guest_id.to_string(),
                &input_id,
                vec![root_proof_id.to_string()],
            )
            .await
            .with_context(|| format!("Failed to prove assesor stark"))?;

        tracing::info!("Assessor proof completed, cycles {}", proof_res.stats.total_cycles);

        Ok(proof_res.id)
    }

    /// Adds a new order to the current aggregation batch.
    async fn aggregate_proof(
        &mut self,
        batch_id: usize,
        order_id: U256,
        proof_id: String,
    ) -> Result<()> {
        tracing::info!("Aggregating order {order_id:x} into batch {batch_id}");

        // Run singleton proof:
        let mut node =
            self.prove_singleton(order_id, proof_id).await.context("Failed to prove singleton")?;

        let mut peaks =
            self.db.get_batch_peaks(batch_id).await.context("Failed to get DB batch peaks")?;

        while let Some(peak) = peaks.pop() {
            if peak.height() == node.height() {
                node = self.prove_join(peak, node).await.context("Failed to prove join")?;
            } else {
                peaks.push(peak);
                break;
            }
        }

        peaks.push(node);

        self.db.set_batch_peaks(batch_id, peaks).await.context("Failed to set batch peaks")?;

        Ok(())
    }

    async fn finalize_batch(&mut self, batch_id: usize) -> Result<()> {
        let mut peaks =
            self.db.get_batch_peaks(batch_id).await.context("Failed to get DB batch peaks")?;

        if peaks.is_empty() {
            tracing::warn!("Attempted to finalize empty batch");
            return Ok(());
        }
        tracing::info!("Finalizing batch {batch_id}");

        while peaks.len() >= 2 {
            let right = peaks.pop().unwrap();
            let left = peaks.pop().unwrap();
            let node = self.prove_join(left, right).await.context("Failed to prove join")?;
            tracing::debug!("Join: {}", node.root());

            peaks.push(node);
        }

        // exactly one node left
        // unwrap here since it should be basically an assert if this fails
        let root = peaks.last().unwrap().clone();

        self.db
            .set_batch_peaks(batch_id, peaks)
            .await
            .context("Failed to set batch peaks after joins")?;

        let orders_root = root.root();

        // prove the assessor for the batch
        tracing::info!("Starting batch assessor proof, root: {}", root.root());
        let batch_order_ids = root.order_ids();
        let assessor_proof_id = self
            .prove_assessor(&batch_order_ids, root.proof_id())
            .await
            .context("Failed to prove assessor")?;

        // now prove a singleton proof of the assessor
        let assessor_singleton = self
            .prove_singleton(U256::ZERO /* TODO ??!?! */, assessor_proof_id)
            .await
            .context("Failed to prove singleton of assessor")?;

        tracing::info!("Assessor merkle node: {}", assessor_singleton.root());
        let batch_root = self
            .prove_join(
                root,
                Node::singleton(
                    assessor_singleton.proof_id().to_string(),
                    U256::ZERO,
                    assessor_singleton.root(),
                ),
            )
            .await
            .context("Failed to prove batch root join")?;

        tracing::info!("Starting groth16 compression proof");
        let compress_proof_id = self
            .prover
            .compress(batch_root.proof_id())
            .await
            .context("Failed to complete compression")?;
        tracing::info!("Completed groth16 compression");

        let mut outputs = vec![];
        batch_root
            .get_order_paths(vec![], &mut outputs)
            .context("Failed to assign order path to db order")?;

        for (order_id, path) in outputs {
            self.db.set_order_path(order_id, path).await.context("Failed to set order path")?;
        }

        self.db
            .complete_batch(batch_id, batch_root.root(), orders_root, compress_proof_id)
            .await
            .context("Failed to set batch as complete")?;

        self.db
            .set_batch_peaks(batch_id, vec![])
            .await
            .context("Failed to set batch peaks to empty vec")?;

        Ok(())
    }

    /// Check if we should finalize the batch
    ///
    /// - check current min-deadline and batch timer
    /// - need to fetch current block, might be good to make that a long polling service with a
    ///   Atomic everyone reads
    ///
    /// if so:
    /// - finalize
    /// - snark proof
    /// - insert batch data in to DB for finalizer
    /// - mark all orders in batch as Aggregated
    async fn check_finalize(&mut self, batch_id: usize) -> Result<()> {
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
        let peak_count = self
            .db
            .get_batch_peak_count(batch_id)
            .await
            .context("Failed to get db batch peak count")?;
        if peak_count == 0 {
            return Ok(());
        }

        let mut finalize = false;

        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        if let Some(batch_size) = conf_batch_size {
            if batch.orders.len() >= batch_size as usize {
                tracing::info!("Batch size limit hit, finalizing");
                finalize = true;
            }
        }

        if let Some(batch_time) = conf_batch_time {
            let time_delta = Utc::now() - batch.start_time;
            if time_delta.num_seconds() as u64 >= batch_time {
                tracing::info!(
                    "Batch time limit hit {} - {}, finalizing",
                    time_delta.num_seconds(),
                    batch.start_time
                );
                finalize = true
            }
        }

        if let Some(batch_max_fees) = conf_batch_fees {
            if batch.fees >= batch_max_fees {
                tracing::info!("Batch max fee limit hit, finalizing");
                finalize = true;
            }
        }

        if !finalize {
            let conf_block_deadline_buf = {
                let config = self.config.lock_all().context("Failed to lock config")?;
                config.batcher.block_deadline_buffer_secs
            };
            // TODO: this will trigger quite frequently so we should
            // try and move the "current_block" to a atomic managed by a sub-service
            let block_number =
                self.provider.get_block_number().await.context("Failed to get current block")?;

            let Some(block_deadline) = batch.block_deadline else {
                tracing::warn!("batch does not yet have a block_deadline");
                return Ok(());
            };

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
                finalize = true;
            }
        }

        if finalize {
            self.finalize_batch(batch_id).await.context("Failed to finalize batch")?;
        }

        Ok(())
    }

    async fn aggregate_proofs(&mut self) -> Result<()> {
        let new_proofs = self
            .db
            .get_aggregation_proofs()
            .await
            .context("Failed to get pending agg proofs from DB")?;

        let batch_id = self.db.get_current_batch().await.context("Failed to get current batch")?;

        self.check_finalize(batch_id).await?;

        if new_proofs.is_empty() {
            return Ok(());
        }

        for agg_proof in new_proofs {
            match self.aggregate_proof(batch_id, agg_proof.order_id, agg_proof.proof_id).await {
                Ok(_) => {
                    tracing::info!("Completed aggregation of proof {:x}", agg_proof.order_id);
                    self.db
                        .update_batch(
                            batch_id,
                            agg_proof.order_id,
                            agg_proof.expire_block,
                            agg_proof.fee,
                        )
                        .await
                        .context("Failed to update batch with new order details")?;
                }
                Err(err) => {
                    tracing::error!(
                        "Failed to complete aggregation of proof {:x} {err:?}",
                        agg_proof.order_id
                    );
                    if let Err(db_err) =
                        self.db.set_order_failure(agg_proof.order_id, format!("{err:?}")).await
                    {
                        tracing::error!("Failed to mark order failure in db: {db_err}");
                    }
                }
            }
        }

        self.check_finalize(batch_id).await?;

        Ok(())
    }
}

impl<T, P> RetryTask for AggregatorService<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
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
    use aggregation_set::{SET_BUILDER_GUEST_ELF, SET_BUILDER_GUEST_ID};
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{aliases::U96, Keccak256, B256},
        providers::{ext::AnvilApi, ProviderBuilder},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProvingRequest, Requirements,
    };
    use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID};
    use guest_util::{ECHO_ELF, ECHO_ID};
    use tracing_test::traced_test;

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
                .on_http(anvil.endpoint().parse().unwrap()),
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

        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            Digest::from(SET_BUILDER_GUEST_ID),
            SET_BUILDER_GUEST_ELF.to_vec(),
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
        let order_request = ProvingRequest::new(
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
                minPrice: U96::from(min_price),
                maxPrice: U96::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
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
        let order_request = ProvingRequest::new(
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
                minPrice: U96::from(min_price),
                maxPrice: U96::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
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
                .on_http(anvil.endpoint().parse().unwrap()),
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

        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            Digest::from(SET_BUILDER_GUEST_ID),
            SET_BUILDER_GUEST_ELF.to_vec(),
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
        let order_request = ProvingRequest::new(
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
                minPrice: U96::from(min_price),
                maxPrice: U96::from(250000000000000000u64),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
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
                .on_http(anvil.endpoint().parse().unwrap()),
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

        let mut aggregator = AggregatorService::new(
            db.clone(),
            provider.clone(),
            Digest::from(SET_BUILDER_GUEST_ID),
            SET_BUILDER_GUEST_ELF.to_vec(),
            Digest::from(ASSESSOR_GUEST_ID),
            ASSESSOR_GUEST_ELF.to_vec(),
            Address::ZERO,
            signer.address(),
            config,
            prover,
            2,
        )
        .await
        .unwrap();

        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let chain_id = provider.get_chain_id().await.unwrap();

        let min_price = 200000000000000000u64;
        let order_request = ProvingRequest::new(
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
                minPrice: U96::from(min_price),
                maxPrice: U96::from(250000000000000000u64),
                biddingStart: 0,
                timeout: 50,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
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
