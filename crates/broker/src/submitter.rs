// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{sync::Arc, time::Duration};

use alloy::{
    network::Ethereum,
    primitives::{Address, B256, U256},
    providers::{Provider, WalletProvider},
    sol_types::SolStruct,
    transports::Transport,
};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::{
    boundless_market::BoundlessMarketService, encode_seal, set_verifier::SetVerifierService,
    Fulfillment,
};
use guest_assessor::ASSESSOR_GUEST_ID;
use risc0_aggregation::{SetInclusionReceipt, SetInclusionReceiptVerifierParameters};
use risc0_zkvm::{
    sha::{Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::DbObj,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Batch,
};

#[derive(Clone)]
pub struct Submitter<T, P> {
    db: DbObj,
    prover: ProverObj,
    market: BoundlessMarketService<T, Arc<P>>,
    set_verifier: SetVerifierService<T, Arc<P>>,
    set_builder_img_id: Digest,
    prover_address: Address,
    config: ConfigLock,
}

impl<T, P> Submitter<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + WalletProvider + 'static + Clone,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        prover: ProverObj,
        provider: Arc<P>,
        set_verifier_addr: Address,
        market_addr: Address,
        set_builder_img_id: Digest,
    ) -> Result<Self> {
        let txn_timeout_opt = {
            let config = config.lock_all().context("Failed to read config")?;
            config.batcher.txn_timeout
        };

        let mut market = BoundlessMarketService::new(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        if let Some(txn_timeout) = txn_timeout_opt {
            market = market.with_timeout(Duration::from_secs(txn_timeout));
        }

        let mut set_verifier = SetVerifierService::new(
            set_verifier_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        if let Some(txn_timeout) = txn_timeout_opt {
            set_verifier = set_verifier.with_timeout(Duration::from_secs(txn_timeout));
        }

        let prover_address = provider.default_signer_address();

        Ok(Self { db, prover, market, set_verifier, set_builder_img_id, prover_address, config })
    }

    async fn fetch_encode_g16(&self, g16_proof_id: &str) -> Result<Vec<u8>> {
        let groth16_receipt = self
            .prover
            .get_compressed_receipt(g16_proof_id)
            .await
            .context("Failed to fetch g16 receipt")?
            .context("Groth16 receipt missing")?;

        let groth16_receipt: Receipt =
            bincode::deserialize(&groth16_receipt).context("Failed to deserialize g16 receipt")?;

        let encoded_seal =
            encode_seal(&groth16_receipt).context("Failed to encode g16 receipt seal")?;

        Ok(encoded_seal)
    }

    pub async fn submit_batch(&self, batch_id: usize, batch: &Batch) -> Result<()> {
        tracing::info!("Submitting batch {batch_id}");

        // Collect the needed parts for the new merkle root:
        let batch_seal = self.fetch_encode_g16(&batch.groth16_proof_id).await?;
        let batch_root = batch.root.context("Batch missing root digest")?;
        let root = B256::from_slice(batch_root.as_bytes());

        // Collect the needed parts for the fulfillBatch:
        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_img_id };

        let mut fulfillments = vec![];
        for order_id in batch.orders.iter() {
            tracing::info!("Submitting order {order_id:x}");

            let (order_request, order_proof_id, order_img_id, order_path) = match self
                .db
                .get_submission_order(*order_id)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!(
                        "Failed to order {order_id:x} path from DB, order NOT finalized: {err:?}"
                    );
                    if let Err(db_err) =
                        self.db.set_order_failure(*order_id, format!("{err:?}")).await
                    {
                        tracing::error!("Failed to set order failure during proof submission: {order_id:x} {db_err:?}");
                        continue;
                    }
                    continue;
                }
            };

            let order_journal = match self.prover.get_journal(&order_proof_id).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!("Failed to order journal {order_id:x} from prover: {err:?}");
                    if let Err(db_err) =
                        self.db.set_order_failure(*order_id, format!("{err:?}")).await
                    {
                        tracing::error!("Failed to set order failure during proof submission: {order_id:x} {db_err:?}");
                    }
                    continue;
                }
            };

            let Some(order_journal) = order_journal else {
                tracing::error!("Order proof Journal missing");
                if let Err(db_err) =
                    self.db.set_order_failure(*order_id, "Order proof Journal missing".into()).await
                {
                    tracing::error!("Failed to set order failure during proof submission: {order_id:x} {db_err:?}");
                }
                continue;
            };

            tracing::debug!("Order path {order_id:x} - {order_path:x?}");
            let mut set_inclusion_receipt = SetInclusionReceipt::from_path(
                ReceiptClaim::ok(order_img_id.0, MaybePruned::Pruned(order_journal.digest())),
                order_path,
            );
            set_inclusion_receipt.verifier_parameters = inclusion_params.digest();
            let seal = match set_inclusion_receipt
                .abi_encode_seal()
                .context("ABI encode set inclusion receipt")
            {
                Ok(seal) => seal,
                Err(err) => {
                    tracing::error!("Failed to encode seal for {order_id:x}: {err:?}");
                    if let Err(db_err) =
                        self.db.set_order_failure(*order_id, format!("{err:?}")).await
                    {
                        tracing::error!("Failed to set order failure during proof submission: {order_id:x} {db_err:?}");
                    }
                    continue;
                }
            };

            let request_digest = order_request
                .eip712_signing_hash(&self.market.eip712_domain().await?.alloy_struct());
            fulfillments.push(Fulfillment {
                id: *order_id,
                requestDigest: request_digest,
                imageId: order_img_id,
                journal: order_journal.into(),
                seal: seal.into(),
                requirePayment: true,
            });
        }

        let orders_root = batch.orders_root.context("Batch missing orders root digest")?;
        let mut assessor_seal = SetInclusionReceipt::from_path(
            // TODO: Set inclusion proofs, when ABI encoded, currently don't contain anything
            // derived from the claim. So instead of constructing the journal, we simply use the
            // zero digest. We should either plumb through the data for the assessor journal, or we
            // should make an explicit way to encode an inclusion proof without the claim.
            ReceiptClaim::ok(ASSESSOR_GUEST_ID, MaybePruned::Pruned(Digest::ZERO)),
            vec![orders_root],
        );
        assessor_seal.verifier_parameters = inclusion_params.digest();
        let assessor_seal =
            assessor_seal.abi_encode_seal().context("ABI encode assessor set inclusion receipt")?;

        let single_txn_fulfill = {
            let config = self.config.lock_all().context("Failed to read config")?;
            config.batcher.single_txn_fulfill
        };

        if single_txn_fulfill {
            if let Err(err) = self
                .market
                .submit_merkle_and_fulfill(
                    root,
                    batch_seal.into(),
                    fulfillments.clone(),
                    assessor_seal.into(),
                    self.prover_address,
                )
                .await
            {
                tracing::error!("Failed to submit proofs: {err:?} for batch {batch_id}");
                for fulfillment in fulfillments.iter() {
                    if let Err(db_err) = self
                        .db
                        .set_order_failure(U256::from(fulfillment.id), format!("{err:?}"))
                        .await
                    {
                        tracing::error!(
                            "Failed to set order failure during proof submission: {:x} {db_err:?}",
                            fulfillment.id
                        );
                    }
                }
                bail!("transaction to fulfill batch failed");
            }
        } else {
            let contains_root = match self.set_verifier.contains_root(root).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::error!("Failed to query if set-verifier contains the new root, trying to submit anyway {err:?}");
                    false
                }
            };
            if !contains_root {
                tracing::info!("Submitting app merkle root: {root}");
                self.set_verifier
                    .submit_merkle_root(root, batch_seal.into())
                    .await
                    .context("Failed to submit app merkle_root")?;
            } else {
                tracing::info!("Contract already contains root, skipping to fulfillment");
            }

            if let Err(err) = self
                .market
                .fulfill_batch(fulfillments.clone(), assessor_seal.into(), self.prover_address)
                .await
            {
                tracing::error!("Failed to submit proofs: {err:?} for batch {batch_id}");
                for fulfillment in fulfillments.iter() {
                    if let Err(db_err) = self
                        .db
                        .set_order_failure(U256::from(fulfillment.id), format!("{err:?}"))
                        .await
                    {
                        tracing::error!(
                            "Failed to set order failure during proof submission: {:x} {db_err:?}",
                            fulfillment.id
                        );
                    }
                }
                bail!("transaction to fulfill batch failed");
            }
        }

        for fulfillment in fulfillments.iter() {
            if let Err(db_err) = self.db.set_order_complete(U256::from(fulfillment.id)).await {
                tracing::error!(
                    "Failed to set order complete during proof submission: {:x} {db_err:?}",
                    fulfillment.id
                );
                continue;
            }
            tracing::info!("✨ Completed order {:x} ✨", fulfillment.id);
        }

        Ok(())
    }

    pub async fn process_next_batch(&self) -> Result<bool, SupervisorErr> {
        let batch_res = self
            .db
            .get_complete_batch()
            .await
            .context("Failed to check db for complete batch")
            .map_err(SupervisorErr::Recover)?;

        let Some((batch_id, batch)) = batch_res else {
            return Ok(false);
        };

        match self.submit_batch(batch_id, &batch).await {
            Ok(_) => {
                if let Err(db_err) = self.db.set_batch_submitted(batch_id).await {
                    tracing::error!("Failed to set batch submitted status: {db_err:?}");
                    // TODO: Handle error here? / record it?
                    return Err(SupervisorErr::Fault(db_err.into()));
                }
                tracing::info!("Completed batch {batch_id}");
            }
            Err(err) => {
                tracing::error!("Submission of batch {batch_id} failed: {err:?}");
                if let Err(err) = self.db.set_batch_failure(batch_id, format!("{err:?}")).await {
                    tracing::error!("Failed to set batch failure: {batch_id} - {err:?}");
                    return Err(SupervisorErr::Recover(err.into()));
                }
            }
        }

        Ok(true)
    }
}

