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

use std::{collections::HashMap, sync::Arc, time::Duration};

use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_ether, format_units},
        Address, U256,
    },
    providers::{Provider, WalletProvider},
};
use anyhow::{anyhow, Context, Result};
use boundless_market::{
    contracts::boundless_market::{
        BoundlessMarketService, FulfillmentTx, MarketError, UnlockedRequest,
    },
    contracts::AssessorReceipt,
    telemetry::CompletionOutcome,
};
use tokio::sync::mpsc;

use crate::{
    backend::{
        BackendRouter, FulfillmentBatch, FulfillmentOrder, VerifierUpdate, VerifierUpdateError,
    },
    config::ConfigLock,
    db::DbObj,
    errors::{handle_order_failure, BrokerFailure, CodedError},
    now_timestamp,
    order_committer::{CommitmentComplete, CommitmentOutcome},
    task::{BrokerService, SupervisorErr},
    Batch, FulfillmentType, Order, OrderStatus,
};
use tokio_util::sync::CancellationToken;

use super::error::SubmitterErr;

#[derive(Clone)]
pub struct Submitter<P> {
    db: DbObj,
    backend: Arc<BackendRouter>,
    market: BoundlessMarketService<Arc<P>>,
    prover_address: Address,
    config: ConfigLock,
    chain_id: u64,
    /// Sends ProvingCompleted or ProvingFailed to the OrderCommitter to free the capacity slot.
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl<P> Submitter<P>
where
    P: Provider<Ethereum> + WalletProvider + 'static + Clone,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        db: DbObj,
        config: ConfigLock,
        backend: Arc<BackendRouter>,
        provider: Arc<P>,
        market_addr: Address,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Result<Self> {
        let txn_timeout_opt = {
            let config = config.lock_all().context("Failed to read config")?;
            config.batcher.txn_timeout
        };

        let mut market = BoundlessMarketService::new_for_broker(
            market_addr,
            provider.clone(),
            provider.default_signer_address(),
        );
        tracing::debug!("Setting market timeout to {}", txn_timeout_opt);
        market = market.with_timeout(Duration::from_secs(txn_timeout_opt));

        let prover_address = provider.default_signer_address();

        Ok(Self { db, backend, market, prover_address, config, chain_id, proving_completion_tx })
    }

