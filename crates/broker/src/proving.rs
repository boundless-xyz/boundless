// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    config::ConfigLock,
    db::DbObj,
    futures_retry::retry,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Order, OrderStatus,
};
use alloy::primitives::U256;
use anyhow::{Context, Result};

#[derive(Clone)]
pub struct ProvingService {
    db: DbObj,
    prover: ProverObj,
    config: ConfigLock,
}

impl ProvingService {
    pub async fn new(db: DbObj, prover: ProverObj, config: ConfigLock) -> Result<Self> {
        Ok(Self { db, prover, config })
    }

    pub async fn monitor_proof(
        &self,
        order_id: U256,
        stark_proof_id: &str,
        is_unaggregated: bool,
        snark_proof_id: Option<String>,
    ) -> Result<()> {
        let proof_res = self
            .prover
            .wait_for_stark(stark_proof_id)
            .await
            .context("Monitoring proof (stark) failed")?;

        if is_unaggregated && snark_proof_id.is_none() {
            let compressed_proof_id =
                self.prover.compress(stark_proof_id).await.context("Failed to compress proof")?;
            self.db
                .set_order_compressed_proof_id(order_id, &compressed_proof_id)
                .await
                .with_context(|| {
                    format!(
                        "Failed to set order {order_id:x} compressed proof id: {compressed_proof_id}"
                    )
                })?;
        };

        let status = match is_unaggregated {
            false => OrderStatus::PendingAgg,
            true => OrderStatus::SkipAggregation,
        };

        self.db
            .set_aggregation_status(order_id, status)
            .await
            .with_context(|| format!("Failed to set the DB record to aggregation {order_id:x}"))?;

        tracing::info!(
            "Customer Proof complete, order_id: {order_id:x} cycles: {} time: {}",
            proof_res.stats.total_cycles,
            proof_res.elapsed_time,
        );

        Ok(())
    }

    pub async fn prove_order(&self, order_id: U256, order: Order) -> Result<()> {
        let (max_file_size, fetch_retries) = {
            let config = self.config.lock_all().context("Failed to read config")?;
            (config.market.max_file_size, config.market.max_fetch_retries)
        };

        // If the ID's are not present then upload them now
        // Mostly hit by skipping pre-flight
        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => crate::upload_image_uri(&self.prover, &order, max_file_size, fetch_retries)
                .await
                .context("Failed to upload image")?,
        };
        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => crate::upload_input_uri(&self.prover, &order, max_file_size, fetch_retries)
                .await
                .context("Failed to upload input")?,
        };

        tracing::info!("Proving order {order_id:x}");

        let proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![])
            .await
            .context("Failed to prove customer proof STARK order")?;

        self.db
            .set_order_proof_id(order_id, &proof_id)
            .await
            .with_context(|| format!("Failed to set order {order_id:x} proof id: {}", proof_id))?;

        self.monitor_proof(order_id, &proof_id, order.is_unaggregated(), None).await?;

        Ok(())
    }

    pub async fn find_and_monitor_proofs(&self) -> Result<()> {
        let current_proofs = self
            .db
            .get_active_proofs()
            .await
            .context("Failed to get active proofs from the DB")
            .map_err(SupervisorErr::Fault)?;

        tracing::info!("Found {} proofs currently proving", current_proofs.len());
        for (order_id, order) in current_proofs {
            let prove_serv = self.clone();
            let Some(proof_id) = order.proof_id.clone() else {
                tracing::error!("Order in status Proving missing proof_id: {order_id:x}");
                if let Err(inner_err) = prove_serv
                    .db
                    .set_order_failure(order_id, "Proving status missing proof_id".into())
                    .await
                {
                    tracing::error!("Failed to set order {order_id:x} failure: {inner_err:?}");
                }
                continue;
            };
            let is_unaggregated = order.is_unaggregated();
            let compressed_proof_id = order.compressed_proof_id;
            // TODO: Manage these tasks in a joinset?
            // They should all be fail-able without triggering a larger failure so it should be
            // fine.
            tokio::spawn(async move {
                match prove_serv
                    .monitor_proof(order_id, &proof_id, is_unaggregated, compressed_proof_id)
                    .await
                {
                    Ok(_) => tracing::info!("Successfully complete order proof {order_id:x}"),
                    Err(err) => {
                        tracing::error!("FATAL: Order failed to prove: {err:?}");
                        if let Err(inner_err) =
                            prove_serv.db.set_order_failure(order_id, format!("{err:?}")).await
                        {
                            tracing::error!(
                                "Failed to set order {order_id:x} failure: {inner_err:?}"
                            );
                        }
                    }
                }
            });
        }

        Ok(())
    }
}

