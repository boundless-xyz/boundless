// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    config::ConfigLock,
    db::DbObj,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Order,
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

    pub async fn monitor_proof(&self, order_id: U256, proof_id: String) -> Result<()> {
        let proof_res =
            self.prover.wait_for_stark(&proof_id).await.context("Monitoring proof failed")?;

        self.db
            .set_aggregation_status(order_id)
            .await
            .with_context(|| format!("Failed to set the DB record to aggregation {order_id}"))?;

        tracing::info!("Customer Proof complete, cycles: {}", proof_res.stats.total_cycles);

        Ok(())
    }

    pub async fn prove_order(&self, order_id: U256, order: Order) -> Result<()> {
        let max_file_size = {
            let config = self.config.lock_all().context("Failed to read config")?;
            config.market.max_file_size
        };

        // If the ID's are not present then upload them now
        // Mostly hit by skipping pre-flight
        let image_id = match order.image_id.as_ref() {
            Some(val) => val.clone(),
            None => crate::upload_image_uri(&self.prover, &order, max_file_size)
                .await
                .context("Failed to upload image")?,
        };
        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => crate::upload_input_uri(&self.prover, &order, max_file_size)
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

        self.monitor_proof(order_id, proof_id).await?;

        Ok(())
    }

    pub async fn find_and_monitor_proofs(&self) -> Result<()> {
        let current_proofs = self
            .db
            .get_active_proofs()
            .await
            .context("Failed to get active proofs from the DB")
            .map_err(|err| SupervisorErr::Fault(err))?;

        tracing::info!("Found {} proofs currently proving", current_proofs.len());
        for (order_id, order) in current_proofs {
            let prove_serv = self.clone();
            let Some(proof_id) = order.proof_id else {
                tracing::error!("Order in status Proving missing proof_id: {order_id:x}");
                if let Err(inner_err) = prove_serv
                    .db
                    .set_order_failure(order_id, "Proving status missing proof_id".into())
                    .await
                {
                    tracing::error!("Failed to set order {order_id} failure: {inner_err:?}");
                }
                continue;
            };
            // TODO: Manage these tasks in a joinset?
            // They should all be fail-able without triggering a larger failure so it should be
            // fine.
            tokio::spawn(async move {
                match prove_serv.monitor_proof(order_id, proof_id).await {
                    Ok(_) => tracing::info!("Successfully complete order proof {order_id}"),
                    Err(err) => {
                        tracing::error!("FATAL: Order failed to prove: {err:?}");
                        if let Err(inner_err) =
                            prove_serv.db.set_order_failure(order_id, format!("{err:?}")).await
                        {
                            tracing::error!(
                                "Failed to set order {order_id} failure: {inner_err:?}"
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
                        match prov_serv.prove_order(order_id, order).await {
                            Ok(_) => {
                                tracing::info!("Successfully complete order proof {order_id:x}")
                            }
                            Err(err) => {
                                tracing::error!("FATAL: Order failed to prove: {err:?}");
                                if let Err(inner_err) = prov_serv
                                    .db
                                    .set_order_failure(order_id, format!("{err:?}"))
                                    .await
                                {
                                    tracing::error!(
                                        "Failed to set order {order_id:x} failure: {inner_err:?}"
                                    );
                                }
                            }
                        };
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
        provers::{encode_input, MockProver},
        OrderStatus,
    };
    use alloy::primitives::{Bytes, B256};
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProvingRequest, Requirements,
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
        let prover: ProverObj = Arc::new(MockProver::default());

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
            target_block: Some(0),
            request: ProvingRequest {
                id: U256::ZERO,
                requirements: Requirements {
                    imageId: B256::ZERO,
                    predicate: Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                },
                imageUrl: "http://risczero.com/image".into(),
                input: Input { inputType: InputType::Inline, data: Default::default() },
                offer: Offer {
                    minPrice: min_price,
                    maxPrice: max_price,
                    biddingStart: 4,
                    rampUpPeriod: 1,
                    timeout: 100,
                    lockinStake: 10,
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: None,
            expire_block: None,
            path: None,
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

        let prover: ProverObj = Arc::new(MockProver::default());

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
            target_block: Some(0),
            request: ProvingRequest {
                id: U256::ZERO,
                requirements: Requirements {
                    imageId: B256::ZERO,
                    predicate: Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                },
                imageUrl: "http://risczero.com/image".into(),
                input: Input { inputType: InputType::Inline, data: Default::default() },
                offer: Offer {
                    minPrice: min_price,
                    maxPrice: max_price,
                    biddingStart: 4,
                    rampUpPeriod: 1,
                    timeout: 100,
                    lockinStake: 10,
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: Some(proof_id.clone()),
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };
        db.add_order(order_id, order.clone()).await.unwrap();

        proving_service.find_and_monitor_proofs().await.unwrap();

        // Sleep long enough for the tokio tasks to pickup and complete the order in the DB
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
        assert_eq!(order.proof_id, Some(proof_id));

        assert!(logs_contain("Found 1 proofs currently proving"));
    }
}