    pub async fn submit_batch(&self, batch_id: usize, batch: &Batch) -> Result<(), SubmitterErr> {
        tracing::info!("Submitting batch {batch_id}");

        // Check that at least one order in the batch is not expired before submitting on chain.
        // Can happen if we overcommitted to work and proving took longer than expected.
        let now = now_timestamp();
        let order_ids = batch.orders.iter().map(|order| order.as_str()).collect::<Vec<_>>();
        let orders = self.db.get_orders(&order_ids).await.context("Failed to get orders")?;
        let expired_orders =
            orders.iter().filter(|order| order.request.expires_at() < now).collect::<Vec<_>>();
        if expired_orders.len() == orders.len() {
            return self.handle_expired_requests_error(batch_id, orders).await;
        } else if !expired_orders.is_empty() {
            // Still submit, since we support partial fulfillment.
            tracing::warn!("Some orders in batch {batch_id} are expired ({}). Batch will still be submitted. {:?}", expired_orders.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "), SubmitterErr::SomeRequestsExpiredBeforeSubmission(expired_orders.iter().map(|order| order.id()).collect()));
        }

        let mut fulfillments = vec![];
        let mut requests_to_price: Vec<UnlockedRequest> = vec![];

        struct OrderPrice {
            price: U256,
            collateral_reward: U256,
        }
        let mut order_prices: HashMap<&str, OrderPrice> = HashMap::new();
        let mut fulfillment_to_order_id: HashMap<U256, String> = HashMap::new();
        let orders_by_id: HashMap<String, Order> =
            orders.iter().map(|order| (order.id(), order.clone())).collect();
        let mut fulfillment_orders = Vec::with_capacity(batch.orders.len());
        let mut skipped_done_orders = Vec::new();

        for order_id in batch.orders.iter() {
            // On a submit_batch retry after a partial on-chain success, an order may already
            // have been finalized and advanced to Done. It must not be resubmitted, and the
            // prep-failure path below must not flip an on-chain-fulfilled order back to Failed.
            if orders_by_id.get(order_id).is_some_and(|order| order.status == OrderStatus::Done) {
                tracing::info!("Order {order_id} already finalized, skipping resubmission");
                skipped_done_orders.push(order_id.clone());
                continue;
            }
            // An order that failed preparation in an earlier attempt is already terminal —
            // re-running handle_order_failure here would emit a duplicate ProvingFailed
            // CommitmentComplete and double-count the capacity release.
            if orders_by_id.get(order_id).is_some_and(|order| order.status == OrderStatus::Failed) {
                tracing::info!(
                    "Order {order_id} already failed in a prior submit attempt, skipping"
                );
                continue;
            }
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

                let order_img_id: [u8; 32] = hex::decode(order_img_id)
                    .context("Failed to decode order image ID")?
                    .try_into()
                    .map_err(|_| anyhow!("Order image ID must be 32 bytes"))?;
                let mut collateral_reward = U256::ZERO;
                if fulfillment_type == FulfillmentType::FulfillAfterLockExpire {
                    requests_to_price
                        .push(UnlockedRequest::new(order_request.clone(), client_sig.clone()));
                    collateral_reward =
                        order_request.offer.collateral_reward_if_locked_and_not_fulfilled();
                }

                order_prices.insert(order_id, OrderPrice { price: lock_price, collateral_reward });

                let compressed_proof_id =
                    orders_by_id.get(order_id).and_then(|order| order.compressed_proof_id.clone());

                fulfillment_orders.push(FulfillmentOrder {
                    order_id: order_id.clone(),
                    request: order_request,
                    proof_id: order_proof_id.try_into()?,
                    compressed_proof_id: compressed_proof_id.map(TryInto::try_into).transpose()?,
                    program_id: order_img_id.into(),
                });
                anyhow::Ok(())
            };

            if let Err(err) = res.await {
                tracing::error!("Failed to submit {order_id}: {err:?}");
                handle_order_failure(
                    &self.db,
                    order_id,
                    &BrokerFailure::new(
                        SubmitterErr::UnexpectedErr(err).code(),
                        "Failed to submit",
                        CompletionOutcome::ProvingFailed,
                    ),
                    self.chain_id,
                    &self.proving_completion_tx,
                )
                .await;
            }
        }

        if fulfillment_orders.is_empty() {
            if skipped_done_orders.len() == batch.orders.len() {
                tracing::info!(
                    "All orders in batch {batch_id} were already finalized; marking batch submitted"
                );
                return Ok(());
            }
            tracing::error!(
                "All orders in batch {batch_id} failed during submission preparation. \
                 Skipping on-chain submission."
            );
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "No fulfillments to submit for batch {batch_id}"
            )));
        }

        let artifacts = self
            .backend
            .build_fulfillments(FulfillmentBatch {
                backend_id: batch.backend_id.clone(),
                state: batch.backend_state.clone(),
                assessor_proof_id: batch.assessor_proof_id.clone(),
                eip712_domain: self.market.eip712_domain().await?,
                orders: fulfillment_orders,
            })
            .await?;

        for failed in artifacts.failed_orders {
            tracing::error!("Failed to submit {}: {:?}", failed.order_id, failed.error);
            handle_order_failure(
                &self.db,
                &failed.order_id,
                &BrokerFailure::new(
                    SubmitterErr::UnexpectedErr(failed.error).code(),
                    "Failed to submit",
                    CompletionOutcome::ProvingFailed,
                ),
                self.chain_id,
                &self.proving_completion_tx,
            )
            .await;
        }

        for artifact in artifacts.orders {
            fulfillment_to_order_id.insert(artifact.fulfillment.id, artifact.order_id);
            fulfillments.push(artifact.fulfillment);
        }

        if fulfillments.is_empty() {
            tracing::error!(
                "All orders in batch {batch_id} failed during fulfillment preparation. \
                 Skipping on-chain submission."
            );
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "No fulfillments to submit for batch {batch_id}"
            )));
        }

        let (single_txn_fulfill, withdraw) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.batcher.single_txn_fulfill, config.batcher.withdraw)
        };

        let assessor_receipt = AssessorReceipt {
            seal: artifacts.assessor.seal,
            callbacks: artifacts.assessor.callbacks,
            selectors: artifacts.assessor.selectors,
            prover: self.prover_address,
        };
        let mut fulfillment_tx = FulfillmentTx::new(fulfillments.clone(), assessor_receipt)
            .with_withdraw(withdraw)
            .with_unlocked_requests(requests_to_price);
        for verifier_update in artifacts.verifier_updates {
            if single_txn_fulfill {
                match verifier_update {
                    VerifierUpdate::SubmitMerkleRoot { verifier, root, seal } => {
                        fulfillment_tx = fulfillment_tx.with_submit_root(verifier, root, seal);
                    }
                }
                continue;
            }

            let request_ids: Vec<_> = fulfillments.iter().map(|f| &f.id).collect();
            let applied = match self
                .backend
                .verifier_update_applied(&batch.backend_id, &verifier_update)
                .await
            {
                Ok(res) => {
                    tracing::info!(
                        "Checked if verifier update for batch {batch_id} with requests {:?} is already applied: {res:?}",
                        request_ids
                    );
                    res
                }
                Err(err) => {
                    tracing::warn!(
                        "Failed to query verifier update status for batch {batch_id} with requests {:?}, trying to submit anyway {err:?}",
                        request_ids
                    );
                    false
                }
            };
            if applied {
                tracing::info!(
                    "Verifier already reflects update for batch {batch_id} with requests {:?}, skipping to fulfillment",
                    request_ids
                );
                continue;
            }
            tracing::info!(
                "Submitting verifier update for batch {batch_id} with requests {:?}",
                request_ids
            );
            if let Err(err) =
                self.backend.apply_verifier_update(&batch.backend_id, &verifier_update).await
            {
                let order_ids: Vec<&str> = fulfillments
                    .iter()
                    .map(|f| fulfillment_to_order_id.get(&f.id).unwrap().as_str())
                    .collect();
                tracing::warn!("Failed to submit verifier update for orders: {order_ids:?}");

                // The backend already classified the failure; map it onto the BoundlessMarket
                // error taxonomy. This `match` is exhaustive, so a new `VerifierUpdateError`
                // variant is a compile error here rather than a silent misclassification.
                let market_err = match err {
                    VerifierUpdateError::TxnConfirmation(err) => {
                        MarketError::TxnConfirmationError(err)
                    }
                    VerifierUpdateError::Other(err) => MarketError::Error(err),
                };
                return Err(Self::classify_fulfillment_error(market_err, batch_id));
            }
        }

        if let Err(err) = self.market.fulfill(fulfillment_tx).await {
            let order_ids: Vec<&str> = fulfillments
                .iter()
                .map(|f| fulfillment_to_order_id.get(&f.id).unwrap().as_str())
                .collect();
            tracing::warn!("Failed to fulfill batch for orders {order_ids:?}: {err:?}");
            return Err(Self::classify_fulfillment_error(err, batch_id));
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
            if let Err(err) = self.proving_completion_tx.try_send(CommitmentComplete {
                order_id: order_id.to_string(),
                chain_id: self.chain_id,
                outcome: CommitmentOutcome::ProvingCompleted,
            }) {
                tracing::error!(
                    "Failed to send proving completion for order {order_id}; capacity tracking may be stale: {err}"
                );
            }

            crate::telemetry::telemetry(self.chain_id).record_fulfilled(order_id);
            let order_price = order_prices
                .get(order_id.as_str())
                .unwrap_or(&OrderPrice { price: U256::ZERO, collateral_reward: U256::ZERO });

            let eth_reward_log = format!("eth_reward: {}", format_ether(order_price.price));
            let collateral_token_decimals = self.market.collateral_token_decimals().await?;
            let collateral_reward =
                format_units(order_price.collateral_reward, collateral_token_decimals).unwrap();
            let mut collateral_reward_log = format!("collateral_reward: {collateral_reward}");

            // If we expect a stake reward, check if we won the proof race to be the first secondary prover.
            if order_price.collateral_reward > U256::ZERO {
                let prover =
                    self.market.get_request_fulfillment_prover(fulfillment.id, None, None).await;
                if let Ok(prover) = prover {
                    if prover != self.prover_address {
                        collateral_reward_log = format!("collateral_reward: 0 (lost secondary prover race to {prover} for {collateral_reward})");
                    }
                } else {
                    tracing::warn!("Failed to confirm if we were the first secondary prover for fulfillment {:x}", fulfillment.id);
                }
            }

            tracing::info!(
                "✨ Completed order: 0x{:x} {} {} ✨",
                fulfillment.id,
                eth_reward_log,
                collateral_reward_log
            );
        }

        Ok(())
    }

    async fn handle_expired_requests_error(
        &self,
        batch_id: usize,
        orders: Vec<Order>,
    ) -> Result<(), SubmitterErr> {
        tracing::warn!("All orders in batch {batch_id} are expired ({}). Batch will not be submitted, and all orders will be marked as failed.", &orders.iter().map(|order| format!("{order}")).collect::<Vec<_>>().join(", "));
        let expired_err = SubmitterErr::AllRequestsExpiredBeforeSubmission(Vec::new());
        for order in orders.clone() {
            handle_order_failure(
                &self.db,
                order.id().as_str(),
                &BrokerFailure::new(
                    expired_err.code(),
                    "Expired before submission",
                    CompletionOutcome::ExpiredBeforeSubmission,
                ),
                self.chain_id,
                &self.proving_completion_tx,
            )
            .await;
        }
        Err(SubmitterErr::AllRequestsExpiredBeforeSubmission(
            orders.iter().map(|order| format!("{order}")).collect(),
        ))
    }

    fn classify_fulfillment_error(err: MarketError, batch_id: usize) -> SubmitterErr {
        tracing::warn!("Failed to submit proofs for batch {batch_id}: {err:?}");
        if let MarketError::TxnConfirmationError(_) = &err {
            SubmitterErr::TxnConfirmationError(err)
        } else {
            SubmitterErr::MarketError(err)
        }
    }

    pub async fn process_next_batch(&self) -> Result<(), SubmitterErr> {
        let batch_res =
            self.db.get_complete_batch().await.context("Failed to get complete batch")?;

        let Some((batch_id, batch)) = batch_res else {
            return Ok(());
        };

        let (max_attempts, retry_delay_ms) = {
            let cfg = self.config.lock_all().context("Failed to read config")?;
            (cfg.batcher.max_submission_attempts, cfg.batcher.submit_retry_delay_ms)
        };
        let retry_count = u64::from(max_attempts.saturating_sub(1));

        let context = format!("batch_id={batch_id}");
        let result = crate::futures_retry::retry_only_with_context(
            retry_count,
            retry_delay_ms,
            || async { self.submit_batch(batch_id, &batch).await },
            "submit_batch",
            &context,
            |err: &SubmitterErr| {
                // Retry on every error except payment-requirements, which is fatal today.
                !matches!(
                    err,
                    SubmitterErr::MarketError(
                        MarketError::PaymentRequirementsFailed(_)
                            | MarketError::PaymentRequirementsFailedUnknownError(_),
                    )
                )
            },
        )
        .await;

        let err = match result {
            Ok(()) => {
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
            Err(err) => err,
        };

        tracing::warn!("Batch {batch_id} submission failed after retries: {err:?}");

        // Now that retries are exhausted, mark every order in the batch as Failed.
        // `handle_order_failure` updates the DB, emits the telemetry Failed event, and sends
        // ProvingFailed to the OrderCommitter so the in-flight capacity slot is freed —
        // without that last step, a permanent batch failure leaks a slot in
        // `OrderCommitter::in_flight`, eventually exhausting `max_concurrent_proofs` and
        // silently halting dispatch on all chains.
        for order_id in batch.orders.iter() {
            match self.db.get_order(order_id).await {
                Ok(Some(order))
                    if matches!(order.status, OrderStatus::Done | OrderStatus::Failed) =>
                {
                    tracing::info!(
                        "Order {order_id} already in terminal state {:?} after batch submission retry; skipping failure update",
                        order.status
                    );
                    continue;
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(
                        "Failed to check order {order_id} status before batch failure handling: {err:?}"
                    );
                }
            }
            handle_order_failure(
                &self.db,
                order_id,
                &BrokerFailure::new(
                    err.code(),
                    "Failed to submit batch",
                    CompletionOutcome::ProvingFailed,
                ),
                self.chain_id,
                &self.proving_completion_tx,
            )
            .await;
        }

        if let Err(db_err) = self.db.set_batch_failure(batch_id, format!("{err:?}")).await {
            return Err(SubmitterErr::UnexpectedErr(anyhow!(
                "Failed to set batch failure in db: {batch_id} - {db_err:?}"
            )));
        }

        if matches!(err, SubmitterErr::TxnConfirmationError(_)) {
            Err(SubmitterErr::BatchSubmissionFailedTimeouts(vec![err]))
        } else {
            Err(SubmitterErr::BatchSubmissionFailed(vec![err]))
        }
    }
}

