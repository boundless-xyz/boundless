// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{collections::HashMap, sync::Arc, time::Duration};

use alloy::{
    network::Ethereum,
    primitives::{utils::format_ether, Address, Bytes, B256, U256},
    providers::{Provider, WalletProvider},
    sol_types::{SolStruct, SolValue},
};
use anyhow::{anyhow, Context, Result};
use boundless_market::{
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        encode_seal, AssessorJournal, AssessorReceipt, Fulfillment, ProofRequest,
    },
    selector::is_groth16_selector,
};
use risc0_aggregation::{SetInclusionReceipt, SetInclusionReceiptVerifierParameters};
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{
    sha::{Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

use crate::{
    config::ConfigLock,
    db::DbObj,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Batch, FulfillmentType,
};
use thiserror::Error;

use crate::errors::CodedError;

#[derive(Error, Debug)]
pub enum SubmitterErr {
    #[error("{code} Batch submission failed: {0}", code = self.code())]
    BatchSubmissionFailed(String),

    #[error("{code} Failed to confirm transaction: {0}", code = self.code())]
    TxnConfirmationError(MarketError),

    #[error("{code} Request expired before submission: {0}", code = self.code())]
    RequestExpiredBeforeSubmission(MarketError),

    #[error("{code} Market error: {0}", code = self.code())]
    MarketError(#[from] MarketError),

    #[error("{code} Unexpected error: {0:?}", code = self.code())]
    UnexpectedErr(#[from] anyhow::Error),
}

impl CodedError for SubmitterErr {
    fn code(&self) -> &str {
        match self {
            SubmitterErr::UnexpectedErr(_) => "[B-SUB-500]",
            SubmitterErr::RequestExpiredBeforeSubmission(_) => "[B-SUB-001]",
            SubmitterErr::MarketError(_) => "[B-SUB-002]",
            SubmitterErr::BatchSubmissionFailed(_) => "[B-SUB-003]",
            SubmitterErr::TxnConfirmationError(_) => "[B-SUB-004]",
        }
    }
}

#[derive(Clone)]
pub struct Submitter<P> {
    db: DbObj,
    prover: ProverObj,
    market: BoundlessMarketService<Arc<P>>,
    set_verifier: SetVerifierService<Arc<P>>,
    set_verifier_addr: Address,
    set_builder_img_id: Digest,
    prover_address: Address,
    config: ConfigLock,
}

impl<P> Submitter<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
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
            tracing::debug!("Setting market timeout to {}", txn_timeout);
            market = market.with_timeout(Duration::from_secs(txn_timeout));
        }

        let mut set_verifier = SetVerifierService::new(
            set_verifier_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        if let Some(txn_timeout) = txn_timeout_opt {
            tracing::debug!("Setting set verifier timeout to {}", txn_timeout);
            set_verifier = set_verifier.with_timeout(Duration::from_secs(txn_timeout));
        }

        let prover_address = provider.default_signer_address();

        Ok(Self {
            db,
            prover,
            market,
            set_verifier,
            set_verifier_addr,
            set_builder_img_id,
            prover_address,
            config,
        })
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

    pub async fn submit_batch(&self, batch_id: usize, batch: &Batch) -> Result<(), SubmitterErr> {
        tracing::info!("Submitting batch {batch_id}");

        let Some(ref aggregation_state) = batch.aggregation_state else {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Cannot submit batch with no recorded aggregation state"
            )));
        };
        let Some(ref groth16_proof_id) = aggregation_state.groth16_proof_id else {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Cannot submit batch with no recorded Groth16 proof ID"
            )));
        };
        if aggregation_state.claim_digests.is_empty() {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Cannot submit batch with no claim digests"
            )));
        }
        if batch.assessor_proof_id.is_none() {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Cannot submit batch with no assessor receipt"
            )));
        }
        if !aggregation_state.guest_state.mmr.is_finalized() {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Cannot submit guest state that is not finalized"
            )));
        }

        // Collect the needed parts for the new merkle root:
        let batch_seal = self.fetch_encode_g16(groth16_proof_id).await?;
        let batch_root = risc0_aggregation::merkle_root(&aggregation_state.claim_digests);
        let root = B256::from_slice(batch_root.as_bytes());

        if aggregation_state.guest_state.mmr.clone().finalized_root().unwrap() != batch_root {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Guest state finalized root is inconsistent with claim digests"
            )));
        }

        // Collect the needed parts for the fulfillBatch:
        let assessor_proof_id = &batch.assessor_proof_id.clone().unwrap();
        let assessor_receipt = self
            .prover
            .get_receipt(assessor_proof_id)
            .await
            .context("Failed to get assessor receipt")?
            .context("Assessor receipt missing")?;
        let assessor_claim_digest = assessor_receipt
            .claim()
            .with_context(|| format!("Receipt for assessor {assessor_proof_id} missing claim"))?
            .value()
            .with_context(|| format!("Receipt for assessor {assessor_proof_id} claims pruned"))?
            .digest();
        let assessor_journal = AssessorJournal::abi_decode(&assessor_receipt.journal.bytes)
            .context("Failed to decode assessor journal for {assessor_proof_id}")?;

        let inclusion_params =
            SetInclusionReceiptVerifierParameters { image_id: self.set_builder_img_id };

        let mut fulfillments = vec![];
        let mut requests_to_price: Vec<(ProofRequest, Bytes)> = vec![];

        struct OrderPrice {
            price: U256,
            stake_reward: U256,
        }
        let mut order_prices: HashMap<&str, OrderPrice> = HashMap::new();
        let mut fulfillment_to_order_id: HashMap<U256, &str> = HashMap::new();

        for order_id in batch.orders.iter() {
            tracing::info!("Submitting order {order_id}");

            let res = async {
                let (
                    order_request,
                    client_sig,
                    order_proof_id,
                    order_img_id,
                    lock_price,
                    fulfillment_type,
                ) =
                    self.db.get_submission_order(order_id).await.context(
                        "Failed to get order from DB for submission, order NOT finalized",
                    )?;

                let mut stake_reward = U256::ZERO;
                if fulfillment_type == FulfillmentType::FulfillAfterLockExpire {
                    requests_to_price.push((order_request.clone(), client_sig.clone()));
                    stake_reward = order_request.offer.stake_reward_if_locked_and_not_fulfilled();
                }

                order_prices.insert(order_id, OrderPrice { price: lock_price, stake_reward });

                let order_journal = self
                    .prover
                    .get_journal(&order_proof_id)
                    .await
                    .context("Failed to get order journal from prover")?
                    .context("Order proof Journal missing")?;

                let seal = if is_groth16_selector(order_request.requirements.selector) {
                    let compressed_proof_id =
                        self.db.get_order_compressed_proof_id(order_id).await.context(
                            "Failed to get order compressed proof ID from DB for submission",
                        )?;
                    self.fetch_encode_g16(&compressed_proof_id)
                        .await
                        .context("Failed to fetch and encode g16 proof")?
                } else {
                    // NOTE: We assume here that the order execution ended with exit code 0.
                    let order_claim = ReceiptClaim::ok(
                        order_img_id.0,
                        MaybePruned::Pruned(order_journal.digest()),
                    );
                    let order_claim_index = aggregation_state
                        .claim_digests
                        .iter()
                        .position(|claim| *claim == order_claim.digest())
                        .ok_or(anyhow!(
                            "Failed to find order claim {order_claim:x?} in aggregated claims"
                        ))?;
                    let order_path = risc0_aggregation::merkle_path(
                        &aggregation_state.claim_digests,
                        order_claim_index,
                    );
                    tracing::debug!(
                        "Merkle path for order {order_id} : {:x?} : {order_path:x?}",
                        order_claim.digest()
                    );
                    let set_inclusion_receipt = SetInclusionReceipt::from_path_with_verifier_params(
                        order_claim,
                        order_path,
                        inclusion_params.digest(),
                    );
                    set_inclusion_receipt.abi_encode_seal().context("Failed to encode seal")?
                };

                tracing::debug!("Seal for order {order_id} : {}", hex::encode(seal.clone()));

                let request_digest = order_request
                    .eip712_signing_hash(&self.market.eip712_domain().await?.alloy_struct());
                let request_id = order_request.id;
                fulfillment_to_order_id.insert(request_id, order_id);
                fulfillments.push(Fulfillment {
                    id: request_id,
                    requestDigest: request_digest,
                    imageId: order_img_id,
                    journal: order_journal.into(),
                    seal: seal.into(),
                });
                anyhow::Ok(())
            };

            if let Err(err) = res.await {
                tracing::error!("Failed to submit {order_id}: {err}");
                if let Err(db_err) = self.db.set_order_failure(order_id, err.to_string()).await {
                    tracing::error!("Failed to set order failure during proof submission: {order_id} {db_err:?}");
                }
            }
        }

        let assessor_claim_index = aggregation_state
            .claim_digests
            .iter()
            .position(|claim| *claim == assessor_claim_digest)
            .ok_or(anyhow!("Failed to find order claim assessor claim in aggregated claims"))?;
        let assessor_path =
            risc0_aggregation::merkle_path(&aggregation_state.claim_digests, assessor_claim_index);
        tracing::debug!(
            "Merkle path for assessor : {:x?} : {assessor_path:x?}",
            assessor_claim_digest
        );

        let assessor_seal = SetInclusionReceipt::from_path_with_verifier_params(
            // TODO: Set inclusion proofs, when ABI encoded, currently don't contain anything
            // derived from the claim. So instead of constructing the journal, we simply use the
            // zero digest. We should either plumb through the data for the assessor journal, or we
            // should make an explicit way to encode an inclusion proof without the claim.
            ReceiptClaim::ok(Digest::ZERO, MaybePruned::Pruned(Digest::ZERO)),
            assessor_path,
            inclusion_params.digest(),
        );
        let assessor_seal =
            assessor_seal.abi_encode_seal().context("ABI encode assessor set inclusion receipt")?;

        let mut single_txn_fulfill = {
            let config = self.config.lock_all().context("Failed to read config")?;
            config.batcher.single_txn_fulfill
        };

        if !requests_to_price.is_empty() && single_txn_fulfill {
            tracing::warn!("Single txn fulfill is enabled but we are fulfilling requests that were not locked. Overriding single txn fulfill to false.");
            single_txn_fulfill = false;
        }

        let assessor_receipt = AssessorReceipt {
            seal: assessor_seal.into(),
            selectors: assessor_journal.selectors,
            prover: self.prover_address,
            callbacks: assessor_journal.callbacks,
        };
        if single_txn_fulfill {
            if let Err(err) = self
                .market
                .submit_merkle_and_fulfill(
                    self.set_verifier_addr,
                    root,
                    batch_seal.into(),
                    fulfillments.clone(),
                    assessor_receipt,
                )
                .await
            {
                let order_ids: Vec<&str> = fulfillments
                    .iter()
                    .map(|f| *fulfillment_to_order_id.get(&f.id).unwrap())
                    .collect();
                tracing::warn!("Failed to submit merkle and fulfill for orders: {order_ids:?}");
                self.handle_fulfillment_error(err, batch_id, &fulfillments, &order_ids).await?;
            }
        } else {
            let contains_root = match self.set_verifier.contains_root(root).await {
                Ok(res) => res,
                Err(err) => {
                    tracing::warn!("Failed to query if set-verifier contains the new root, trying to submit anyway {err:?}");
                    false
                }
            };
            if !contains_root {
                tracing::info!("Submitting app merkle root: {root}");
                if let Err(err) =
                    self.set_verifier.submit_merkle_root(root, batch_seal.into()).await
                {
                    let order_ids: Vec<&str> = fulfillments
                        .iter()
                        .map(|f| *fulfillment_to_order_id.get(&f.id).unwrap())
                        .collect();
                    tracing::warn!("Failed to submit app merkle root for orders: {order_ids:?}");

                    // Map the error from the R0 Contracts crate crate to an error type from BoundlessMarket
                    if err.to_string().contains("failed to confirm tx") {
                        self.handle_fulfillment_error(
                            MarketError::TxnConfirmationError(err),
                            batch_id,
                            &fulfillments,
                            &order_ids,
                        )
                        .await?;
                    } else {
                        self.handle_fulfillment_error(
                            MarketError::Error(err),
                            batch_id,
                            &fulfillments,
                            &order_ids,
                        )
                        .await?;
                    }
                }
            } else {
                tracing::info!("Contract already contains root, skipping to fulfillment");
            }

            if !requests_to_price.is_empty() {
                let (requests, client_sigs): (Vec<ProofRequest>, Vec<Bytes>) =
                    requests_to_price.into_iter().unzip();
                tracing::info!(
                    "Fulfilling {} requests, and pricing {} requests using priceAndFulfillBatch",
                    fulfillments.len(),
                    requests.len()
                );
                if let Err(err) = self
                    .market
                    .price_and_fulfill_batch(
                        requests,
                        client_sigs,
                        fulfillments.clone(),
                        assessor_receipt,
                        None,
                    )
                    .await
                {
                    let order_ids: Vec<&str> = fulfillments
                        .iter()
                        .map(|f| *fulfillment_to_order_id.get(&f.id).unwrap())
                        .collect();
                    tracing::warn!("Failed to price and fulfill batch for orders: {order_ids:?}");
                    self.handle_fulfillment_error(err, batch_id, &fulfillments, &order_ids).await?;
                }
            } else {
                tracing::info!("Fulfilling {} requests using fulfillBatch", fulfillments.len());
                if let Err(err) =
                    self.market.fulfill_batch(fulfillments.clone(), assessor_receipt).await
                {
                    let order_ids: Vec<&str> = fulfillments
                        .iter()
                        .map(|f| *fulfillment_to_order_id.get(&f.id).unwrap())
                        .collect();
                    tracing::warn!("Failed to fulfill batch for orders: {order_ids:?}");
                    self.handle_fulfillment_error(err, batch_id, &fulfillments, &order_ids).await?;
                }
            }
        }

        for fulfillment in fulfillments.iter() {
            let order_id = fulfillment_to_order_id.get(&fulfillment.id).unwrap();
            if let Err(db_err) = self.db.set_order_complete(order_id).await {
                tracing::error!(
                    "Failed to set order complete during proof submission: {:x} {db_err:?}",
                    fulfillment.id
                );
                continue;
            }
            let order_price = order_prices
                .get(order_id)
                .unwrap_or(&OrderPrice { price: U256::ZERO, stake_reward: U256::ZERO });
            tracing::info!(
                "✨ Completed order: {:x} fee: {} stake_reward: {} ✨",
                fulfillment.id,
                format_ether(order_price.price),
                format_ether(order_price.stake_reward)
            );
        }

        Ok(())
    }

    async fn handle_fulfillment_error(
        &self,
        err: MarketError,
        batch_id: usize,
        fulfillments: &[Fulfillment],
        order_ids: &[&str],
    ) -> Result<(), SubmitterErr> {
        tracing::warn!("Failed to submit proofs: {err:?} for batch {batch_id}");
        for (fulfillment, order_id) in fulfillments.iter().zip(order_ids.iter()) {
            if let Err(db_err) = self.db.set_order_failure(order_id, format!("{err:?}")).await {
                tracing::error!(
                    "Failed to set order failure during proof submission: {:x} {db_err:?}",
                    fulfillment.id
                );
            }
        }

        if err.to_string().contains("RequestIsExpiredOrNotPriced") {
            return Err(SubmitterErr::RequestExpiredBeforeSubmission(err));
        }

        if let MarketError::TxnConfirmationError(_) = &err {
            return Err(SubmitterErr::TxnConfirmationError(err));
        }

        Err(SubmitterErr::MarketError(err))
    }

    pub async fn process_next_batch(&self) -> Result<(), SubmitterErr> {
        let batch_res =
            self.db.get_complete_batch().await.context("Failed to get complete batch")?;

        let Some((batch_id, batch)) = batch_res else {
            return Ok(());
        };

        let max_batch_submission_attempts = self
            .config
            .lock_all()
            .context("Failed to read config")?
            .batcher
            .max_submission_attempts;

        let mut errors = Vec::new();
        for attempt in 0..max_batch_submission_attempts {
            match self.submit_batch(batch_id, &batch).await {
                Ok(_) => {
                    self.db
                        .set_batch_submitted(batch_id)
                        .await
                        .context("Failed to set batch submitted")?;
                    tracing::info!(
                        "Completed batch: {batch_id} total_fees: {}",
                        format_ether(batch.fees)
                    );
                    return Ok(());
                }
                Err(err) => {
                    tracing::warn!(
                        "Batch submission attempt {}/{} failed. Error: {err:?}",
                        attempt + 1,
                        max_batch_submission_attempts,
                    );
                    errors.push(err);
                }
            }
        }
        tracing::warn!("Batch {batch_id} has reached max submission attempts. Errors: {errors:?}");
        if let Err(err) = self.db.set_batch_failure(batch_id, format!("{errors:?}")).await {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Failed to set batch failure in db: {batch_id} - {err:?}"
            )));
        }
        Err(SubmitterErr::BatchSubmissionFailed(format!("{errors:?}")))
    }
}

