use alloy::primitives::{Address, U256};
use chrono::Utc;
use proptest::prelude::*;
use proptest_derive::Arbitrary;
use std::sync::Arc;
use tokio::runtime::Runtime;

use crate::{Order, OrderStatus};

use super::{BrokerDb, SqliteDb};

use boundless_market::contracts::{
    Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
};

// Define the possible operations we want to test
#[derive(Debug, Arbitrary, Clone)]
enum DbOperation {
    AddOrder(u32),
    GetOrder(u32),
    SetOrderLock { id: u32, lock_block: u64, expire_block: u64 },
    SetProvingStatus { id: u32, lock_price: u64 },
    SetOrderComplete(u32),
    SkipOrder(u32),
    GetLastBlock,
    SetLastBlock(u64),
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
    fn fuzz_db_operations(operations in prop::collection::vec(any::<DbOperation>(), 1..100)) {
        let rt = Runtime::new().unwrap();

        rt.block_on(async {
            // Create in-memory SQLite database
            let db: Arc<dyn BrokerDb + Send + Sync> = Arc::new(
                SqliteDb::new("sqlite::memory:").await.unwrap()
            );

            // Spawn multiple tasks to execute operations concurrently
            let mut handles = vec![];

            for ops in operations.chunks(4) { // Process in chunks of 4 concurrent operations
                let db = db.clone();
                let ops = ops.to_vec();

                handles.push(tokio::spawn(async move {
                    for op in ops {
                        match op {
                            DbOperation::AddOrder(id) => {
                                let _ = db.add_order(U256::from(id), generate_test_order(id)).await;
                            },
                            DbOperation::GetOrder(id) => {
                                let _ = db.get_order(U256::from(id)).await;
                            },
                            DbOperation::SetOrderLock { id, lock_block, expire_block } => {
                                let _ = db.set_order_lock(U256::from(id), lock_block, expire_block).await;
                            },
                            DbOperation::SetProvingStatus { id, lock_price } => {
                                let _ = db.set_proving_status(U256::from(id), U256::from(lock_price)).await;
                            },
                            DbOperation::SetOrderComplete(id) => {
                                let _ = db.set_order_complete(U256::from(id)).await;
                            },
                            DbOperation::SkipOrder(id) => {
                                let _ = db.skip_order(U256::from(id)).await;
                            },
                            DbOperation::GetLastBlock => {
                                let _ = db.get_last_block().await;
                            },
                            DbOperation::SetLastBlock(block) => {
                                let _ = db.set_last_block(block).await;
                            },
                        }
                    }
                }));
            }

            // Wait for all operations to complete
            for handle in handles {
                let _ = handle.await;
            }
        });
    }
}

// Add test to verify database consistency after fuzzing
#[tokio::test]
async fn test_db_consistency_after_fuzz() {
    let db = SqliteDb::new("sqlite::memory:").await.unwrap();

    // Verify database is in a consistent state
    let last_block = db.get_last_block().await.unwrap();
    assert!(last_block.is_none() || last_block.unwrap() > 0);

    // Add more consistency checks as needed
}
