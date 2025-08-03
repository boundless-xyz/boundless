// Copyright 2025 RISC Zero, Inc.
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

use crate::FulfillmentType;
use crate::{db::AggregationOrder, AggregationState, Order, OrderStatus};

use super::{BrokerDb, SqliteDb};

use boundless_market::contracts::{
    Offer, Predicate, PredicateType, ProofRequest, RequestId, RequestInput, RequestInputType,
    Requirements,
};

// Add new state tracking structure
struct TestState {
    added_orders: Arc<FrozenVec<String>>,
    completed_batch: Arc<AtomicBool>,
}

// Define the possible operations we want to test
#[derive(Debug, Arbitrary, Clone)]
enum DbOperation {
    AddOrder(u32),
    OperateOnExistingOrder(ExistingOrderOperation),
    BatchOperation(BatchOperation),
    GetProvingOrder,
    GetActiveProofs,
    GetAggregationProofs,
    GetBatch(u32),
}

#[derive(Debug, Arbitrary, Clone)]
enum ExistingOrderOperation {
    GetOrder,
    SetOrderComplete,
    SetOrderFailure,
    SetOrderProofId { proof_id: String },
    SetAggregationStatus,
    GetSubmissionOrder,
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
fn generate_test_order(request_id: u32) -> Order {
    Order {
        status: OrderStatus::PendingProving,
        updated_at: Utc::now(),
        target_timestamp: None,
        request: ProofRequest::new(
            RequestId::new(Address::ZERO, request_id),
            Requirements::new(
                Digest::ZERO,
                Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
            ),
            "test",
            RequestInput { inputType: RequestInputType::Url, data: Default::default() },
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
        proof_id: Some(format!("proof_{request_id}")),
        compressed_proof_id: Some(format!("compressed_proof_{request_id}")),
        expire_timestamp: Some(1000),
        client_sig: vec![].into(),
        lock_price: Some(U256::from(10)),
        fulfillment_type: FulfillmentType::LockAndFulfill,
        error_msg: None,
        boundless_market_address: Address::ZERO,
        chain_id: 1,
        total_cycles: None,
        proving_started_at: None,
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
                            DbOperation::AddOrder(request_id) => {
                                let order = generate_test_order(request_id);
                                let id = order.id();
                                db.add_order(&order).await.unwrap();
                                state.added_orders.push(id);
                            },
                            DbOperation::OperateOnExistingOrder(operation) => {
                                // Skip if no orders have been added yet
                                if state.added_orders.is_empty() {
                                    continue;
                                }

                                // Randomly select an existing order by index
                                let len = state.added_orders.len();
                                let random_index: usize = rand::rng().random_range(0..len);
                                let id = state.added_orders.get(random_index).unwrap();

                                match operation {
                                    ExistingOrderOperation::GetOrder => {
                                        db.get_order(id).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderComplete => {
                                        db.set_order_complete(id).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderFailure => {
                                        db.set_order_failure(id, "test").await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderProofId { proof_id } => {
                                        db.set_order_proof_id(id, &proof_id).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetAggregationStatus => {
                                        db.set_aggregation_status(id, OrderStatus::PendingAgg).await.unwrap();
                                    },
                                    ExistingOrderOperation::GetSubmissionOrder => {
                                        let order = db.get_order(id).await.unwrap();
                                        if let Some(order) = order {
                                            if order.proof_id.is_some() && order.lock_price.is_some() {
                                                db.get_submission_order(id).await.unwrap();
                                            }
                                        }
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
                                            db.complete_batch(batch_id, &g16_proof_id).await.unwrap();
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
                                        if !state.added_orders.is_empty() {
                                            let batch_id = db.get_current_batch().await.unwrap();
                                            // Select up to order_count random orders
                                            let count = std::cmp::min(order_count as usize, state.added_orders.len());
                                            let mut orders = Vec::with_capacity(count);

                                            for _ in 0..count {
                                                let len = state.added_orders.len();
                                                let random_index: usize = rand::rng().random_range(0..len);
                                                let id = state.added_orders.get(random_index).unwrap();

                                                orders.push(AggregationOrder {
                                                    order_id: id.to_string(),
                                                    proof_id: format!("proof_{id}"),
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

                            DbOperation::GetProvingOrder => {
                                db.get_proving_order().await.unwrap();
                            },
                            DbOperation::GetActiveProofs => {
                                db.get_active_proofs().await.unwrap();
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
