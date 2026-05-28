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

use alloy::primitives::utils;
#[cfg(test)]
use alloy::primitives::Address;
use anyhow::{Context, Result};
use chrono::Utc;
#[cfg(test)]
use risc0_zkvm::sha::Digest;
use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::{
    backend::Risc0Backend, provers::ProverObj, requestor_monitor::PriorityRequestors,
    ConfigurableDownloader,
};
use crate::{
    backend::{
        BackendId, BackendRouter, BatchOrder, BatchSizeEstimateRequest, BatchUpdate, CloseBatch,
        OrderProvingData, UpdateBatch,
    },
    config::ConfigLock,
    db::{BatchReadyOrder, DbObj},
    now_timestamp,
    order_committer::{CommitmentComplete, CommitmentOutcome},
    task::{BrokerService, SupervisorErr},
    Batch, BatchStatus, FulfillmentType, Order,
};

use super::error::BatcherErr;

/// Generic per-order data the broker hands the backend so the backend stays stateless. The
/// opaque `backend_state` is replayed verbatim; the rest is broker-owned order data.
fn order_proving_data(order: &Order) -> OrderProvingData {
    OrderProvingData {
        order_id: order.id(),
        request: order.request.clone(),
        client_sig: order.client_sig.clone(),
        image_id: order.image_id.clone(),
        backend_state: order.backend_state.clone(),
    }
}

#[cfg(test)]
fn test_risc0_order_state(
    proof_id: impl Into<String>,
) -> Option<crate::backend::BackendOrderState> {
    Some(crate::backend::BackendOrderState(
        serde_json::json!({ "proof_id": proof_id.into(), "compressed_proof_id": null }),
    ))
}

