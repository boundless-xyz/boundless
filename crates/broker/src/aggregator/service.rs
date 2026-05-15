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
#[cfg(test)]
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::{backend::Risc0BatchProcessor, provers::ProverObj};
use crate::{
    backend::{BackendId, BatchProcessorObj, BatchUpdate, CloseBatch, UpdateBatch},
    config::ConfigLock,
    db::{AggregationOrder, DbObj},
    now_timestamp,
    order_committer::{CommitmentComplete, CommitmentOutcome},
    task::{BrokerService, SupervisorErr},
    Batch, BatchStatus, FulfillmentType,
};

use super::error::AggregatorErr;
#[derive(Clone)]
pub struct AggregatorService {
    db: DbObj,
    config: ConfigLock,
    backend_id: BackendId,
    batch_backend: BatchProcessorObj,
    chain_id: u64,
    /// Sends ProvingFailed to the OrderCommitter to free the global proving capacity slot.
    proving_completion_tx: mpsc::Sender<CommitmentComplete>,
}

impl AggregatorService {
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
        let backend_id = BackendId::new("risc0_v3")?;
        let batch_backend = Arc::new(Risc0BatchProcessor::new(
            db.clone(),
            config.clone(),
            prover.clone(),
            set_builder_guest_id,
            assessor_guest_id,
            market_addr,
            prover_addr,
            chain_id,
        ));

