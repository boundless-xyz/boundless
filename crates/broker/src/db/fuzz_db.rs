// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::{Address, U256};
use chrono::Utc;
use elsa::sync::FrozenVec;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use rand::Rng;
use risc0_aggregation::GuestState;
use risc0_zkvm::sha::Digest;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::runtime::Builder;

use crate::{db::AggregationOrder, AggregationState, Order, OrderStatus};

use super::{BrokerDb, SqliteDb};

use boundless_market::contracts::{
    Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
};

// Add new state tracking structure
struct TestState {
    added_orders: Arc<FrozenVec<Box<u32>>>,
    completed_batch: Arc<AtomicBool>,
}

// Define the possible operations we want to test
#[derive(Debug, Arbitrary, Clone)]
enum DbOperation {
    AddOrder(u32),
    OperateOnExistingOrder(ExistingOrderOperation),
    BatchOperation(BatchOperation),
    GetOrderForPricing,
    GetActivePricingOrders,
    GetPendingLockOrders(u32),
    GetProvingOrder,
    GetActiveProofs,
    GetLastBlock,
    SetLastBlock(u32),
    GetAggregationProofs,
    GetBatch(u32),
}

#[derive(Debug, Arbitrary, Clone)]
enum ExistingOrderOperation {
    GetOrder,
    SetOrderLock { lock_timestamp: u32, expire_timestamp: u32 },
    SetProvingStatus { lock_price: u64 },
    SetOrderComplete,
    SkipOrder,
    SetOrderFailure { failure_str: String },
    SetOrderProofId { proof_id: String },
    SetImageInputIds { image_id: String, input_id: String },
    SetAggregationStatus,
    GetSubmissionOrder,
    OrderExists,
}

#[derive(Debug, Arbitrary, Clone)]
enum BatchOperation {
    GetCurrentBatch,
    CompleteBatch {
        g16_proof_id: String,
    },
    GetCompleteBatch,
    SetBatchSubmitted,
    SetBatchFailure {
        error: String,
    },
    UpdateBatch {
        proof_id: String,
        order_count: u8, // Will use this to select N random orders for the batch
    },
}

// Generate a valid Order for testing
fn generate_test_order(id: u32) -> Order {
    Order {
        status: OrderStatus::New,
        updated_at: Utc::now(),
        target_timestamp: None,
        request: ProofRequest::new(
            id,
            &Address::ZERO,
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "test",
            Input { inputType: InputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                biddingStart: 0,
                timeout: 1000,
                lockTimeout: 1000,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
            },
        ),
        image_id: None,
        input_id: None,
        proof_id: Some(format!("proof_{}", id)),
        compressed_proof_id: Some(format!("compressed_proof_{}", id)),
        expire_timestamp: Some(1000),
        lock_expire_timestamp: Some(1000),
        client_sig: vec![].into(),
        lock_price: Some(U256::from(10)),
        error_msg: None,
    }
}

