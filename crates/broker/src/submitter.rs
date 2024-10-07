// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::sync::Arc;

use aggregation_set::{SetInclusionReceipt, SetInclusionReceiptVerifierParameters};
use alloy::{
    network::Ethereum,
    primitives::{aliases::U192, Address, B256, U256},
    providers::{Provider, WalletProvider},
    transports::Transport,
};
use anyhow::{bail, Context, Result};
use boundless_market::contracts::{
    encode_seal, proof_market::ProofMarketService, set_verifier::SetVerifierService, Fulfillment,
};
use guest_assessor::ASSESSOR_GUEST_ID;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

use crate::{
    db::DbObj,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Batch,
};

#[derive(Clone)]
pub struct Submitter<T, P> {
    db: DbObj,
    prover: ProverObj,
    market: ProofMarketService<T, Arc<P>>,
    set_verifier: SetVerifierService<T, Arc<P>>,
    agg_set_img_id: Digest,
}

impl<T, P> Submitter<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + WalletProvider + 'static + Clone,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        prover: ProverObj,
        provider: Arc<P>,
        set_verifier_addr: Address,
        market_addr: Address,
        agg_set_img_id: Digest,
    ) -> Self {
        let market = ProofMarketService::new(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        let set_verifier = SetVerifierService::new(
            set_verifier_addr,
            provider.clone(),
            provider.default_signer_address(),
        );

        Self { db, prover, market, set_verifier, agg_set_img_id }
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

        let batch_seal = self.fetch_encode_g16(&batch.groth16_proof_id).await?;
        let batch_root = batch.root.context("Batch missing root digest")?;
        let root = B256::from_slice(batch_root.as_bytes());

        tracing::info!("Submitting app merkle root: {root}");
        self.set_verifier
            .submit_merkle_root(root, batch_seal.into())
            .await
            .context("Failed to submit app merkle_root")?;

        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: self.agg_set_img_id };

        let mut fulfillments = vec![];
        for order_id in batch.orders.iter() {
            tracing::info!("Submitting order {order_id:x}");

            let (order_proof_id, order_img_id, order_path) = match self
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

            fulfillments.push(Fulfillment {
                id: U192::from(*order_id),
                imageId: order_img_id,
                journal: order_journal.into(),
                seal: seal.into(),
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

        if let Err(err) =
            self.market.fulfill_batch(fulfillments.clone(), assessor_seal.into()).await
        {
            tracing::error!("Failed to submit proofs: {err:?} for batch {batch_id}");
            for fulfillment in fulfillments.iter() {
                if let Err(db_err) =
                    self.db.set_order_failure(U256::from(fulfillment.id), format!("{err:?}")).await
                {
                    tracing::error!(
                        "Failed to set order failure during proof submission: {:x} {db_err:?}",
                        fulfillment.id
                    );
                }
            }
            bail!("transaction to fulfill batch failed");
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
    use aggregation_set::{
        GuestInput, GuestOutput, AGGREGATION_SET_GUEST_ELF, AGGREGATION_SET_GUEST_ID,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::Anvil,
        primitives::{aliases::U96, FixedBytes, B256, U256},
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
        sol_types::SolValue,
    };
    use assessor::{AssessorInput, Fulfillment};
    use boundless_market::contracts::{
        test_utils::{MockVerifier, ProofMarket, SetVerifier},
        Input, InputType, Offer, Predicate, PredicateType, ProvingRequest, Requirements,
    };
    use chrono::Utc;
    use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID};
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn submit_batch() {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let prover_addr = signer.address();
        let customer_addr = customer_signer.address();
        tracing::info!("prover: {prover_addr} customer: {customer_addr}");

        let provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(signer))
                .on_http(anvil.endpoint().parse().unwrap()),
        );

        let customer_provider = Arc::new(
            ProviderBuilder::new()
                .with_recommended_fillers()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .on_http(anvil.endpoint().parse().unwrap()),
        );

        let verifier = MockVerifier::deploy(&provider, FixedBytes::ZERO).await.unwrap();
        let set_verifier = SetVerifier::deploy(
            &provider,
            *verifier.address(),
            FixedBytes::from_slice(&Digest::from(AGGREGATION_SET_GUEST_ID).as_bytes()),
            String::new(),
        )
        .await
        .unwrap();
        let proof_market = ProofMarket::deploy(
            &provider,
            *set_verifier.address(),
            FixedBytes::from_slice(&Digest::from(ASSESSOR_GUEST_ID).as_bytes()),
            String::new(),
        )
        .await
        .unwrap();

        let market =
            ProofMarketService::new(*proof_market.address(), provider.clone(), prover_addr);
        market.deposit(U256::from(10000000000u64)).await.unwrap();

        let market_customer = ProofMarketService::new(
            *proof_market.address(),
            customer_provider.clone(),
            customer_addr,
        );
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

        let agg_id = Digest::from(AGGREGATION_SET_GUEST_ID);
        let agg_id_str = agg_id.to_string();
        prover.upload_image(&agg_id_str, AGGREGATION_SET_GUEST_ELF.to_vec()).await.unwrap();

        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        let assessor_id_str = assessor_id.to_string();
        prover.upload_image(&assessor_id_str, ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();

        let echo_proof =
            prover.prove_and_monitor_stark(&echo_id_str, &input_id, vec![]).await.unwrap();
        let echo_receipt = prover.get_receipt(&echo_proof.id).await.unwrap().unwrap();

        let order_request = ProvingRequest::new(
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
                minPrice: U96::from(2),
                maxPrice: U96::from(4),
                biddingStart: 0,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
            },
        );

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = order_request
            .sign_request(&customer_signer, *proof_market.address(), chain_id)
            .unwrap()
            .as_bytes();

        let agg_input = prover
            .upload_input(
                encode_input(&GuestInput::Singleton {
                    self_image_id: agg_id,
                    claim: echo_receipt.claim().unwrap().as_value().unwrap().clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let echo_singleton = prover
            .prove_and_monitor_stark(&agg_id_str, &agg_input, vec![echo_proof.id.clone()])
            .await
            .unwrap();

        let assessor_input = prover
            .upload_input(
                AssessorInput {
                    domain: boundless_market::contracts::eip712_domain(
                        *proof_market.address(),
                        chain_id,
                    ),
                    fills: vec![Fulfillment {
                        request: order_request.clone(),
                        signature: client_sig.into(),
                        journal: echo_receipt.journal.bytes,
                    }],
                }
                .to_vec(),
            )
            .await
            .unwrap();

        let assessor_proof = prover
            .prove_and_monitor_stark(
                &assessor_id_str,
                &assessor_input,
                vec![echo_singleton.id.clone()],
            )
            .await
            .unwrap();
        let assessor_receipt = prover.get_receipt(&assessor_proof.id).await.unwrap().unwrap();

        let agg_input = prover
            .upload_input(
                encode_input(&GuestInput::Singleton {
                    self_image_id: agg_id,
                    claim: assessor_receipt.claim().unwrap().as_value().unwrap().clone(),
                })
                .unwrap(),
            )
            .await
            .unwrap();

        let assessor_singleton = prover
            .prove_and_monitor_stark(&agg_id_str, &agg_input, vec![assessor_proof.id.clone()])
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
                    self_image_id: agg_id,
                    left_set_root: tree_output.root(),
                    right_set_root: assessor_output.root(),
                })
                .unwrap(),
            )
            .await
            .unwrap();
        let batch_root_proof = prover
            .prove_and_monitor_stark(
                &agg_id_str,
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
            prover.clone(),
            provider.clone(),
            *set_verifier.address(),
            *proof_market.address(),
            agg_id,
        );

        assert!(submitter.process_next_batch().await.unwrap());

        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::Submitted);
    }
}
