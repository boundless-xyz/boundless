// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    config::ConfigLock,
    db::DbObj,
    decode_input,
    provers::ProverObj,
    task::{RetryRes, RetryTask, SupervisorErr},
    Order,
};
use alloy::primitives::U256;
use anyhow::{bail, Context, Result};
use boundless_resolve::{AssumptionReceipt, ResolveInput};
use risc0_zkvm::sha::{Digest, Digestible};
use risc0_zkvm::Assumption;

#[derive(Clone)]
pub struct ProvingService {
    db: DbObj,
    prover: ProverObj,
    config: ConfigLock,
    set_builder_guest_id: Digest,
    resolve_guest_id: Digest,
}

impl ProvingService {
    pub async fn new(
        db: DbObj,
        prover: ProverObj,
        config: ConfigLock,
        set_builder_guest_id: Digest,
        set_builder_guest: Vec<u8>,
        resolve_guest_id: Digest,
        resolve_guest: Vec<u8>,
    ) -> Result<Self> {
        prover
            .upload_image(&set_builder_guest_id.to_string(), set_builder_guest)
            .await
            .context("Failed to upload resolve guest")?;

        prover
            .upload_image(&resolve_guest_id.to_string(), resolve_guest)
            .await
            .context("Failed to upload resolve guest")?;

        Ok(Self { db, prover, config, set_builder_guest_id, resolve_guest_id })
    }

    pub async fn monitor_proof(&self, order_id: U256, proof_id: String) -> Result<()> {
        let proof_res =
            self.prover.wait_for_stark(&proof_id).await.context("Monitoring proof failed")?;

        self.db
            .set_aggregation_status(order_id)
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
        let guest_env = decode_input(&order, max_file_size, fetch_retries)
            .await
            .context("Failed to decode order input")?;
        let input_id = match order.input_id.as_ref() {
            Some(val) => val.clone(),
            None => {
                self.prover.upload_input(guest_env.stdin).await.context("Failed to upload input")?
            }
        };

        tracing::info!("Proving order {order_id:x}");

        let unresolved = guest_env
            .assumptions
            .iter()
            .map(|receipt| Assumption { claim: receipt.claim.digest(), control_root: Digest::ZERO })
            .collect::<Vec<_>>();

        let mut proof_id = self
            .prover
            .prove_stark(&image_id, &input_id, /* TODO assumptions */ vec![], unresolved)
            .await
            .context("Failed to prove customer proof STARK order")?;

        if !guest_env.assumptions.is_empty() {
            let assumption = match guest_env.assumptions {
                vec if vec.len() == 1 => vec.into_iter().next().unwrap(),
                _ => bail!("expected exactly one assumption"),
            };

            let proof_res =
                self.prover.wait_for_stark(&proof_id).await.context("Monitoring proof failed")?;

            tracing::info!(
                "Conditional proof completed for order {}, cycles: {} time: {}",
                order_id,
                proof_res.stats.total_cycles,
                proof_res.elapsed_time
            );

            let receipt = self
                .prover
                .get_receipt(&proof_res.id)
                .await
                .with_context(|| format!("Failed to get proof receipt for {proof_id}"))?
                .with_context(|| format!("Proof receipt not found for {proof_id}"))?;
            let claim = receipt
                .claim()
                .with_context(|| format!("Receipt for {proof_id} missing claim"))?
                .value()
                .with_context(|| format!("Receipt for {proof_id} claims pruned"))?;

            let resolve_input = ResolveInput {
                set_builder_image_id: self.set_builder_guest_id,
                conditional: claim,
                assumption: AssumptionReceipt::SetInclusion(assumption),
            };
            let input_data = resolve_input.to_vec();

            let input_id = self
                .prover
                .upload_input(input_data)
                .await
                .context("Failed to upload resolve input")?;

            proof_id = self
                .prover
                .prove_stark(
                    &self.resolve_guest_id.to_string(),
                    &input_id,
                    vec![proof_res.id],
                    vec![],
                )
                .await
                .context("Failed to prove customer proof STARK order")?;
        }

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
            .map_err(SupervisorErr::Fault)?;

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
                    tracing::error!("Failed to set order {order_id:x} failure: {inner_err:?}");
                }
                continue;
            };
            // TODO: Manage these tasks in a joinset?
            // They should all be fail-able without triggering a larger failure so it should be
            // fine.
            tokio::spawn(async move {
                match prove_serv.monitor_proof(order_id, proof_id).await {
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
    use alloy::primitives::{Bytes, B256, U256};
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use boundless_market::input::GuestEnv;
    use chrono::Utc;
    use guest_resolve::{RESOLVE_GUEST_ELF, RESOLVE_GUEST_ID};
    use guest_set_builder::{SET_BUILDER_ELF, SET_BUILDER_ID};
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

        let proving_service = ProvingService::new(
            db.clone(),
            prover,
            config.clone(),
            Digest::from(SET_BUILDER_ID),
            SET_BUILDER_ELF.to_vec(),
            Digest::from(RESOLVE_GUEST_ID),
            RESOLVE_GUEST_ELF.to_vec(),
        )
        .await
        .unwrap();

        let order_id = U256::ZERO;
        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::Locking,
            updated_at: Utc::now(),
            target_block: Some(0),
            request: ProofRequest {
                id: U256::ZERO,
                requirements: Requirements {
                    imageId: B256::ZERO,
                    predicate: Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                },
                imageUrl: "http://risczero.com/image".into(),
                input: Input {
                    inputType: InputType::Inline,
                    data: GuestEnv::default().encode().unwrap().into(),
                },
                offer: Offer {
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 4,
                    rampUpPeriod: 1,
                    lockTimeout: 100,
                    timeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: None,
            expire_block: None,
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
        let proof_id = prover.prove_stark(&image_id, &input_id, vec![], vec![]).await.unwrap();

        let proving_service = ProvingService::new(
            db.clone(),
            prover,
            config.clone(),
            Digest::from(SET_BUILDER_ID),
            SET_BUILDER_ELF.to_vec(),
            Digest::from(RESOLVE_GUEST_ID),
            RESOLVE_GUEST_ELF.to_vec(),
        )
        .await
        .unwrap();

        let order_id = U256::ZERO;
        let min_price = 2;
        let max_price = 4;

        let order = Order {
            status: OrderStatus::Proving,
            updated_at: Utc::now(),
            target_block: Some(0),
            request: ProofRequest {
                id: order_id,
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
                    minPrice: U256::from(min_price),
                    maxPrice: U256::from(max_price),
                    biddingStart: 4,
                    rampUpPeriod: 1,
                    timeout: 100,
                    lockTimeout: 100,
                    lockStake: U256::from(10),
                },
            },
            image_id: Some(image_id),
            input_id: Some(input_id),
            proof_id: Some(proof_id.clone()),
            expire_block: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        };
        let order_id = U256::from(order_id);
        db.add_order(order_id, order.clone()).await.unwrap();

        proving_service.find_and_monitor_proofs().await.unwrap();

        // Sleep long enough for the tokio tasks to pickup and complete the order in the DB
        for _ in 0..4 {
            let db_order = db.get_order(order_id).await.unwrap().unwrap();
            if db_order.status != OrderStatus::Proving {
                break;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        let order = db.get_order(order_id).await.unwrap().unwrap();
        assert_eq!(order.status, OrderStatus::PendingAgg);
        assert_eq!(order.proof_id, Some(proof_id));

        assert!(logs_contain("Found 1 proofs currently proving"));
    }
}