// Main fuzz test function
proptest! {
    #[test]
    fn fuzz_db_operations(operations in prop::collection::vec(any::<DbOperation>(), 1..1000)) {
        // Create a multi-threaded runtime with 4 worker threads
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_time()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Create temporary file for SQLite database
            let temp_db = NamedTempFile::new().unwrap();
            // SQLite URL requires 3 forward slashes after sqlite:
            let db_path = format!("sqlite://{}", temp_db.path().display());

            // Create and initialize the database
            let opts = SqliteConnectOptions::from_str(&db_path).unwrap()
                .create_if_missing(true)
                .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);

            let pool = SqlitePool::connect_with(opts).await.unwrap();

            // Initialize with VACUUM using sqlx
            // TODO double check if necessary with `create_if_missing`, was previously
            sqlx::query("VACUUM").execute(&pool).await.unwrap();
            drop(pool);

            // Create file-based SQLite database
            let db: Arc<dyn BrokerDb + Send + Sync> = Arc::new(
                SqliteDb::new(&db_path).await.unwrap()
            );

            // Create state tracking structure
            let state = TestState {
                added_orders: Arc::new(FrozenVec::new()),
                completed_batch: Arc::new(AtomicBool::new(false)),
            };

            // Spawn multiple tasks to execute operations concurrently
            let mut handles = vec![];

            for ops in operations.chunks(12) {
                let db = db.clone();
                let ops = ops.to_vec();
                let state = TestState {
                    added_orders: state.added_orders.clone(),
                    completed_batch: state.completed_batch.clone(),
                };

                handles.push(tokio::spawn(async move {
                    for op in ops {
                        match op {
                            DbOperation::AddOrder(id) => {
                                db.add_order(U256::from(id), generate_test_order(id)).await.unwrap();
                                state.added_orders.push(Box::new(id));
                            },
                            DbOperation::OperateOnExistingOrder(operation) => {
                                // Skip if no orders have been added yet
                                if state.added_orders.len() == 0 {
                                    continue;
                                }

                                // Randomly select an existing order by index
                                let len = state.added_orders.len();
                                let random_index: usize = rand::rng().random_range(0..len);
                                let id = *state.added_orders.get(random_index).unwrap();

                                match operation {
                                    ExistingOrderOperation::GetOrder => {
                                        db.get_order(U256::from(id)).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderLock { lock_timestamp, expire_timestamp } => {
                                        db.set_order_lock(U256::from(id), lock_timestamp as u64, expire_timestamp as u64).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetProvingStatus { lock_price } => {
                                        db.set_proving_status(U256::from(id), U256::from(lock_price)).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderComplete => {
                                        db.set_order_complete(U256::from(id)).await.unwrap();
                                    },
                                    ExistingOrderOperation::SkipOrder => {
                                        db.skip_order(U256::from(id)).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderFailure { failure_str } => {
                                        db.set_order_failure(U256::from(id), failure_str).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderProofId { proof_id } => {
                                        db.set_order_proof_id(U256::from(id), &proof_id).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetImageInputIds { image_id, input_id } => {
                                        db.set_image_input_ids(U256::from(id), &image_id, &input_id).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetAggregationStatus => {
                                        db.set_aggregation_status(U256::from(id), OrderStatus::PendingAgg).await.unwrap();
                                    },
                                    ExistingOrderOperation::GetSubmissionOrder => {
                                        let order = db.get_order(U256::from(id)).await.unwrap();
                                        if let Some(order) = order {
                                            if order.proof_id.is_some() && order.lock_price.is_some() {
                                                db.get_submission_order(U256::from(id)).await.unwrap();
                                            }
                                        }
                                    },
                                    ExistingOrderOperation::OrderExists => {
                                        db.order_exists(U256::from(id)).await.unwrap();
                                    },
                                }
                            },
                            DbOperation::BatchOperation(operation) => {
                                match operation {
                                    BatchOperation::GetCurrentBatch => {
                                        db.get_current_batch().await.unwrap();
                                    },
                                    BatchOperation::CompleteBatch { g16_proof_id } => {
                                        let batch_id = db.get_current_batch().await.unwrap();
                                        let batch = db.get_batch(batch_id).await.unwrap();
                                        if batch.aggregation_state.is_some() {
                                            db.complete_batch(batch_id, g16_proof_id).await.unwrap();
                                            state.completed_batch.store(true, Ordering::SeqCst);
                                        }
                                    },
                                    BatchOperation::GetCompleteBatch => {
                                        db.get_complete_batch().await.unwrap();
                                    },
                                    BatchOperation::SetBatchSubmitted => {
                                        if state.completed_batch.load(Ordering::SeqCst) {
                                            let batch_id = db.get_current_batch().await.unwrap();
                                            db.set_batch_submitted(batch_id).await.unwrap();
                                        }
                                    },
                                    BatchOperation::SetBatchFailure { error } => {
                                        if state.completed_batch.load(Ordering::SeqCst) {
                                            let batch_id = db.get_current_batch().await.unwrap();
                                            db.set_batch_failure(batch_id, error).await.unwrap();
                                        }
                                    },
                                    BatchOperation::UpdateBatch { proof_id, order_count } => {
                                        if state.added_orders.len() > 0 {
                                            let batch_id = db.get_current_batch().await.unwrap();
                                            // Select up to order_count random orders
                                            let count = std::cmp::min(order_count as usize, state.added_orders.len());
                                            let mut orders = Vec::with_capacity(count);

                                            for _ in 0..count {
                                                let len = state.added_orders.len();
                                                let random_index: usize = rand::rng().random_range(0..len);
                                                let id = *state.added_orders.get(random_index).unwrap();

                                                orders.push(AggregationOrder {
                                                    order_id: U256::from(id),
                                                    proof_id: format!("proof_{}", id),
                                                    expiration: 1000,
                                                    fee: U256::from(10),
                                                });
                                            }

                                            let agg_state = AggregationState {
                                                guest_state: GuestState::initial([1u32; 8]),
                                                claim_digests: vec![],
                                                groth16_proof_id: None,
                                                proof_id,
                                            };

                                            db.update_batch(
                                                batch_id,
                                                &agg_state,
                                                &orders,
                                                Some("proof_id".to_string()),
                                            ).await.unwrap();
                                        }
                                    },
                                }
                            },
                            DbOperation::GetOrderForPricing => {
                                db.update_orders_for_pricing(1).await.unwrap();
                            },
                            DbOperation::GetActivePricingOrders => {
                                db.get_active_pricing_orders().await.unwrap();
                            },
                            DbOperation::GetPendingLockOrders(end_timestamp) => {
                                db.get_pending_lock_orders(end_timestamp as u64).await.unwrap();
                            },
                            DbOperation::GetProvingOrder => {
                                db.get_proving_order().await.unwrap();
                            },
                            DbOperation::GetActiveProofs => {
                                db.get_active_proofs().await.unwrap();
                            },
                            DbOperation::GetLastBlock => {
                                db.get_last_block().await.unwrap();
                            },
                            DbOperation::SetLastBlock(block) => {
                                db.set_last_block(block as u64).await.unwrap();
                            },
                            DbOperation::GetAggregationProofs => {
                                db.get_aggregation_proofs().await.unwrap();
                            },
                            DbOperation::GetBatch(batch_id) => {
                                let current_batch = db.get_current_batch().await.unwrap();
                                let _ = db.get_batch(batch_id as usize % current_batch).await;
                            },
                        }
                    }
                }));
            }

            // Wait for all operations to complete
            for handle in handles {
                handle.await.unwrap();
            }
        });
    }
}