impl RetryTask for ProvingService {
    fn spawn(&self) -> RetryRes {
        let proving_service_copy = self.clone();
        Box::pin(async move {
            tracing::info!("Starting proving service");

            // First search the DB for any existing dangling proofs and kick off their concurrent
            // monitors
            proving_service_copy.find_and_monitor_proofs().await.map_err(SupervisorErr::Fault)?;

            // Start monitoring for new proofs
            loop {
                // TODO: parallel_proofs management
                // we need to query the Bento/Bonsai backend and constrain the number of running
                // parallel proofs currently bonsai does not have this feature but
                // we could add it to both to support it. Alternatively we could
                // track it in our local DB but that could de-sync from the proving-backend so
                // its not ideal
                let order_res = proving_service_copy
                    .db
                    .get_proving_order()
                    .await
                    .map_err(|err| SupervisorErr::Recover(err.into()))?;

                if let Some((order_id, order)) = order_res {
                    let prov_serv = proving_service_copy.clone();
                    tokio::spawn(async move {
                        let (proof_retry_count, proof_retry_sleep_ms) = {
                            let config = prov_serv.config.lock_all().unwrap();
                            (config.prover.proof_retry_count, config.prover.proof_retry_sleep_ms)
                        };

                        match retry(
                            proof_retry_count,
                            proof_retry_sleep_ms,
                            || async { prov_serv.prove_order(order_id, order.clone()).await },
                            "prove_order",
                        )
                        .await
                        {
                            Ok(_) => {
                                tracing::info!("Successfully complete order proof {order_id:x}");
                            }
                            Err(err) => {
                                tracing::error!(
                                    "FATAL: Order {} failed to prove after {} retries: {err:?}",
                                    order_id,
                                    proof_retry_count
                                );
                                if let Err(inner_err) = prov_serv
                                    .db
                                    .set_order_failure(order_id, format!("{err:?}"))
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to set order {} failure: {inner_err:?}",
                                        order_id
                                    );
                                }
                            }
                        }
                    });
                }

                // TODO: configuration
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
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
        OrderStatus,
    };
    use alloy::primitives::{Bytes, U256};
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use chrono::Utc;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::sha::Digest;
    use std::sync::Arc;
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn prove_order() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();
        let prover: ProverObj = Arc::new(DefaultProver::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        let proving_service =
            ProvingService::new(db.clone(), prover, config.clone()).await.unwrap();

        let order_id = U256::ZERO;
        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::Locking,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: ProofRequest {
                id: U256::ZERO,
                requirements: Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                imageUrl: "http://risczero.com/image".into(),
                input: Input { inputType: InputType::Inline, data: Default::default() },
                offer: Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: now_timestamp(),
                    rampUpPeriod: 1,
                    lockTimeout: 100,
                    timeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };

        db.add_order(order_id, order.clone()).await.unwrap();

        proving_service.prove_order(order_id, order).await.unwrap();

        let order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
    }

    #[tokio::test]
    #[traced_test]
    async fn resume_proving() {
        let db: DbObj = Arc::new(SqliteDb::new("sqlite::memory:").await.unwrap());
        let config = ConfigLock::default();

        let prover: ProverObj = Arc::new(DefaultProver::new());

        let image_id = Digest::from(ECHO_ID).to_string();
        prover.upload_image(&image_id, ECHO_ELF.to_vec()).await.unwrap();
        let input_id = prover
            .upload_input(encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap())
            .await
            .unwrap();

        // pre-prove the stark so it already exists before the service comes up
        let proof_id = prover.prove_stark(&image_id, &input_id, vec![]).await.unwrap();

        let proving_service =
            ProvingService::new(db.clone(), prover, config.clone()).await.unwrap();

        let order_id = U256::ZERO;
        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::Proving,
            updated_at: Utc::now(),
            target_timestamp: Some(0),
            request: ProofRequest {
                id: order_id,
                requirements: Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                imageUrl: "http://risczero.com/image".into(),
                input: Input { inputType: InputType::Inline, data: Default::default() },
                offer: Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: now_timestamp(),
                    rampUpPeriod: 1,
                    timeout: 100,
                    lockTimeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: Some(proof_id.clone()),
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };
        let order_id = U256::from(order_id);
        db.add_order(order_id, order.clone()).await.unwrap();

        proving_service.find_and_monitor_proofs().await.unwrap();

        // Sleep long enough for the tokio tasks to pickup and complete the order in the DB
        loop {
            let db_order = db.get_order(order_id).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Proving {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        }

        let order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
        assert_eq!(order.proof_id, Some(proof_id));

        assert!(logs_contain("Found 1 proofs currently proving"));
    }
}
