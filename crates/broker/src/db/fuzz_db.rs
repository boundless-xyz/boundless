use alloy::primitives::{Address, U256};
use chrono::Utc;
use elsa::sync::FrozenVec;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use rand::Rng;
use sqlx::{sqlite::SqliteConnectOptions, SqlitePool};
use std::str::FromStr;
use std::sync::Arc;
use tempfile::NamedTempFile;
use tokio::runtime::Builder;

use crate::{Order, OrderStatus};

use super::{BrokerDb, SqliteDb};

use boundless_market::contracts::{
    Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
};

// Define the possible operations we want to test
#[derive(Debug, Arbitrary, Clone)]
enum DbOperation {
    AddOrder(u32),
    OperateOnExistingOrder(ExistingOrderOperation),
}

#[derive(Debug, Arbitrary, Clone)]
enum ExistingOrderOperation {
    GetOrder,
    SetOrderLock { lock_block: u32, expire_block: u32 },
    SetProvingStatus { lock_price: u64 },
    SetOrderComplete,
    SkipOrder,
}

// Generate a valid Order for testing
fn generate_test_order(id: u32) -> Order {
    Order {
        status: OrderStatus::New,
        updated_at: Utc::now(),
        target_block: None,
        request: ProofRequest::new(
            id,
            &Address::ZERO,
            Requirements {
                imageId: Default::default(),
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            "test",
            Input { inputType: InputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
            },
        ),
        image_id: None,
        input_id: None,
        proof_id: None,
        expire_block: None,
        client_sig: vec![].into(),
        lock_price: None,
        error_msg: None,
    }
}

// Main fuzz test function
proptest! {
    #[test]
    fn fuzz_db_operations(operations in prop::collection::vec(any::<DbOperation>(), 1..10000)) {
        // Create a multi-threaded runtime with 4 worker threads
        let rt = Builder::new_multi_thread()
            .worker_threads(4)
            .enable_time()
            .enable_all()
            .build()
            .unwrap();

        println!("Operations: {}", operations.len());

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
            sqlx::query("VACUUM").execute(&pool).await.unwrap();
            drop(pool);

            // Create file-based SQLite database
            let db: Arc<dyn BrokerDb + Send + Sync> = Arc::new(
                SqliteDb::new(&db_path).await.unwrap()
            );

            // Keep track of added order IDs
            let added_orders = Arc::new(FrozenVec::new());

            // Spawn multiple tasks to execute operations concurrently
            let mut handles = vec![];

            for ops in operations.chunks(12) {
                let db = db.clone();
                let ops = ops.to_vec();
                let added_orders = added_orders.clone();

                handles.push(tokio::spawn(async move {
                    for op in ops {
                        match op {
                            DbOperation::AddOrder(id) => {
                                db.add_order(U256::from(id), generate_test_order(id)).await.unwrap();
                                added_orders.push(Box::new(id));
                            },
                            DbOperation::OperateOnExistingOrder(operation) => {
                                // Skip if no orders have been added yet
                                if added_orders.len() == 0 {
                                    continue;
                                }

                                // Randomly select an existing order by index
                                let len = added_orders.len();
                                let random_index: usize = rand::rng().random_range(0..len);
                                let id = *added_orders.get(random_index).unwrap();

                                match operation {
                                    ExistingOrderOperation::GetOrder => {
                                        db.get_order(U256::from(id)).await.unwrap();
                                    },
                                    ExistingOrderOperation::SetOrderLock { lock_block, expire_block } => {
                                        db.set_order_lock(U256::from(id), lock_block as u64, expire_block as u64).await.unwrap();
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
                                }
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