impl<P> BrokerService for Submitter<P>
where
    P: Provider<Ethereum> + WalletProvider + Clone + Send + Sync + 'static,
{
    type Error = SubmitterErr;

    async fn run(self, cancel_token: CancellationToken) -> Result<(), SupervisorErr<Self::Error>> {
        tracing::info!("Starting Submitter service");
        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!("Submitter service received cancellation");
                break;
            }

            // Process batch without interruption
            let result = self.process_next_batch().await;
            if let Err(err) = result {
                // Only restart the service on unexpected errors.
                match err {
                    SubmitterErr::BatchSubmissionFailed(_)
                    | SubmitterErr::BatchSubmissionFailedTimeouts(_) => {
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
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backend::{
            AssessorProofId, BackendBatchState, BackendEntry, BackendRouter, CompressedProofId,
            ProofId, Risc0Backend,
        },
        db::SqliteDb,
        now_timestamp,
        provers::{encode_input, DefaultProver, ProverObj},
        requestor_monitor::PriorityRequestors,
        Batch, BatchStatus, ConfigurableDownloader, Order, OrderStatus,
    };
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        primitives::{Address, Bytes, U256},
        providers::ProviderBuilder,
        signers::local::PrivateKeySigner,
    };
    use boundless_assessor::{AssessorInput, Fulfillment};
    use boundless_market::{
        contracts::{
            hit_points::default_allowance, FulfillmentData, Offer, Predicate, ProofRequest,
            RequestId, RequestInput, RequestInputType, Requirements,
        },
        input::GuestEnv,
    };
    use boundless_test_utils::{
        guests::{
            ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH, ECHO_ELF, ECHO_ID,
            SET_BUILDER_ELF, SET_BUILDER_ID, SET_BUILDER_PATH,
        },
        market::{deploy_boundless_market, deploy_hit_points},
        verifier::{deploy_mock_verifier, deploy_set_verifier},
    };
    use chrono::Utc;
    use risc0_aggregation::GuestState;
    use risc0_zkvm::sha::{Digest, Digestible};
    use tracing_test::traced_test;

    #[derive(Clone, Default)]
    struct BatchHarnessOptions {
        /// Add a second order to the batch whose DB row has no proof id, so the submitter
        /// fails it during submission preparation while still submitting the real order.
        with_failing_order: bool,
    }

    async fn build_submitter_and_batch(
        config: ConfigLock,
    ) -> (
        AnvilInstance,
        Submitter<impl Provider + WalletProvider + Clone + 'static>,
        DbObj,
        usize,
        mpsc::Receiver<CommitmentComplete>,
    ) {
        let (anvil, submitter, db, batch_id, rx, _failing) =
            build_submitter_and_batch_with_options(config, BatchHarnessOptions::default()).await;
        (anvil, submitter, db, batch_id, rx)
    }

    async fn build_submitter_and_batch_with_options(
        config: ConfigLock,
        options: BatchHarnessOptions,
    ) -> (
        AnvilInstance,
        Submitter<impl Provider + WalletProvider + Clone + 'static>,
        DbObj,
        usize,
        mpsc::Receiver<CommitmentComplete>,
        Option<String>,
    ) {
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
            bytemuck::cast::<_, [u8; 32]>(SET_BUILDER_ID).into(),
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

        let market =
            BoundlessMarketService::new_for_broker(market_address, provider.clone(), prover_addr);
        market.deposit_collateral_with_permit(default_allowance(), &signer).await.unwrap();

        let market_customer = BoundlessMarketService::new_for_broker(
            market_address,
            customer_provider.clone(),
            customer_addr,
        );
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
            Requirements::new(Predicate::prefix_match(echo_id, Bytes::default())),
            "http://risczero.com/image",
            RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
            Offer {
                minPrice: U256::from(2),
                maxPrice: U256::from(4),
                rampUpStart: now_timestamp(),
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
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
                fulfillment_data: FulfillmentData::from_image_id_and_journal(
                    echo_id,
                    echo_receipt.journal.bytes.clone(),
                ),
            }],
            prover_address: prover_addr,
        };
        let assessor_stdin = GuestEnv::builder().write_frame(&assessor_input.encode()).stdin;

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
            backend_id: None,
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::ZERO),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: market_address,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            cached_id: Default::default(),
        };
        let order_id = order.id();
        db.add_order(&order).await.unwrap();

        // Optionally add a second order the submitter cannot prepare (its DB row has no proof
        // id, so `get_submission_order` fails). It exercises partial-batch failure: the good
        // order must still submit, and each order must emit exactly one CommitmentComplete.
        let failing_order_id = if options.with_failing_order {
            let failing_request = ProofRequest::new(
                RequestId::new(Address::repeat_byte(0xBB), 1),
                Requirements::new(Predicate::prefix_match(echo_id, Bytes::default())),
                "http://risczero.com/image",
                RequestInput { inputType: RequestInputType::Inline, data: Default::default() },
                Offer {
                    minPrice: U256::from(2),
                    maxPrice: U256::from(4),
                    rampUpStart: now_timestamp(),
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockCollateral: U256::from(10),
                },
            );
            let failing_order = Order {
                status: OrderStatus::PendingSubmission,
                updated_at: Utc::now(),
                target_timestamp: Some(0),
                request: failing_request,
                image_id: None,
                input_id: None,
                proof_id: None,
                compressed_proof_id: None,
                backend_id: None,
                expire_timestamp: Some(now_timestamp() + 100),
                client_sig: Bytes::new(),
                lock_price: Some(U256::ZERO),
                fulfillment_type: FulfillmentType::LockAndFulfill,
                error_msg: None,
                boundless_market_address: market_address,
                chain_id,
                total_cycles: None,
                journal_bytes: None,
                proving_started_at: None,
                cached_id: Default::default(),
            };
            let failing_order_id = failing_order.id();
            db.add_order(&failing_order).await.unwrap();
            Some(failing_order_id)
        } else {
            None
        };

        let batch_id = 0;
        let mut batch_orders = vec![order_id];
        if let Some(failing) = failing_order_id.clone() {
            batch_orders.push(failing);
        }
        let batch = Batch {
            backend_id: Risc0Backend::default_id(),
            status: BatchStatus::ReadyToSubmit,
            assessor_proof_id: Some(AssessorProofId::new(assessor_proof.id).unwrap()),
            orders: batch_orders,
            fees: U256::ZERO,
            start_time: Utc::now(),
            deadline: Some(order.request.offer.rampUpStart + order.request.offer.timeout as u64),
            error_msg: None,
            backend_state: Some(BackendBatchState {
                data: serde_json::json!({
                    "guest_state": batch_guest_state,
                    "claim_digests": vec![
                        echo_receipt.claim().unwrap().digest(),
                        assessor_receipt.claim().unwrap().digest(),
                    ],
                }),
                proof_id: Some(ProofId::new(aggregation_proof.id).unwrap()),
                compressed_proof_id: Some(CompressedProofId::new(batch_g16).unwrap()),
            }),
        };
        db.add_batch(batch_id, batch).await.unwrap();

        market.lock_request(&order.request, client_sig.to_vec()).await.unwrap();

        let (commitment_tx, commitment_rx) = mpsc::channel::<CommitmentComplete>(100);
        let downloader = ConfigurableDownloader::new(config.clone()).await.unwrap();
        let priority_requestors = PriorityRequestors::new(config.clone(), anvil.chain_id());
        let risc0_backend = Arc::new(
            Risc0Backend::with_provers(
                prover.clone(),
                prover.clone(),
                downloader,
                priority_requestors,
            )
            .with_set_builder_program_id(set_builder_id)
            .with_set_verifier(set_verifier, provider.clone(), prover_addr),
        );
        let backend_router = Arc::new(
            BackendRouter::new().register_backend(BackendEntry::new(risc0_backend)).unwrap(),
        );
        let submitter = Submitter::new(
            db.clone(),
            config,
            backend_router,
            provider.clone(),
            market_address,
            anvil.chain_id(),
            commitment_tx,
        )
        .unwrap();

        (anvil, submitter, db, batch_id, commitment_rx, failing_order_id)
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
        let (_anvil, submitter, db, batch_id, _commitment_rx) =
            build_submitter_and_batch(config).await;
        process_next_batch(submitter, db, batch_id).await;
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_merged_txn() {
        let config = ConfigLock::default();
        config.load_write().as_mut().unwrap().batcher.single_txn_fulfill = true;
        let (_anvil, submitter, db, batch_id, _commitment_rx) =
            build_submitter_and_batch(config).await;
        process_next_batch(submitter, db, batch_id).await;
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_retry_max_attempts() {
        let config = ConfigLock::default();
        {
            let mut cfg = config.load_write();
            let cfg = cfg.as_mut().unwrap();
            // Pin the attempt count for stable assertions and zero out the retry delay
            // so the test does not wait between attempts.
            cfg.batcher.max_submission_attempts = 2;
            cfg.batcher.submit_retry_delay_ms = 0;
        }
        let (anvil, submitter, db, batch_id, mut commitment_rx) =
            build_submitter_and_batch(config).await;

        let batch = db.get_batch(batch_id).await.unwrap();
        let order_ids = batch.orders.clone();
        assert!(!order_ids.is_empty(), "test batch should have at least one order");

        drop(anvil); // drop anvil to simulate an RPC fault

        let res = submitter.process_next_batch().await;
        // futures_retry emits this format on each failed attempt (retry status before the
        // error so it survives multi-line error Debug output):
        //   "Operation [submit_batch] (context: batch_id=0) failed, starting retry 1/1: ..."
        assert!(logs_contain("Operation [submit_batch] (context: batch_id=0)"));
        assert!(logs_contain("starting retry 1/1"));
        assert!(logs_contain(
            "Operation [submit_batch] (context: batch_id=0) failed after 1 retries",
        ));
        assert!(logs_contain("Batch 0 submission failed after retries"));
        assert!(matches!(res, Err(SubmitterErr::BatchSubmissionFailed(_))));

        // After exhaustion, every order in the batch must be marked Failed and the
        // batch itself marked as failed.
        for order_id in &order_ids {
            let order = db.get_order(order_id).await.unwrap().expect("order exists");
            assert_eq!(
                order.status,
                OrderStatus::Failed,
                "order {order_id} should be Failed after retries exhausted"
            );
        }

        let final_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(final_batch.status, BatchStatus::Failed);

        // Every order in the failed batch must release its committer in-flight slot via a
        // ProvingFailed CommitmentComplete; without this the OrderCommitter capacity gate
        // leaks and silently halts dispatch on all chains.
        let mut received: Vec<CommitmentComplete> = Vec::with_capacity(order_ids.len());
        for _ in 0..order_ids.len() {
            let event = commitment_rx
                .recv()
                .await
                .expect("commitment_rx closed before receiving all completion events");
            received.push(event);
        }
        let mut received_ids: Vec<String> = received.iter().map(|c| c.order_id.clone()).collect();
        received_ids.sort();
        let mut expected_ids = order_ids.clone();
        expected_ids.sort();
        assert_eq!(
            received_ids, expected_ids,
            "every failed-batch order must emit exactly one CommitmentComplete"
        );
        for c in &received {
            assert!(
                matches!(c.outcome, CommitmentOutcome::ProvingFailed),
                "expected ProvingFailed outcome, got {:?}",
                c.outcome
            );
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_all_expired() {
        let config = ConfigLock::default();
        let (_anvil, submitter, db, _batch_id, _commitment_rx) =
            build_submitter_and_batch(config).await;

        // Expire the order by setting rampUpStart=0 and timeout=100 → expires_at()=100 (past)
        db.execute_raw(
            r#"UPDATE orders SET data = json_set(data, '$.request.offer.rampUpStart', 0, '$.request.offer.timeout', 100)"#,
        )
        .await
        .unwrap();

        let res = submitter.process_next_batch().await;
        // The retry loop wraps all errors in BatchSubmissionFailed; verify every
        // inner error is AllRequestsExpiredBeforeSubmission.
        assert!(
            matches!(
                &res,
                Err(SubmitterErr::BatchSubmissionFailed(errs))
                    if errs.iter().all(|e| matches!(e, SubmitterErr::AllRequestsExpiredBeforeSubmission(_)))
            ),
            "Expected BatchSubmissionFailed wrapping AllRequestsExpiredBeforeSubmission but got: {res:?}"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_prep_failure_submits_remaining_orders() {
        let config = ConfigLock::default();
        let (_anvil, submitter, db, batch_id, mut commitment_rx, failing_order_id) =
            build_submitter_and_batch_with_options(
                config,
                BatchHarnessOptions { with_failing_order: true },
            )
            .await;
        let failing_order_id = failing_order_id.expect("harness added a failing order");

        let batch = db.get_batch(batch_id).await.unwrap();
        let good_order_id = batch
            .orders
            .iter()
            .find(|id| **id != failing_order_id)
            .expect("batch has a submittable order")
            .clone();

        submitter.process_next_batch().await.unwrap();

        let final_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(final_batch.status, BatchStatus::Submitted);

        let good = db.get_order(&good_order_id).await.unwrap().expect("good order exists");
        assert_eq!(good.status, OrderStatus::Done, "good order should still submit");
        let failed = db.get_order(&failing_order_id).await.unwrap().expect("failing order exists");
        assert_eq!(failed.status, OrderStatus::Failed, "unprepared order should be Failed");

        let mut completed_for = Vec::new();
        let mut failed_for = Vec::new();
        for _ in 0..2 {
            let event = commitment_rx
                .recv()
                .await
                .expect("every batch order must emit a CommitmentComplete");
            match event.outcome {
                CommitmentOutcome::ProvingCompleted => completed_for.push(event.order_id),
                CommitmentOutcome::ProvingFailed => failed_for.push(event.order_id),
                other => panic!("unexpected commitment outcome: {other:?}"),
            }
        }
        assert!(
            commitment_rx.try_recv().is_err(),
            "each order must emit exactly one CommitmentComplete, no more"
        );
        completed_for.sort();
        failed_for.sort();
        assert_eq!(completed_for, vec![good_order_id], "good order should complete");
        assert_eq!(failed_for, vec![failing_order_id], "bad order should fail");
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_failure_does_not_fail_done_orders() {
        let config = ConfigLock::default();
        let (_anvil, submitter, db, batch_id, mut commitment_rx, failing_order_id) =
            build_submitter_and_batch_with_options(
                config,
                BatchHarnessOptions { with_failing_order: true },
            )
            .await;
        let failing_order_id = failing_order_id.expect("harness added a failing order");

        let batch = db.get_batch(batch_id).await.unwrap();
        let done_order_id = batch
            .orders
            .iter()
            .find(|id| **id != failing_order_id)
            .expect("batch has a submittable order")
            .clone();
        db.set_order_complete(&done_order_id).await.unwrap();

        let res = submitter.process_next_batch().await;
        assert!(matches!(res, Err(SubmitterErr::BatchSubmissionFailed(_))));

        let done = db.get_order(&done_order_id).await.unwrap().expect("done order exists");
        assert_eq!(done.status, OrderStatus::Done, "done order must not be failed");
        let failed = db.get_order(&failing_order_id).await.unwrap().expect("failing order exists");
        assert_eq!(failed.status, OrderStatus::Failed, "unprepared order should be Failed");

        let event =
            commitment_rx.recv().await.expect("failing order must emit a CommitmentComplete");
        assert_eq!(event.order_id, failing_order_id);
        assert!(matches!(event.outcome, CommitmentOutcome::ProvingFailed));
        assert!(
            commitment_rx.try_recv().is_err(),
            "already-done order must not emit another CommitmentComplete"
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn submit_batch_all_done_marks_batch_submitted() {
        let config = ConfigLock::default();
        let (_anvil, submitter, db, batch_id, mut commitment_rx) =
            build_submitter_and_batch(config).await;

        let batch = db.get_batch(batch_id).await.unwrap();
        for order_id in &batch.orders {
            db.set_order_complete(order_id).await.unwrap();
        }

        submitter.process_next_batch().await.unwrap();

        let final_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(final_batch.status, BatchStatus::Submitted);
        for order_id in &batch.orders {
            let order = db.get_order(order_id).await.unwrap().expect("order exists");
            assert_eq!(order.status, OrderStatus::Done);
        }
        assert!(
            commitment_rx.try_recv().is_err(),
            "already-done orders must not emit duplicate CommitmentComplete events"
        );
    }
}