#[derive(Clone)]
pub struct BatcherService {
    db: DbObj,
    config: ConfigLock,
    backend: Arc<BackendRouter>,
    chain_id: u64,
    /// Sends ProvingFailed to the OrderCommitter to free the global proving capacity slot.
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl BatcherService {
    #[cfg(test)]
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
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Result<Self> {
        let downloader = ConfigurableDownloader::new(config.clone()).await?;
        let priority_requestors = PriorityRequestors::new(config.clone(), chain_id);
        let backend = Arc::new(
            Risc0Backend::with_provers(
                prover.clone(),
                prover.clone(),
                Arc::new(downloader),
                priority_requestors.as_check(),
            )
            .with_set_builder_program_id(set_builder_guest_id)
            .with_test_batch_processor(
                config.proof_retry_policy(),
                prover.clone(),
                set_builder_guest_id,
                assessor_guest_id,
                market_addr,
                prover_addr,
                chain_id,
            ),
        );

        Ok(Self {
            db,
            config,
            backend: Arc::new(
                BackendRouter::new()
                    .register_backend(crate::backend::BackendEntry::new(backend))
                    .expect("static RISC0 backend registration is valid"),
            ),
            chain_id,
            proving_completion_tx,
        })
    }

    pub fn new_with_backend_router(
        db: DbObj,
        config: ConfigLock,
        backend_router: Arc<BackendRouter>,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Result<Self> {
        Ok(Self { db, config, backend: backend_router, chain_id, proving_completion_tx })
    }

    /// Check if we should finalize the batch
    ///
    /// Checks current min-deadline, batch timer, and current block.
    async fn check_finalize(
        &self,
        backend_id: &BackendId,
        batch_id: usize,
        batch: &Batch,
        pending_orders: &[BatchReadyOrder],
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
        let is_initial_state = batch.backend_state.is_none();
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

        // Historical config name: for RISC0 this estimate is journal bytes. More generally this is
        // a backend-estimated batch size used to cap submission payload growth.
        let pending_ids: Vec<String> = pending_orders.iter().map(|o| o.order_id.clone()).collect();
        let total_size_estimate = self
            .backend
            .estimate_batch_size(
                backend_id,
                BatchSizeEstimateRequest {
                    state: batch.backend_state.clone(),
                    existing_orders: self.fetch_proving_data(&batch.orders).await?,
                    pending_orders: self.fetch_proving_data(&pending_ids).await?,
                },
            )
            .await?
            .size;
        if total_size_estimate >= conf_max_journal_bytes {
            tracing::debug!(
                "Finalizing batch {batch_id}: batch size target hit {} >= {}",
                total_size_estimate,
                conf_max_journal_bytes
            );
            return Ok(true);
        } else {
            tracing::debug!(
                "Batch {batch_id} size estimate below limit {} < {}",
                total_size_estimate,
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

    /// Filter out non-actionable orders (expired or already fulfilled) and mark them as failed.
    async fn filter_non_actionable_orders(
        &self,
        orders: Vec<BatchReadyOrder>,
        current_time: u64,
    ) -> Result<Vec<BatchReadyOrder>, BatcherErr> {
        let mut valid_orders = Vec::with_capacity(orders.len());

        for order in orders {
            if order.expiration < current_time {
                tracing::warn!(
                    "[B-AGG-600] Order {} expired before backend batch processing, marking as failed",
                    order.order_id
                );

                if let Err(err) = self
                    .db
                    .set_order_failure(&order.order_id, "Expired before backend batch processing")
                    .await
                {
                    tracing::error!(
                        "Failed to set order {} as failed before backend batch processing: {err}",
                        order.order_id,
                    );
                }
                if let Err(err) = self.proving_completion_tx.try_send(CommitmentComplete {
                    order_id: order.order_id.clone(),
                    chain_id: self.chain_id,
                    outcome: CommitmentOutcome::ProvingFailed,
                }) {
                    tracing::error!(
                        "Failed to send proving failure completion for order {}; capacity tracking may be stale: {err}",
                        order.order_id
                    );
                }
                continue;
            }

            // Check if order has been fulfilled externally.
            // For LockAndFulfill, only filter if the lock has also expired —
            // if the lock is still active, we MUST continue to avoid slashing.
            let should_check_fulfilled = match order.fulfillment_type {
                FulfillmentType::LockAndFulfill => current_time >= order.lock_expiration,
                FulfillmentType::FulfillAfterLockExpire
                | FulfillmentType::FulfillWithoutLocking => true,
            };

            if should_check_fulfilled {
                match self.db.is_request_fulfilled(order.request_id).await {
                    Ok(true) => {
                        tracing::warn!(
                            "[B-AGG-601] Order {} already fulfilled externally, marking as failed",
                            order.order_id
                        );
                        if let Err(err) = self
                            .db
                            .set_order_failure(
                                &order.order_id,
                                "Fulfilled before backend batch processing",
                            )
                            .await
                        {
                            tracing::error!(
                                "Failed to set order {} as failed before backend batch processing: {err}",
                                order.order_id
                            );
                        }
                        if let Err(err) = self.proving_completion_tx.try_send(CommitmentComplete {
                            order_id: order.order_id.clone(),
                            chain_id: self.chain_id,
                            outcome: CommitmentOutcome::ProvingFailed,
                        }) {
                            tracing::error!(
                                "Failed to send proving failure completion for order {}; capacity tracking may be stale: {err}",
                                order.order_id
                            );
                        }
                        continue;
                    }
                    Ok(false) => {}
                    Err(e) => {
                        tracing::warn!(
                            "Failed to check fulfillment for order {}, keeping in batch: {e:?}",
                            order.order_id
                        );
                    }
                }
            }

            valid_orders.push(order);
        }

        Ok(valid_orders)
    }

    /// Claim backend-ready orders, filter non-actionable orders, and keep direct-submit
    /// orders separate so their existing compressed proof handle can be passed to the backend.
    async fn get_filtered_batch_ready_orders(
        &self,
        backend_id: &BackendId,
    ) -> Result<(Vec<BatchReadyOrder>, Vec<BatchReadyOrder>), BatcherErr> {
        let current_time = crate::now_timestamp();

        let batch_update_orders = self
            .db
            .get_pending_batch_orders(backend_id)
            .await
            .context("Failed to get pending backend batch orders")?;
        let direct_submit_orders = self
            .db
            .get_pending_direct_submission_orders(backend_id)
            .await
            .context("Failed to get pending direct-submit orders")?;

        let valid_batch_update_orders =
            self.filter_non_actionable_orders(batch_update_orders, current_time).await?;
        let valid_direct_submit_orders =
            self.filter_non_actionable_orders(direct_submit_orders, current_time).await?;

        Ok((valid_batch_update_orders, valid_direct_submit_orders))
    }

    /// Load full orders for `ids` and project them to the generic [`OrderProvingData`] the
    /// backend consumes, preserving order and failing loudly if any id is missing.
    async fn fetch_proving_data(&self, ids: &[String]) -> Result<Vec<OrderProvingData>> {
        let refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
        let orders = self.db.get_orders(&refs).await.context("Failed to load orders for batch")?;
        let by_id: HashMap<String, Order> =
            orders.into_iter().map(|order| (order.id(), order)).collect();
        ids.iter()
            .map(|id| {
                by_id
                    .get(id)
                    .map(order_proving_data)
                    .with_context(|| format!("Order {id} missing from DB while assembling batch"))
            })
            .collect()
    }

    async fn update_backend_batch(
        &self,
        backend_id: &BackendId,
        batch_id: usize,
        batch: &Batch,
        batch_update_orders: &[BatchReadyOrder],
        direct_submit_orders: &[BatchReadyOrder],
        finalize: bool,
    ) -> Result<BatchUpdate> {
        let ready: Vec<&BatchReadyOrder> =
            batch_update_orders.iter().chain(direct_submit_orders.iter()).collect();
        let ready_ids: Vec<String> = ready.iter().map(|o| o.order_id.clone()).collect();
        let new_orders: Vec<BatchOrder> = self
            .fetch_proving_data(&ready_ids)
            .await?
            .into_iter()
            .zip(ready.iter())
            .map(|(proving, ready)| BatchOrder {
                proving,
                expiration: ready.expiration,
                fee: ready.fee,
                fulfillment_type: ready.fulfillment_type,
                request_id: ready.request_id,
                lock_expiration: ready.lock_expiration,
            })
            .collect();

        // Existing orders are only consumed on the finalize (assessor) path.
        let existing_orders =
            if finalize { self.fetch_proving_data(&batch.orders).await? } else { Vec::new() };

        let update = self
            .backend
            .update_batch(
                backend_id,
                UpdateBatch {
                    batch_id,
                    existing_orders,
                    state: batch.backend_state.clone(),
                    new_orders,
                    finalize,
                },
            )
            .await
            .with_context(|| {
                format!("Failed to update backend batch {batch_id} with orders {:?}", batch.orders)
            })?;

        self.db
            .update_batch(
                batch_id,
                &update.state,
                &[batch_update_orders, direct_submit_orders].concat(),
                update.finalize,
            )
            .await
            .with_context(|| {
                format!(
                    "Failed to update batch {batch_id} with orders {:?} in the DB",
                    batch.orders
                )
            })?;

        Ok(update)
    }

    async fn process_backend_batch(&self, backend_id: &BackendId) -> Result<(), BatcherErr> {
        // Get the current batch. This service works on one backend-owned broker batch at a time,
        // including any newly backend-ready orders that can be added to the current batch.
        let batch_id =
            self.db.get_current_batch(backend_id).await.context("Failed to get current batch")?;
        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        let (compress, batch_update_secs, assessor_secs) = match batch.status {
            BatchStatus::Open => {
                // Claim and filter orders that are ready for backend batch processing.
                let (batch_update_orders, direct_submit_orders) =
                    self.get_filtered_batch_ready_orders(backend_id).await?;

                // Finalize the current batch before adding any new orders if the finalization conditions
                // are already met.
                let finalize = self
                    .check_finalize(
                        backend_id,
                        batch_id,
                        &batch,
                        &[batch_update_orders.clone(), direct_submit_orders.clone()].concat(),
                    )
                    .await?;

                // If we don't need to finalize and there are no new backend batch-update
                // orders, there is no work to do. Direct-submit orders are picked up when the
                // backend batch is finalized.
                if !finalize && batch_update_orders.is_empty() {
                    tracing::trace!("No backend batch work to do for batch {batch_id}");
                    return Ok(());
                }

                let result = self
                    .update_backend_batch(
                        backend_id,
                        batch_id,
                        &batch,
                        &batch_update_orders,
                        &direct_submit_orders,
                        finalize,
                    )
                    .await
                    .context(format!(
                        "Failed to update backend batch {batch_id} with orders {:?}",
                        batch.orders
                    ))?;
                (finalize, result.batch_update_secs, result.assessor_secs)
            }
            BatchStatus::PendingCompression => (true, None, None),
            status => {
                return Err(BatcherErr::UnexpectedErr(anyhow::anyhow!(
                    "Unexpected batch status {status:?}"
                )))
            }
        };

        if compress {
            let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;
            tracing::debug!("Closing batch {batch_id} with orders {:?}", batch.orders);

            let close = match self
                .backend
                .close_batch(
                    backend_id,
                    CloseBatch {
                        batch_id,
                        order_ids: batch.orders.clone(),
                        state: batch.backend_state.clone(),
                    },
                )
                .await
            {
                Ok(close) => close,
                Err(err) => {
                    self.db
                        .set_batch_failure(batch_id, err.to_string())
                        .await
                        .map_err(|e| BatcherErr::UnexpectedErr(e.into()))?;
                    return Err(BatcherErr::CompressionErr(err));
                }
            };
            tracing::debug!("Closed batch {batch_id} with orders {:?}", batch.orders);

            for order_id_str in &batch.orders {
                crate::telemetry::telemetry(self.chain_id).record_backend_batch_completed(
                    order_id_str,
                    batch_update_secs,
                    assessor_secs,
                    Some(close.compression_secs),
                );
            }

            self.db.complete_batch(batch_id, &close.state).await.with_context(|| {
                format!("Failed to set batch {batch_id} with orders {:?} as complete", batch.orders)
            })?;
        }

        Ok(())
    }

    async fn process_batches(&self) -> Result<(), BatcherErr> {
        // Isolate per-backend failures: an error on one backend must not skip the
        // remaining backends for this poll cycle. The batcher loop retries next tick.
        for backend_id in self.backend.backend_ids() {
            if let Err(err) = self.process_backend_batch(&backend_id).await {
                tracing::warn!("Failed to process batch for backend {backend_id}: {err:?}");
            }
        }

        Ok(())
    }
}

impl BrokerService for BatcherService {
    type Error = BatcherErr;

    async fn run(self, cancel_token: CancellationToken) -> Result<(), SupervisorErr<Self::Error>> {
        tracing::debug!("Starting Batcher service");
        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!("Batcher service received cancellation");
                break;
            }

            let conf_poll_time_ms = {
                let config = self
                    .config
                    .lock_all()
                    .context("Failed to lock config")
                    .map_err(BatcherErr::UnexpectedErr)
                    .map_err(SupervisorErr::Recover)?;
                config.batcher.batch_poll_time_ms.unwrap_or(1000)
            };

            self.process_batches().await.map_err(SupervisorErr::Recover)?;
            tokio::time::sleep(tokio::time::Duration::from_millis(conf_poll_time_ms)).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        chain_monitor_v2::ChainMonitorService,
        db::SqliteDb,
        now_timestamp,
        order_committer::CommitmentComplete,
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
    use boundless_market::{
        contracts::{
            Offer, Predicate, ProofRequest, RequestId, RequestInput, RequestInputType, Requirements,
        },
        dynamic_gas_filler::PriorityMode,
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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());
        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));
        let chain_id = provider.get_chain_id().await.unwrap();
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let batcher = BatcherService::new(
            db.clone(),
            chain_id,
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
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
                timeout: 1000,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res_1.id),
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
                timeout: 1000,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig,
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res_2.id),
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        batcher.process_batches().await.unwrap();

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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());
        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));
        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let batcher = BatcherService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(order_request.expires_at()),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            request: order_request,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res_1.id),
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        // Aggregate the first order. Should not finalize.
        batcher.process_batches().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let option_batch = db.get_complete_batch().await.unwrap();
        assert!(option_batch.is_none());

        let aggregating_batch_id = db.get_current_batch(&Risc0Backend::default_id()).await.unwrap();
        let aggregating_batch = db.get_batch(aggregating_batch_id).await.unwrap();
        assert_eq!(aggregating_batch.orders, vec![order.id()]);
        let backend_state = aggregating_batch.backend_state.unwrap();
        let guest_state: risc0_aggregation::GuestState =
            serde_json::from_value(backend_state.0["guest_state"].clone()).unwrap();
        assert!(!guest_state.mmr.is_finalized());

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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(order_request.expires_at()),
            client_sig,
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            request: order_request,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res_2.id),
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        batcher.process_batches().await.unwrap();

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
        let batcher = BatcherService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            prover_addr,
            config,
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
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
                timeout: 1000,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res.id),
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        batcher.process_batches().await.unwrap();

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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());

        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let batcher = BatcherService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            signer.address(),
            config.clone(),
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 100),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res.id),
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        provider.anvil_mine(Some(51), Some(2)).await.unwrap();

        batcher.process_batches().await.unwrap();

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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());

        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));

        let set_builder_id = Digest::from(SET_BUILDER_ID);
        prover.upload_image(&set_builder_id.to_string(), SET_BUILDER_ELF.to_vec()).await.unwrap();
        let assessor_id = Digest::from(ASSESSOR_GUEST_ID);
        prover.upload_image(&assessor_id.to_string(), ASSESSOR_GUEST_ELF.to_vec()).await.unwrap();
        let batcher = BatcherService::new(
            db.clone(),
            provider.get_chain_id().await.unwrap(),
            set_builder_id,
            assessor_id,
            Address::ZERO,
            signer.address(),
            config.clone(),
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
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
                timeout: 1000,
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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request.clone(),
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 1000),
            client_sig: client_sig.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res.id),
            cached_id: Default::default(),
        };

        // add first order and aggregate
        db.add_order(&order).await.unwrap();
        batcher.process_batches().await.unwrap();
        assert!(logs_contain("size estimate below limit 20 < 30"));

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
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request_2,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp: Some(now_timestamp() + 1000),
            client_sig: client_sig_2.into(),
            lock_price: Some(U256::from(min_price)),
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: test_risc0_order_state(&proof_res.id),
            cached_id: Default::default(),
        };

        db.add_order(&order2).await.unwrap();
        batcher.process_batches().await.unwrap();
        assert!(logs_contain("batch size target hit 40 >= 30"));

        let (_, batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert_eq!(batch.orders.len(), 2);
        assert_eq!(batch.status, BatchStatus::PendingSubmission);
    }

    /// Helper to create a test order with the given parameters.
    fn make_test_order(
        request_id_nonce: u32,
        fulfillment_type: FulfillmentType,
        expire_timestamp: Option<u64>,
        ramp_up_start: u64,
        lock_timeout: u32,
        timeout: u32,
    ) -> Order {
        Order {
            status: OrderStatus::ReadyForBatch,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: ProofRequest::new(
                RequestId::new(Address::ZERO, request_id_nonce),
                Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
                "http://risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    rampUpStart: ramp_up_start,
                    timeout,
                    lockTimeout: lock_timeout,
                    rampUpPeriod: 1,
                    lockCollateral: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            backend_id: Some(Risc0Backend::default_id()),
            expire_timestamp,
            client_sig: Bytes::new(),
            lock_price: Some(U256::from(1)),
            fulfillment_type,
            error_msg: None,
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            total_cycles: None,
            journal_bytes: None,
            proving_started_at: None,
            backend_state: None,
            cached_id: Default::default(),
        }
    }

    fn batch_ready_order_from(order: &Order) -> BatchReadyOrder {
        BatchReadyOrder {
            order_id: order.id(),
            expiration: order.request.expires_at(),
            fee: U256::from(10),
            fulfillment_type: order.fulfillment_type,
            request_id: order.request.id,
            lock_expiration: order.request.lock_expires_at(),
        }
    }

    async fn setup_batcher(db: DbObj) -> BatcherService {
        setup_batcher_with_completion_tx(db, mpsc::channel::<CommitmentComplete>(100).0).await
    }

    async fn setup_batcher_with_completion_tx(
        db: DbObj,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> BatcherService {
        BatcherService::new_with_backend_router(
            db,
            ConfigLock::default(),
            Arc::new(BackendRouter::new()),
            1,
            proving_completion_tx,
        )
        .unwrap()
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_orders_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;

        let current_time = crate::now_timestamp();

        // rampUpStart=0, timeout=100 → expires_at()=100 (far in the past)
        let expired_order = make_test_order(
            999,
            FulfillmentType::LockAndFulfill,
            Some(current_time - 100),
            0,
            100,
            100,
        );
        db.add_order(&expired_order).await.unwrap();

        // rampUpStart=current_time, timeout=200 → expires_at()=current_time+200 (future)
        let valid_order = make_test_order(
            1000,
            FulfillmentType::LockAndFulfill,
            Some(current_time + 200),
            current_time,
            100,
            200,
        );
        db.add_order(&valid_order).await.unwrap();

        let orders =
            vec![batch_ready_order_from(&expired_order), batch_ready_order_from(&valid_order)];

        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, valid_order.id());

        // Check that expired order was marked as failed
        let db_expired_order = db.get_order(&expired_order.id()).await.unwrap().unwrap();
        assert_eq!(db_expired_order.status, OrderStatus::Failed);
        assert_eq!(
            db_expired_order.error_msg,
            Some("Expired before backend batch processing".to_string())
        );

        // Check that valid order is unchanged
        let db_valid_order = db.get_order(&valid_order.id()).await.unwrap().unwrap();
        assert_eq!(db_valid_order.status, OrderStatus::ReadyForBatch);
        assert!(db_valid_order.error_msg.is_none());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_after_lock_expire_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let (completion_tx, mut completion_rx) = mpsc::channel::<CommitmentComplete>(100);
        let batcher_service = setup_batcher_with_completion_tx(db.clone(), completion_tx).await;
        let current_time = crate::now_timestamp();

        // FulfillAfterLockExpire order that has been fulfilled externally
        // rampUpStart=current_time, timeout=500 → expires_at()=current_time+500 (future)
        let order = make_test_order(
            100,
            FulfillmentType::FulfillAfterLockExpire,
            Some(current_time + 500),
            current_time,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();

        // Mark the request as fulfilled
        db.set_request_fulfilled(order.request.id, 1).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled externally
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(
            db_order.error_msg,
            Some("Fulfilled before backend batch processing".to_string())
        );

        let completion = completion_rx.try_recv().expect("fulfilled order should free capacity");
        assert_eq!(completion.order_id, order.id());
        assert_eq!(completion.chain_id, 1);
        assert!(matches!(completion.outcome, CommitmentOutcome::ProvingFailed));
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_after_lock_expire_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // FulfillAfterLockExpire order that has NOT been fulfilled
        // rampUpStart=current_time, timeout=500 → expires_at()=current_time+500 (future)
        let order = make_test_order(
            101,
            FulfillmentType::FulfillAfterLockExpire,
            Some(current_time + 500),
            current_time,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_fulfilled_lock_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // LockAndFulfill with lock already expired but request still valid:
        // rampUpStart=current_time-200, lockTimeout=100 → lock_expires_at=current_time-100 (past)
        // timeout=500 → expires_at()=current_time+300 (future)
        let order = make_test_order(
            200,
            FulfillmentType::LockAndFulfill,
            Some(current_time + 300),
            current_time - 200,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();
        assert!(order.request.lock_expires_at() < current_time);
        assert!(order.request.expires_at() > current_time);

        // Mark the request as fulfilled
        db.set_request_fulfilled(order.request.id, 1).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled AND lock expired
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(
            db_order.error_msg,
            Some("Fulfilled before backend batch processing".to_string())
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_fulfilled_lock_not_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // LockAndFulfill with lock still active:
        // rampUpStart=current_time, lockTimeout=1000 → lock_expires_at=current_time+1000 (future)
        // timeout=2000 → expires_at()=current_time+2000 (future)
        let order = make_test_order(
            201,
            FulfillmentType::LockAndFulfill,
            Some(current_time + 2000),
            current_time,
            1000,
            2000,
        );
        db.add_order(&order).await.unwrap();
        assert!(order.request.lock_expires_at() > current_time);

        // Mark the request as fulfilled
        db.set_request_fulfilled(order.request.id, 1).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be KEPT — lock still active, we must continue to avoid slashing
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // LockAndFulfill NOT fulfilled
        // rampUpStart=current_time, timeout=500 → expires_at()=current_time+500 (future)
        let order = make_test_order(
            202,
            FulfillmentType::LockAndFulfill,
            Some(current_time + 500),
            current_time,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_without_locking_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // rampUpStart=current_time, timeout=500 → expires_at()=current_time+500 (future)
        let order = make_test_order(
            300,
            FulfillmentType::FulfillWithoutLocking,
            Some(current_time + 500),
            current_time,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();

        // Mark the request as fulfilled
        db.set_request_fulfilled(order.request.id, 1).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled externally
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(
            db_order.error_msg,
            Some("Fulfilled before backend batch processing".to_string())
        );
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_without_locking_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // rampUpStart=current_time, timeout=500 → expires_at()=current_time+500 (future)
        let order = make_test_order(
            301,
            FulfillmentType::FulfillWithoutLocking,
            Some(current_time + 500),
            current_time,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();

        let orders = vec![batch_ready_order_from(&order)];
        let valid_orders =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_lock_expired_request_valid() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let batcher_service = setup_batcher(db.clone()).await;
        let current_time = crate::now_timestamp();

        // Lock expired but request still valid, NOT fulfilled — this is the key scenario
        // rampUpStart=current_time-200, lockTimeout=100 → lock_expires_at=current_time-100 (past)
        // timeout=500 → expires_at()=current_time+300 (future)
        let order = make_test_order(
            400,
            FulfillmentType::LockAndFulfill,
            Some(current_time + 300),
            current_time - 200,
            100,
            500,
        );
        db.add_order(&order).await.unwrap();
        assert!(order.request.lock_expires_at() < current_time);
        assert!(order.request.expires_at() > current_time);

        let orders = vec![batch_ready_order_from(&order)];
        let valid =
            batcher_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be KEPT — lock expired but request still valid and not fulfilled
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].order_id, order.id());
    }
}
