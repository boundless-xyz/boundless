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

use std::{process::Command, sync::Arc, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, U256},
    providers::Provider,
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use boundless_cli::{OrderFulfilled, OrderFulfiller};
use boundless_market::contracts::{
    boundless_market::{FulfillmentTx, UnlockedRequest},
    Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
};
use boundless_slasher::db::PgDb;
use boundless_test_utils::guests::{ECHO_ID, ECHO_PATH};
use boundless_test_utils::market::create_test_ctx;
use broker::provers::{DefaultProver as BrokerDefaultProver, Prover};
use futures_util::StreamExt;

/// Extract the database connection string from a sqlx::test PgPool.
/// sqlx::test creates an isolated database per test with a unique name.
/// We get the base DATABASE_URL and replace the database name with the test database name.
async fn get_db_url_from_pool(pool: &sqlx::PgPool) -> String {
    // Get the base DATABASE_URL that sqlx::test uses
    let base_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");

    // Query the test database name (sqlx::test creates unique names)
    let db_name: String =
        sqlx::query_scalar("SELECT current_database()").fetch_one(pool).await.unwrap();

    // Replace the database name in the URL (everything after the last '/')
    if let Some(last_slash) = base_url.rfind('/') {
        format!("{}/{}", &base_url[..last_slash + 1], db_name)
    } else {
        // Fallback: append database name if no slash found
        format!("{}/{}", base_url, db_name)
    }
}

/// Wait for slasher to process a locked event (i.e., wait for order to appear in database).
/// Similar to `wait_for_indexer` in indexer tests, this polls the database to ensure
/// the slasher has processed the locked event before proceeding.
async fn wait_for_slasher_to_process_locked(pool: &sqlx::PgPool, request_id: U256) {
    use std::time::{Duration, Instant};

    let timeout = Duration::from_secs(10);
    let poll_interval = Duration::from_millis(200);
    let start = Instant::now();

    loop {
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM orders WHERE id = $1)")
            .bind(format!("{request_id:x}"))
            .fetch_one(pool)
            .await
            .unwrap();

        if exists {
            tracing::info!("Slasher has processed locked event for request 0x{:x}", request_id);
            return;
        }

        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for slasher to process locked event for request 0x{:x}",
                request_id
            );
        }

        tokio::time::sleep(poll_interval).await;
    }
}

async fn create_order(
    signer: &impl Signer,
    signer_addr: Address,
    order_id: u32,
    contract_addr: Address,
    chain_id: u64,
    now: u64,
) -> (ProofRequest, Bytes) {
    let req = ProofRequest::new(
        RequestId::new(signer_addr, order_id),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1),
            rampUpStart: now - 3,
            timeout: 15,
            rampUpPeriod: 1,
            lockTimeout: 10,
            lockCollateral: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

#[sqlx::test(migrations = "./migrations")]
async fn test_basic_usage(pool: sqlx::PgPool) {
    // Run migrations on the isolated test database
    PgDb::from_pool(pool.clone()).await.unwrap();

    // Get connection string for spawned binary
    let db_url = get_db_url_from_pool(&pool).await;

    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-slasher");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--private-key",
        &hex::encode(ctx.customer_signer.clone().to_bytes()),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &db_url,
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Subscribe to slash events before operations
    let slash_event = ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
    let mut stream = slash_event.into_stream();
    println!("Subscribed to ProverSlashed event");

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    let (request, client_sig) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    // Do the operations that should trigger the slash
    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.prover_market.lock_request(&request, client_sig.clone()).await.unwrap();

    // Wait for slasher to process the locked event before expecting slash
    wait_for_slasher_to_process_locked(&pool, request.id).await;

    // Wait for the slash event with timeout
    tokio::select! {
        Some(event) = stream.next() => {
            let request_slashed = event.unwrap().0;
            println!("Detected prover slashed for request {:?}", request_slashed.requestId);
            // Check that the stake recipient is the market treasury address
            assert_eq!(request_slashed.collateralRecipient, ctx.deployment.boundless_market_address);
            cli_process.kill().unwrap();
        }
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            panic!("Test timed out waiting for slash event");
        }
    }
}

#[sqlx::test(migrations = "./migrations")]
#[ignore = "Generate proofs. Slow without dev mode"]
async fn test_slash_fulfilled(pool: sqlx::PgPool) {
    // Run migrations on the isolated test database
    PgDb::from_pool(pool.clone()).await.unwrap();

    // Get connection string for spawned binary
    let db_url = get_db_url_from_pool(&pool).await;

    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-slasher");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--private-key",
        &hex::encode(ctx.customer_signer.clone().to_bytes()),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &db_url,
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Subscribe to slash events before operations
    let slash_event = ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
    let mut stream = slash_event.into_stream();
    println!("Subscribed to ProverSlashed event");

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    let (request, client_sig) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    // Do the operations that should trigger the slash
    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.prover_market.lock_request(&request, client_sig.clone()).await.unwrap();

    // Wait for slasher to process the locked event before fulfilling
    wait_for_slasher_to_process_locked(&pool, request.id).await;

    let client =
        boundless_market::Client::new(ctx.customer_market.clone(), ctx.set_verifier.clone());
    let prover: Arc<dyn Prover + Send + Sync> = Arc::new(BrokerDefaultProver::default());
    let prover = OrderFulfiller::initialize(prover, &client).await.unwrap();
    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(request.clone(), client_sig.clone())]).await.unwrap();
    let order_fulfilled = OrderFulfilled::new(fill, root_receipt, assessor_receipt).unwrap();
    let expires_at = request.offer.rampUpStart + request.offer.timeout as u64;
    let lock_expires_at = request.offer.rampUpStart + request.offer.lockTimeout as u64;

    // Wait for the lock to expire
    loop {
        let ts = ctx
            .customer_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        if ts > lock_expires_at {
            break;
        }
        println!("Waiting for lock to expire...{ts} < {lock_expires_at} - Expires at {expires_at}");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Fulfill the order
    ctx.customer_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                )
                .with_unlocked_request(UnlockedRequest::new(request, client_sig)),
        )
        .await
        .unwrap();

    // Wait for the slash event with timeout
    tokio::select! {
        Some(event) = stream.next() => {
            let request_slashed = event.unwrap().0;
            println!("Detected prover slashed for request {:?}", request_slashed.requestId);
            // Check that the stake recipient is the market treasury address
            assert_eq!(request_slashed.collateralRecipient, ctx.customer_signer.address());
            cli_process.kill().unwrap();
        }
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            panic!("Test timed out waiting for slash event");
        }
    }
}