impl<T, P> RetryTask for Submitter<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + WalletProvider + 'static + Clone,
{
    fn spawn(&self) -> RetryRes {
        let obj_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting Submitter service");
            loop {
                obj_clone.process_next_batch().await?;

                // TODO: configuration
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
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
        Batch, BatchStatus, Order, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{B256, U256},
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
        sol_types::SolValue,
    };
    use assessor::{AssessorInput, Fulfillment};
    use boundless_market::contracts::{
        test_utils::{deploy_boundless_market, deploy_mock_verifier, deploy_set_verifier},
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use chrono::Utc;
    use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID};
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_aggregation::{GuestInput, GuestOutput, SET_BUILDER_ELF, SET_BUILDER_ID};
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    async fn run_submit_batch(config: ConfigLock) {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let prover_addr = signer.address();
        let customer_addr = customer_signer.address();
        tracing::info!("prover: {prover_addr} customer: {customer_addr}");

        let provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer.clone()))
                .on_http(anvil.endpoint().parse().unwrap()),
        );

        let customer_provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .on_http(anvil.endpoint().parse().unwrap()),
        );

        let verifier = deploy_mock_verifier(provider.clone()).await.unwrap();
        let set_verifier = deploy_set_verifier(provider.clone(), verifier).await.unwrap();
        let market_address =
            deploy_boundless_market(&signer, provider.clone(), set_verifier, Some(prover_addr))
                .await
                .unwrap();

        let market = BoundlessMarketService::new(market_address, provider.clone(), prover_addr);
        market.deposit(U256::from(10000000000u64)).await.unwrap();

        let market_customer =
            BoundlessMarketService::new(market_address, customer_provider.clone(), customer_addr);
        market_customer.deposit(U256::from(10000000000u64)).await.unwrap();

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let prover: ProverObj = Arc::new(MockProver::default());

        let echo_id = Digest::from(ECHO_ID);
        let echo_id_str = echo_id.to_string();
        prover.upload_image(&echo_id_str, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        let set_builder_id_str = set_builder_id.to_string();
        prover.upload_image(&set_builder_id_str, SET_BUILDER_ELF.to_vec()).await.unwrap();

        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        let assessor_id_str = assessor_id.to_string();
        prover.upload_image(&assessor_id_str, ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();

        let echo_proof =
            prover.prove_and_monitor_stark(&echo_id_str, &input_id, vec![]).await.unwrap();
        let echo_receipt = prover.get_receipt(&echo_proof.id).await.unwrap().unwrap();

        let order_request = ProofRequest::new(
            market_customer.index_from_nonce().await.unwrap(),
            &customer_addr,
            Requirements {
                imageId: B256::from_slice(echo_id.as_bytes()),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "http://risczero.com/image".into(),
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(2),
                maxPrice: U256::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
            },
        );

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = order_request
            .sign_request(&customer_signer, market_address, chain_id)
            .unwrap()
            .as_bytes();

        let set_builder_input = prover
            .upload_input(
                encode_input(&GuestInput::Singleton {
                    self_image_id: set_builder_id,
                    claim: echo_receipt.claim().unwrap().as_value().unwrap().clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let echo_singleton = prover
            .prove_and_monitor_stark(
                &set_builder_id_str,
                &set_builder_input,
                vec![echo_proof.id.clone()],
            )
            .await
            .unwrap();

        let assessor_input = prover
            .upload_input(
                AssessorInput {
                    domain: boundless_market::contracts::eip712_domain(market_address, chain_id),
                    fills: vec![Fulfillment {
                        request: order_request.clone(),
                        signature: client_sig.into(),
                        journal: echo_receipt.journal.bytes,
                        require_payment: true,
                    }],
                    prover_address: prover_addr,
                }
                .to_vec(),
            )
            .await
            .unwrap();

        let assessor_proof = prover
            .prove_and_monitor_stark(&assessor_id_str, &assessor_input, vec![echo_proof.id.clone()])
            .await
            .unwrap();
        let assessor_receipt = prover.get_receipt(&assessor_proof.id).await.unwrap().unwrap();

        let set_builder_input = prover
            .upload_input(
                encode_input(&GuestInput::Singleton {
                    self_image_id: set_builder_id,
                    claim: assessor_receipt.claim().unwrap().as_value().unwrap().clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();

        let assessor_singleton = prover
            .prove_and_monitor_stark(
                &set_builder_id_str,
                &set_builder_input,
                vec![assessor_proof.id.clone()],
            )
            .await
            .unwrap();
        let assessor_singleton_journal =
            prover.get_journal(&assessor_singleton.id).await.unwrap().unwrap();
        let assessor_output = GuestOutput::abi_decode(&assessor_singleton_journal, false).unwrap();

        let singleton_journal = prover.get_journal(&echo_singleton.id).await.unwrap().unwrap();
        let tree_output = GuestOutput::abi_decode(&singleton_journal, false).unwrap();

        let join_input = prover
            .upload_input(
                encode_input(&GuestInput::Join {
                    self_image_id: set_builder_id,
                    left_set_root: tree_output.root(),
                    right_set_root: assessor_output.root(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let batch_root_proof = prover
            .prove_and_monitor_stark(
                &set_builder_id_str,
                &join_input,
                vec![echo_singleton.id.clone(), assessor_singleton.id.clone()],
            )
            .await
            .unwrap();
        let batch_g16 = prover.compress(&batch_root_proof.id).await.unwrap();
        let batch_journal = prover.get_journal(&batch_root_proof.id).await.unwrap().unwrap();
        let batch_output = GuestOutput::abi_decode(&batch_journal, false).unwrap();

        let order = Order {
            status: OrderStatus::PendingSubmission,
            updated_at: Utc::now(),
            target_block: Some(0),
            request: order_request,
            image_id: Some(echo_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(echo_proof.id.clone()),
            expire_block: Some(100),
            path: Some(vec![assessor_output.root()]),
            client_sig: client_sig.into(),
            lock_price: None,
            error_msg: None,
        };
        let order_id = U256::from(order.request.id);
        db.add_order(order_id, order.clone()).await.unwrap();

        let batch_id = 0;
        let batch = Batch {
            status: BatchStatus::Complete,
            root: Some(batch_output.root()),
            orders_root: Some(tree_output.root()),
            orders: vec![order_id],
            groth16_proof_id: batch_g16,
            fees: U256::ZERO,
            start_time: Utc::now(),
            block_deadline: Some(
                order.request.offer.biddingStart + order.request.offer.timeout as u64,
            ),
            error_msg: None,
            peaks: vec![],
        };
        db.add_batch(batch_id, batch).await.unwrap();

        market.lockin_request(&order.request, &client_sig.into(), None).await.unwrap();

        let submitter = Submitter::new(
            db.clone(),
            config,
            prover.clone(),
            provider.clone(),
            set_verifier,
            market_address,
            set_builder_id,
        )
        .unwrap();

        assert!(submitter.process_next_batch().await.unwrap());

        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::Submitted);
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch() {
        let config = ConfigLock::default();
        run_submit_batch(config).await;
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_merged_txn() {
        let config = ConfigLock::default();
        config.load_write().as_mut().unwrap().batcher.single_txn_fulfill = true;
        run_submit_batch(config).await;
    }
}