        Ok(Self::new_with_batch_processor(
            db,
            config,
            backend_id,
            batch_backend,
            chain_id,
            proving_completion_tx,
        ))
    }

    pub fn new_with_batch_processor(
        db: DbObj,
        config: ConfigLock,
        backend_id: BackendId,
        batch_backend: BatchProcessorObj,
        chain_id: u64,
        proving_completion_tx: mpsc::Sender<CommitmentComplete>,
    ) -> Self {
        Self { db, config, backend_id, batch_backend, chain_id, proving_completion_tx }
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

        // Historical config name: for RISC0 this estimate is journal bytes. More generally this is
        // a backend-estimated batch size used to cap submission payload growth.
        let batch_size_estimate = self.batch_backend.estimate_batch_size(&batch.orders).await?.size;
        let pending_order_ids: Vec<_> = pending_orders.iter().map(|o| o.order_id.clone()).collect();
        let pending_size_estimate =
            self.batch_backend.estimate_batch_size(&pending_order_ids).await?.size;
        let total_size_estimate = batch_size_estimate + pending_size_estimate;
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
                let _ = self.proving_completion_tx.try_send(CommitmentComplete {
                    order_id: order.order_id.clone(),
                    chain_id: self.chain_id,
                    outcome: CommitmentOutcome::ProvingFailed,
                });
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
                            .set_order_failure(&order.order_id, "Fulfilled before aggregation")
                            .await
                        {
                            tracing::error!(
                                "Failed to set order {} as failed: {err}",
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

    /// Get all pending proofs, filter expired orders, and return both aggregation and groth16 proofs separately
    async fn get_filtered_pending_proofs(
        &self,
    ) -> Result<(Vec<AggregationOrder>, Vec<AggregationOrder>), AggregatorErr> {
        let current_time = crate::now_timestamp();

        // Get both types of proofs
        let new_proofs = self
            .db
            .get_aggregation_proofs(&self.backend_id)
            .await
            .context("Failed to get aggregation proofs")?;
        let groth16_proofs = self
            .db
            .get_groth16_proofs(&self.backend_id)
            .await
            .context("Failed to get groth16 proofs")?;

        // Filter expired orders from both lists
        let valid_new_proofs = self.filter_non_actionable_orders(new_proofs, current_time).await?;
        let valid_groth16_proofs =
            self.filter_non_actionable_orders(groth16_proofs, current_time).await?;

        Ok((valid_new_proofs, valid_groth16_proofs))
    }

    async fn aggregate_proofs(
        &self,
        batch_id: usize,
        batch: &Batch,
        new_proofs: &[AggregationOrder],
        new_groth16_proofs: &[AggregationOrder],
        finalize: bool,
    ) -> Result<BatchUpdate> {
        let update = self
            .batch_backend
            .update_batch(UpdateBatch {
                batch_id,
                existing_order_ids: batch.orders.clone(),
                aggregation_state: batch.aggregation_state.clone(),
                new_proofs: new_proofs.to_vec(),
                new_compressed_proofs: new_groth16_proofs.to_vec(),
                finalize,
            })
            .await
            .with_context(|| {
                format!("Failed to update backend batch {batch_id} with orders {:?}", batch.orders)
            })?;

        self.db
            .update_batch(
                batch_id,
                &update.aggregation_state,
                &[new_proofs, new_groth16_proofs].concat(),
                update.assessor_proof_id.clone(),
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

    async fn aggregate(&self) -> Result<(), AggregatorErr> {
        // Get the current batch. This aggregator service works on one batch at a time, including
        // any proofs ready for aggregation into the current batch.
        let batch_id = self
            .db
            .get_current_batch(&self.backend_id)
            .await
            .context("Failed to get current batch")?;
        let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;

        let (aggregation_proof_id, compress, set_builder_proving_secs, assessor_proving_secs) =
            match batch.status {
                BatchStatus::Aggregating => {
                    // Get and filter all pending proofs
                    let (new_proofs, new_groth16_proofs) =
                        self.get_filtered_pending_proofs().await?;

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

                    let result = self
                        .aggregate_proofs(
                            batch_id,
                            &batch,
                            &new_proofs,
                            &new_groth16_proofs,
                            finalize,
                        )
                        .await
                        .context(format!(
                            "Failed to aggregate proofs for batch {batch_id} with orders {:?}",
                            batch.orders
                        ))?;
                    (
                        result.aggregation_state.proof_id,
                        finalize,
                        result.set_builder_proving_secs,
                        result.assessor_proving_secs,
                    )
                }
                BatchStatus::PendingCompression => {
                    let aggregation_state = batch.aggregation_state.with_context(|| format!("Batch {batch_id} in inconsistent state: status is PendingCompression but aggregation_state is None"))?;
                    (aggregation_state.proof_id, true, None, None)
                }
                status => {
                    return Err(AggregatorErr::UnexpectedErr(anyhow::anyhow!(
                        "Unexpected batch status {status:?}"
                    )))
                }
            };

        if compress {
            let batch = self.db.get_batch(batch_id).await.context("Failed to get batch")?;
            tracing::debug!("Closing batch {batch_id} with orders {:?}", batch.orders);

            let close = match self
                .batch_backend
                .close_batch(CloseBatch {
                    batch_id,
                    aggregation_proof_id,
                    order_ids: batch.orders.clone(),
                })
                .await
            {
                Ok(close) => close,
                Err(err) => {
                    self.db
                        .set_batch_failure(batch_id, err.to_string())
                        .await
                        .map_err(|e| AggregatorErr::UnexpectedErr(e.into()))?;
                    return Err(AggregatorErr::CompressionErr(err));
                }
            };
            tracing::debug!("Closed batch {batch_id} with orders {:?}", batch.orders);

            for order_id_str in &batch.orders {
                crate::telemetry::telemetry(self.chain_id).record_aggregation_completed(
                    order_id_str,
                    set_builder_proving_secs,
                    assessor_proving_secs,
                    Some(close.compression_secs),
                );
            }

            self.db.complete_batch(batch_id, &close.compressed_proof_id).await.with_context(
                || {
                    format!(
                        "Failed to set batch {batch_id} with orders {:?} as complete",
                        batch.orders
                    )
                },
            )?;
        }

        Ok(())
    }
}

impl BrokerService for AggregatorService {
    type Error = AggregatorErr;

    async fn run(self, cancel_token: CancellationToken) -> Result<(), SupervisorErr<Self::Error>> {
        tracing::debug!("Starting Aggregator service");
        loop {
            if cancel_token.is_cancelled() {
                tracing::debug!("Aggregator service received cancellation");
                break;
            }

            let conf_poll_time_ms = {
                let config = self
                    .config
                    .lock_all()
                    .context("Failed to lock config")
                    .map_err(AggregatorErr::UnexpectedErr)
                    .map_err(SupervisorErr::Recover)?;
                config.batcher.batch_poll_time_ms.unwrap_or(1000)
            };

            self.aggregate().await.map_err(SupervisorErr::Recover)?;
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
        let aggregator = AggregatorService::new(
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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res_1.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str),
            input_id: Some(input_id),
            proof_id: Some(proof_res_2.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());
        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));
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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res_1.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            cached_id: Default::default(),
        };
        db.add_order(&order).await.unwrap();

        // Aggregate the first order. Should not finalize.
        aggregator.aggregate().await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);

        let option_batch = db.get_complete_batch().await.unwrap();
        assert!(option_batch.is_none());

        let aggregating_batch_id =
            db.get_current_batch(&BackendId::new("risc0_v3").unwrap()).await.unwrap();
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
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());

        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));

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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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

        let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::default()));
        let chain_monitor =
            Arc::new(ChainMonitorService::new(provider.clone(), gas_priority_mode).await.unwrap());

        let _handle = tokio::spawn((*chain_monitor).clone().run(CancellationToken::new()));

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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request.clone(),
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.clone().id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            cached_id: Default::default(),
        };

        // add first order and aggregate
        db.add_order(&order).await.unwrap();
        aggregator.aggregate().await.unwrap();
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
            status: OrderStatus::PendingAgg,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: order_request_2,
            image_id: Some(image_id_str.clone()),
            input_id: Some(input_id.clone()),
            proof_id: Some(proof_res.id),
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            cached_id: Default::default(),
        };

        db.add_order(&order2).await.unwrap();
        aggregator.aggregate().await.unwrap();
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
            status: OrderStatus::PendingAgg,
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
            proof_id: None,
            compressed_proof_id: None,
            backend_id: Some(BackendId::new("risc0_v3").unwrap()),
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
            cached_id: Default::default(),
        }
    }

    fn agg_order_from(order: &Order) -> AggregationOrder {
        AggregationOrder {
            order_id: order.id(),
            proof_id: "proof".to_string(),
            expiration: order.request.expires_at(),
            fee: U256::from(10),
            fulfillment_type: order.fulfillment_type,
            request_id: order.request.id,
            lock_expiration: order.request.lock_expires_at(),
        }
    }

    async fn setup_aggregator(db: DbObj) -> AggregatorService {
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());

        AggregatorService::new(
            db,
            1,
            Digest::ZERO,
            Digest::ZERO,
            Address::ZERO,
            Address::ZERO,
            config,
            prover,
            mpsc::channel::<CommitmentComplete>(100).0,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_orders_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;

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

        let orders = vec![agg_order_from(&expired_order), agg_order_from(&valid_order)];

        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

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

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_after_lock_expire_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled externally
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some("Fulfilled before aggregation".to_string()));
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_after_lock_expire_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_fulfilled_lock_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled AND lock expired
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some("Fulfilled before aggregation".to_string()));
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_fulfilled_lock_not_expired() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be KEPT — lock still active, we must continue to avoid slashing
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_without_locking_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be filtered out — fulfilled externally
        assert_eq!(valid_orders.len(), 0);

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some("Fulfilled before aggregation".to_string()));
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_fulfill_without_locking_not_fulfilled() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid_orders =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be kept — not fulfilled
        assert_eq!(valid_orders.len(), 1);
        assert_eq!(valid_orders[0].order_id, order.id());
    }

    #[tokio::test]
    #[traced_test]
    async fn filter_non_actionable_lock_and_fulfill_lock_expired_request_valid() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let aggregator_service = setup_aggregator(db.clone()).await;
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

        let orders = vec![agg_order_from(&order)];
        let valid =
            aggregator_service.filter_non_actionable_orders(orders, current_time).await.unwrap();

        // Should be KEPT — lock expired but request still valid and not fulfilled
        assert_eq!(valid.len(), 1);
        assert_eq!(valid[0].order_id, order.id());
    }
}