impl<P> RetryTask for Submitter<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
{
    type Error = SubmitterErr;
    fn spawn(&self) -> RetryRes<Self::Error> {
        let obj_clone = self.clone();

        Box::pin(async move {
            tracing::info!("Starting Submitter service");
            loop {
                let result = obj_clone.process_next_batch().await;
                if let Err(err) = result {
                    // Only restart the service on unexpected errors.
                    match err {
                        SubmitterErr::BatchSubmissionFailed(_) => {
                            tracing::error!("Batch submission failed: {err:?}");
                        }
                        _ => {
                            tracing::error!("Submitter service failed: {err:?}");
                            return Err(SupervisorErr::Recover(err));
                        }
                    }
                }

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
        now_timestamp,
        provers::{encode_input, DefaultProver},
        AggregationState, Batch, BatchStatus, Order, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::U256,
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
    };
    use boundless_assessor::{AssessorInput, Fulfillment};
    use boundless_market::{
        contracts::{
            hit_points::default_allowance, Input, InputType, Offer, Predicate, PredicateType,
            ProofRequest, RequestId, Requirements,
        },
        input::GuestEnvBuilder,
    };
    use boundless_market_test_utils::{
        deploy_boundless_market, deploy_hit_points, deploy_mock_verifier, deploy_set_verifier,
        ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH, ECHO_ELF, ECHO_ID,
        SET_BUILDER_ELF, SET_BUILDER_ID, SET_BUILDER_PATH,
    };
    use chrono::Utc;
    use risc0_aggregation::GuestState;
    use risc0_zkvm::sha::Digest;
    use tracing_test::traced_test;

    async fn build_submitter_and_batch(
        config: ConfigLock,
    ) -> (AnvilInstance, Submitter<impl Provider + WalletProvider + Clone + 'static>, DbObj, usize)
    {
        let anvil = Anvil::new().spawn();
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        let customer_signer: PrivateKeySigner = anvil.keys()[1].clone().into();
        let prover_addr = signer.address();
        let customer_addr = customer_signer.address();
        tracing::info!("prover: {prover_addr} customer: {customer_addr}");

        let provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let customer_provider = Arc::new(
            ProviderBuilder::new()
                .wallet(EthereumWallet::from(customer_signer.clone()))
                .connect(&anvil.endpoint())
                .await
                .unwrap(),
        );

        let verifier = deploy_mock_verifier(provider.clone()).await.unwrap();
        let set_verifier = deploy_set_verifier(
            provider.clone(),
            verifier,
            Digest::from(SET_BUILDER_ID),
            format!("file://{SET_BUILDER_PATH}"),
        )
        .await
        .unwrap();
        let hit_points = deploy_hit_points(prover_addr, provider.clone()).await.unwrap();
        let market_address = deploy_boundless_market(
            prover_addr,
            provider.clone(),
            set_verifier,
            hit_points,
            Digest::from(ASSESSOR_GUEST_ID),
            format!("file://{ASSESSOR_GUEST_PATH}"),
            Some(prover_addr),
        )
        .await
        .unwrap();

        let market = BoundlessMarketService::new(market_address, provider.clone(), prover_addr);
        market.deposit_stake_with_permit(default_allowance(), &signer).await.unwrap();

        let market_customer =
            BoundlessMarketService::new(market_address, customer_provider.clone(), customer_addr);
        market_customer.deposit(U256::from(10000000000u64)).await.unwrap();

        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let prover: ProverObj = Arc::new(DefaultProver::new());

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
            RequestId::new(customer_addr, market_customer.index_from_nonce().await.unwrap()),
            Requirements::new(
                echo_id,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "http://risczero.com/image",
            Input { inputType: InputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(2),
                maxPrice: U256::from(4),
                biddingStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
            },
        );

        let chain_id = provider.get_chain_id().await.unwrap();
        let client_sig = order_request
            .sign_request(&customer_signer, market_address, chain_id)
            .await
            .unwrap()
            .as_bytes();

        let assessor_input = AssessorInput {
            domain: boundless_market::contracts::eip712_domain(market_address, chain_id),
            fills: vec![Fulfillment {
                request: order_request.clone(),
                signature: client_sig.into(),
                journal: echo_receipt.journal.bytes.clone(),
            }],
            prover_address: prover_addr,
        };
        let assessor_stdin = GuestEnvBuilder::new().write_frame(&assessor_input.encode()).stdin;

        let assessor_input = prover.upload_input(assessor_stdin).await.unwrap();

        let assessor_proof = prover
            .prove_and_monitor_stark(&assessor_id_str, &assessor_input, vec![echo_proof.id.clone()])
            .await
            .unwrap();
        let assessor_receipt = prover.get_receipt(&assessor_proof.id).await.unwrap().unwrap();

        // Build and finalize the aggregation in one execution.
        let set_builder_input = prover
            .upload_input(
                encode_input(
                    &GuestState::initial(set_builder_id)
                        .into_input(
                            vec![
                                echo_receipt.claim().unwrap().value().unwrap(),
                                assessor_receipt.claim().unwrap().value().unwrap(),
                            ],
                            true,
                        )
                        .unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let aggregation_proof = prover
            .prove_and_monitor_stark(
                &set_builder_id_str,
                &set_builder_input,
                vec![echo_proof.id.clone(), assessor_proof.id.clone()],
            )
            .await
            .unwrap();

        let batch_g16 = prover.compress(&aggregation_proof.id).await.unwrap();
        let batch_journal = prover.get_journal(&aggregation_proof.id).await.unwrap().unwrap();
        let batch_guest_state = GuestState::decode(&batch_journal).unwrap();
        assert!(batch_guest_state.mmr.is_finalized());
        assert_eq!(
            batch_guest_state.mmr.clone().finalized_root().unwrap(),
            risc0_aggregation::merkle_root(&[
                echo_receipt.claim().unwrap().digest(),
                assessor_receipt.claim().unwrap().digest(),
            ])
        );

        let order = Order {
            status: OrderStatus::PendingSubmission,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: order_request,
            image_id: Some(echo_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(echo_proof.id.clone()),
            compressed_proof_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::ZERO),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            proving_started_at: None,
        };
        let order_id = order.id();
        db.add_order(order.clone()).await.unwrap();

        let batch_id = 0;
        let batch = Batch {
            status: BatchStatus::Complete,
            assessor_proof_id: Some(assessor_proof.id),
            orders: vec![order_id],
            fees: U256::ZERO,
            start_time: Utc::now(),
            deadline: Some(order.request.offer.biddingStart + order.request.offer.timeout as u64),
            error_msg: None,
            aggregation_state: Some(AggregationState {
                guest_state: batch_guest_state,
                proof_id: aggregation_proof.id,
                groth16_proof_id: Some(batch_g16),
                claim_digests: vec![
                    echo_receipt.claim().unwrap().digest(),
                    assessor_receipt.claim().unwrap().digest(),
                ],
            }),
        };
        db.add_batch(batch_id, batch).await.unwrap();

        market.lock_request(&order.request, &client_sig.into(), None).await.unwrap();

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

        (anvil, submitter, db, batch_id)
    }

    async fn process_next_batch<P>(submitter: Submitter<P>, db: DbObj, batch_id: usize)
    where
        P: Provider<Ethereum> + WalletProvider + 'static + Clone,
    {
        submitter.process_next_batch().await.unwrap();
        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::Submitted);
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch() {
        let config = ConfigLock::default();
        let (_anvil, submitter, db, batch_id) = build_submitter_and_batch(config).await;
        process_next_batch(submitter, db, batch_id).await;
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_merged_txn() {
        let config = ConfigLock::default();
        config.load_write().as_mut().unwrap().batcher.single_txn_fulfill = true;
        let (_anvil, submitter, db, batch_id) = build_submitter_and_batch(config).await;
        process_next_batch(submitter, db, batch_id).await;
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_retry_max_attempts() {
        let config = ConfigLock::default();
        let (anvil, submitter, _db, _batch_id) = build_submitter_and_batch(config).await;

        drop(anvil); // drop anvil to simluate an RPC fault

        let res = submitter.process_next_batch().await;
        assert!(logs_contain("Batch submission attempt 1/2 failed"));
        assert!(logs_contain("reached max submission attempts"));
        assert!(matches!(res, Err(SubmitterErr::BatchSubmissionFailed(_))));
    }
}
