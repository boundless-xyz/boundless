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

use std::{process::Command, sync::Arc, time::{Duration, SystemTime}};

use assert_cmd::Command as AssertCommand;

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, U256},
    providers::{ext::AnvilApi, Provider},
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use boundless_cli::{DefaultProver, OrderFulfilled};
use boundless_indexer::{
    db::market::{RequestStatusType, SlashedStatus},
    test_utils::TestDb,
};
use boundless_market::contracts::{
    boundless_market::FulfillmentTx, Offer, Predicate, ProofRequest, RequestId, RequestInput,
    Requirements,
};
use boundless_test_utils::{
    guests::{ASSESSOR_GUEST_ELF, ECHO_ID, ECHO_PATH, SET_BUILDER_ELF},
    market::create_test_ctx,
};
use sqlx::{AnyPool, Row};
use tracing_test::traced_test;

// Helper struct for hourly summary data
#[derive(Debug)]
struct HourlySummaryRow {
    period_timestamp: u64,
    total_fulfilled: u64,
    unique_provers_locking_requests: u64,
    unique_requesters_submitting_requests: u64,
    total_fees_locked: String,
    total_collateral_locked: String,
    total_requests_submitted: u64,
    total_requests_submitted_onchain: u64,
    total_requests_submitted_offchain: u64,
    total_requests_locked: u64,
    total_requests_slashed: u64,
    total_expired: u64,
    total_locked_and_expired: u64,
    total_locked_and_fulfilled: u64,
    locked_orders_fulfillment_rate: f32,
}

async fn count_hourly_summaries(pool: &AnyPool) -> i64 {
    let result = sqlx::query("SELECT COUNT(*) as count FROM hourly_market_summary")
        .fetch_one(pool)
        .await
        .unwrap();
    result.get("count")
}

async fn get_all_hourly_summaries_asc(pool: &AnyPool) -> Vec<HourlySummaryRow> {
    let rows = sqlx::query(
        "SELECT period_timestamp, total_fulfilled, unique_provers_locking_requests,
                unique_requesters_submitting_requests, total_fees_locked, total_collateral_locked,
                total_requests_submitted, total_requests_submitted_onchain,
                total_requests_submitted_offchain, total_requests_locked, total_requests_slashed,
                total_expired, total_locked_and_expired, total_locked_and_fulfilled,
                locked_orders_fulfillment_rate
         FROM hourly_market_summary ORDER BY period_timestamp ASC",
    )
    .fetch_all(pool)
    .await
    .unwrap();

    rows.into_iter()
        .map(|row| HourlySummaryRow {
            period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
            total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
            unique_provers_locking_requests: row.get::<i64, _>("unique_provers_locking_requests") as u64,
            unique_requesters_submitting_requests: row.get::<i64, _>("unique_requesters_submitting_requests") as u64,
            total_fees_locked: row.get("total_fees_locked"),
            total_collateral_locked: row.get("total_collateral_locked"),
            total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
            total_requests_submitted_onchain: row.get::<i64, _>("total_requests_submitted_onchain") as u64,
            total_requests_submitted_offchain: row.get::<i64, _>("total_requests_submitted_offchain") as u64,
            total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
            total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
            total_expired: row.get::<i64, _>("total_expired") as u64,
            total_locked_and_expired: row.get::<i64, _>("total_locked_and_expired") as u64,
            total_locked_and_fulfilled: row.get::<i64, _>("total_locked_and_fulfilled") as u64,
            locked_orders_fulfillment_rate: row.get::<f64, _>("locked_orders_fulfillment_rate") as f32,
        })
        .collect()
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
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockCollateral: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

async fn create_order_with_timeouts(
    signer: &impl Signer,
    signer_addr: Address,
    order_id: u32,
    contract_addr: Address,
    chain_id: u64,
    now: u64,
    lock_timeout: u32,
    timeout: u32,
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
            timeout,
            rampUpPeriod: 1,
            lockTimeout: lock_timeout,
            lockCollateral: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_e2e() {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Use assert_cmd to find the binary path
    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "0",
        "--start-block",
        "0",
    ];

    println!("{exe_path} {args:?}");

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let header = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header;
    
    let now = header.timestamp;
    let block_number = header.number;
    tracing::info!("Before submitting order: now: {}, block_number: {}", now, block_number);

    let (request, client_sig) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    ctx.prover_market.lock_request(&request, client_sig.clone(), None).await.unwrap();

    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(request.clone(), client_sig.clone())]).await.unwrap();
    let order_fulfilled =
        OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await
        .unwrap();

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let header2 = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header;
    
    let now2 = header2.timestamp;
    let block_number2 = header2.number;
    tracing::info!("After fulfilling order: now: {}, block_number: {}", now2, block_number2);

    // Wait for the events to be indexed
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check that the request was indexed
    let result = sqlx::query("SELECT * FROM proof_requests WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // check that the requestSubmitted event was indexed
    let result = sqlx::query("SELECT * FROM request_submitted_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the request was locked
    let result = sqlx::query("SELECT * FROM request_locked_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the proof was delivered
    let result = sqlx::query("SELECT * FROM proof_delivered_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the fulfillment was indexed
    let result = sqlx::query("SELECT * FROM fulfillments WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the proof was fulfilled
    let result = sqlx::query("SELECT * FROM request_fulfilled_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check hourly aggregation
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(summary_count >= 1, "Expected at least one hourly summary, got {}", summary_count);

    let mut summaries = get_all_hourly_summaries_asc(&test_db.pool).await;
    // Sort summaries descending by period_timestamp
    summaries.sort_by(|a, b| b.period_timestamp.cmp(&a.period_timestamp));
    let summary = &summaries[0];

    // Verify hour boundary alignment (timestamp should be divisible by 3600)
    assert_eq!(
        summary.period_timestamp % 3600,
        0,
        "Hour timestamp should be aligned to hour boundary"
    );

    // Verify counts match our test scenario
    assert_eq!(summary.total_fulfilled, 1, "Expected 1 fulfilled request");
    assert_eq!(
        summary.unique_provers_locking_requests, 1,
        "Expected 1 unique prover"
    );
    assert_eq!(
        summary.unique_requesters_submitting_requests, 1,
        "Expected 1 unique requester"
    );

    // Verify new request count fields
    assert_eq!(summary.total_requests_submitted, 1, "Expected 1 total request submitted");
    assert_eq!(summary.total_requests_submitted_onchain, 1, "Expected 1 onchain request (has requestSubmitted event)");
    assert_eq!(summary.total_requests_submitted_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(summary.total_requests_locked, 1, "Expected 1 locked request");
    assert_eq!(summary.total_requests_slashed, 0, "Expected 0 slashed requests");

    // Verify fees and collateral are non-zero (the offer had minPrice=0, maxPrice=1, collateral=0)
    // But we should at least have the strings present
    assert!(!summary.total_fees_locked.is_empty(), "total_fees_locked should not be empty");
    assert!(!summary.total_collateral_locked.is_empty(), "total_collateral_locked should not be empty");

    // Verify new expiration and fulfillment fields
    assert_eq!(summary.total_expired, 0, "Expected 0 expired requests (request was fulfilled)");
    assert_eq!(summary.total_locked_and_expired, 0, "Expected 0 locked and expired requests");
    assert_eq!(summary.total_locked_and_fulfilled, 1, "Expected 1 locked and fulfilled request");
    assert_eq!(summary.locked_orders_fulfillment_rate, 100.0, "Expected 100% fulfillment rate (1/1)");

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_monitoring() {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Use assert_cmd to find the binary path
    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{exe_path} {args:?}");

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    let monitor = indexer_monitor::monitor::Monitor::new(&test_db.db_url).await.unwrap();

    let mut now = ctx
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

    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    ctx.prover_market.lock_request(&request, client_sig, None).await.unwrap();

    // Fetch requests ids that expired in the last 30 seconds
    // This should be empty since the request is not expired yet
    let expired = monitor
        .fetch_requests_expired((now - Duration::from_secs(30).as_secs()) as i64, now as i64)
        .await
        .unwrap();
    assert_eq!(expired.len(), 0);

    // Wait for the request to expire
    loop {
        now = ctx
            .customer_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        if now > request.expires_at() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // fetch requests ids that expired in the last 30 seconds
    // This should return the request id since it is expired
    let expired = monitor
        .fetch_requests_expired((now - Duration::from_secs(30).as_secs()) as i64, now as i64)
        .await
        .unwrap();
    assert_eq!(expired.len(), 1);
    // fetch requests ids that expired in the last 30 seconds submitted by the customer
    // This should return the request id since it is expired and submitted by the customer
    let expired = monitor
        .fetch_requests_expired_from(
            (now - Duration::from_secs(30).as_secs()) as i64,
            now as i64,
            ctx.customer_signer.address(),
        )
        .await
        .unwrap();
    assert_eq!(expired.len(), 1);

    // slash the request
    ctx.prover_market.slash(request.id).await.unwrap();

    // Send a new request
    now = ctx
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
        2,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    ctx.prover_market.lock_request(&request, client_sig.clone(), None).await.unwrap();
    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(request.clone(), client_sig.clone())]).await.unwrap();
    let order_fulfilled =
        OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    let fulfillment = FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
        .with_submit_root(
            ctx.deployment.set_verifier_address,
            order_fulfilled.root,
            order_fulfilled.seal,
        );
    ctx.prover_market.fulfill(fulfillment).await.unwrap();

    // Wait for the events to be indexed
    tokio::time::sleep(Duration::from_secs(2)).await;

    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Check top level metrics
    let total_requests = monitor.total_requests().await.unwrap();
    assert_eq!(total_requests, 2);

    let requests = monitor.fetch_requests(0, now as i64).await.unwrap();
    assert_eq!(requests.len(), 2);

    let total_requests =
        monitor.total_requests_from_client(ctx.customer_signer.address()).await.unwrap();
    assert_eq!(total_requests, 2);

    let total_fulfillments = monitor.total_fulfillments().await.unwrap();
    assert_eq!(total_fulfillments, 1);

    let total_fulfillments =
        monitor.total_fulfillments_from_client(ctx.customer_signer.address()).await.unwrap();
    assert_eq!(total_fulfillments, 1);

    let total_slashed = monitor.total_slashed().await.unwrap();
    assert_eq!(total_slashed, 1);

    let total_slashed_by_prover =
        monitor.total_slashed_by_prover(ctx.prover_signer.address()).await.unwrap();
    assert_eq!(total_slashed_by_prover, 1);

    let client_success_rate =
        monitor.total_success_rate_from_client(ctx.customer_signer.address()).await.unwrap();
    assert_eq!(client_success_rate, Some(0.5));

    let prover_success_rate =
        monitor.total_success_rate_by_prover(ctx.prover_signer.address()).await.unwrap();
    assert_eq!(prover_success_rate, Some(0.5));

    // Check hourly aggregation
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(summary_count >= 1, "Expected at least one hourly summary, got {}", summary_count);

    let summaries = get_all_hourly_summaries_asc(&test_db.pool).await;

    // Sum up all fulfilled across all hours (should be 1, since one was slashed)
    let total_fulfilled_across_hours: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
    assert_eq!(
        total_fulfilled_across_hours, 1,
        "Expected 1 total fulfilled across all hours (one was slashed)"
    );

    // Verify new request count fields across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 = summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 = summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();
    let total_locked: u64 = summaries.iter().map(|s| s.total_requests_locked).sum();
    let total_slashed: u64 = summaries.iter().map(|s| s.total_requests_slashed).sum();

    assert_eq!(total_requests_submitted, 2, "Expected 2 total requests submitted");
    assert_eq!(total_requests_onchain, 2, "Expected 2 onchain requests (both used submit_request_with_signature)");
    assert_eq!(total_requests_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(total_locked, 2, "Expected 2 locked requests");
    assert_eq!(total_slashed, 1, "Expected 1 slashed request");

    // Verify new expiration and fulfillment fields across all hours
    let total_expired: u64 = summaries.iter().map(|s| s.total_expired).sum();
    let total_locked_and_expired: u64 = summaries.iter().map(|s| s.total_locked_and_expired).sum();
    let total_locked_and_fulfilled: u64 = summaries.iter().map(|s| s.total_locked_and_fulfilled).sum();

    assert_eq!(total_expired, 1, "Expected 1 expired request (the slashed one)");
    assert_eq!(total_locked_and_expired, 1, "Expected 1 locked and expired request");
    assert_eq!(total_locked_and_fulfilled, 1, "Expected 1 locked and fulfilled request");

    // Verify at least one hour has data
    let summary = &summaries[0];
    assert_eq!(
        summary.period_timestamp % 3600,
        0,
        "Hour timestamp should be aligned to hour boundary"
    );

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_across_hours() {
    // Create a persistent database file for debugging
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Create a persistent database file in /tmp for easy debugging
    let db_filename = format!("test_aggregation_debug_{}.db", timestamp);
    let db_path = std::path::Path::new("/tmp").join(&db_filename);

    // Create an empty file to ensure it exists
    std::fs::File::create(&db_path).unwrap();

    let db_url = format!("sqlite:{}", db_path.display());

    // Log the database location for debugging
    tracing::info!("Creating persistent test database at: {}", db_path.display());
    tracing::info!("Database URL: {}", &db_url);
    tracing::info!("To inspect the database after the test, use: sqlite3 {}", db_path.display());

    // Connect to the database
    sqlx::any::install_default_drivers();
    let pool = sqlx::AnyPool::connect(&db_url).await.unwrap();
    let db = Arc::new(boundless_indexer::db::AnyDb::new(&db_url).await.unwrap());

    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Use assert_cmd to find the binary path
    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
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

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Get initial timestamp
    let now = ctx
        .customer_provider
        .clone()
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Create and fulfill first order
    let (request1, client_sig1) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx.customer_market
        .submit_request_with_signature(&request1, client_sig1.clone())
        .await
        .unwrap();
    ctx.prover_market
        .lock_request(&request1, client_sig1.clone(), None)
        .await
        .unwrap();

    let (fill1, root_receipt1, assessor_receipt1) =
        prover.fulfill(&[(request1.clone(), client_sig1.clone())]).await.unwrap();
    let order_fulfilled1 = OrderFulfilled::new(fill1.clone(), root_receipt1, assessor_receipt1).unwrap();
    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled1.fills, order_fulfilled1.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled1.root,
                    order_fulfilled1.seal,
                ),
        )
        .await
        .unwrap();

    // Advance time by more than an hour (3700 seconds)
    let provider = ctx.customer_provider.clone();
    provider.anvil_set_next_block_timestamp(now + 3700).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    // Get new timestamp
    let now2 = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Create and fulfill second order in a different hour
    let (request2, client_sig2) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        2,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now2,
    )
    .await;

    ctx.customer_market
        .submit_request_with_signature(&request2, client_sig2.clone())
        .await
        .unwrap();
    ctx.prover_market
        .lock_request(&request2, client_sig2.clone(), None)
        .await
        .unwrap();

    let (fill2, root_receipt2, assessor_receipt2) =
        prover.fulfill(&[(request2.clone(), client_sig2.clone())]).await.unwrap();
    let order_fulfilled2 = OrderFulfilled::new(fill2.clone(), root_receipt2, assessor_receipt2).unwrap();
    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled2.fills, order_fulfilled2.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled2.root,
                    order_fulfilled2.seal,
                ),
        )
        .await
        .unwrap();

    let now3 = ctx
        .customer_provider.clone()
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    provider.anvil_set_next_block_timestamp(now3 + 3700).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    // Wait for the events to be indexed and aggregated
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Check hourly aggregation across multiple hours
    let summary_count = count_hourly_summaries(&pool).await;
    assert!(
        summary_count >= 2,
        "Expected at least 2 hourly summaries (one per hour), got {}",
        summary_count
    );

    let summaries = get_all_hourly_summaries_asc(&pool).await;

    // Verify we have multiple hours
    assert!(
        summaries.len() >= 2,
        "Expected at least 2 different hours of data"
    );

    // Verify all hour timestamps are aligned to hour boundaries
    for summary in &summaries {
        assert_eq!(
            summary.period_timestamp % 3600,
            0,
            "Period timestamp {} should be aligned to hour boundary",
            summary.period_timestamp
        );
    }

    // Verify hour timestamps are different (events are in different hours)
    let hour1 = summaries[0].period_timestamp;
    let hour2 = summaries[1].period_timestamp;
    assert_ne!(
        hour1, hour2,
        "Expected different hour timestamps for different hours"
    );

    // Verify time difference is at least 1 hour
    let hour_diff = hour2.saturating_sub(hour1);
    assert!(
        hour_diff >= 3600,
        "Expected at least 1 hour difference between timestamps, got {}",
        hour_diff
    );

    // Total fulfilled across all hours should be 2
    let total_fulfilled_across_hours: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
    assert_eq!(
        total_fulfilled_across_hours, 2,
        "Expected 2 total fulfilled across all hours"
    );

    // Verify new request count fields across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 = summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 = summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();
    let total_locked: u64 = summaries.iter().map(|s| s.total_requests_locked).sum();
    let total_slashed: u64 = summaries.iter().map(|s| s.total_requests_slashed).sum();

    assert_eq!(total_requests_submitted, 2, "Expected 2 total requests submitted");
    assert_eq!(total_requests_onchain, 2, "Expected 2 onchain requests (both used submit_request_with_signature)");
    assert_eq!(total_requests_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(total_locked, 2, "Expected 2 locked requests");
    assert_eq!(total_slashed, 0, "Expected 0 slashed requests");

    //debug by printing proof_request table
    let request_statuses = sqlx::query("SELECT * FROM request_status")
        .fetch_all(&pool)
        .await
        .unwrap();
    for request_status_row in request_statuses {
        let request_status = db.row_to_request_status(&request_status_row).unwrap();
        tracing::info!(
            "Request status: request_digest: {}, request_id: {}, request_status: {}, source: {}, client_address: {}, locked_at: {:?}, lock_block: {:?}, lock_tx_hash: {:?}, lock_prover_address: {:?}, fulfilled_at: {:?}, fulfill_block: {:?}, fulfill_tx_hash: {:?}, fulfill_prover_address: {:?}",
            request_status.request_digest,
            request_status.request_id,
            request_status.request_status,
            request_status.source,
            request_status.client_address,
            request_status.locked_at,
            request_status.lock_block,
            request_status.lock_tx_hash,
            request_status.lock_prover_address,
            request_status.fulfilled_at,
            request_status.fulfill_block,
            request_status.fulfill_tx_hash,
            request_status.fulfill_prover_address,
        );
    }

    // Verify new expiration and fulfillment fields across all hours
    let total_expired: u64 = summaries.iter().map(|s| s.total_expired).sum();
    let total_locked_and_expired: u64 = summaries.iter().map(|s| s.total_locked_and_expired).sum();
    let total_locked_and_fulfilled: u64 = summaries.iter().map(|s| s.total_locked_and_fulfilled).sum();

    tracing::info!("Summaries: {:?}", summaries);
    assert_eq!(total_expired, 0, "Expected 0 expired requests (both were fulfilled)");
    assert_eq!(total_locked_and_expired, 0, "Expected 0 locked and expired requests");
    assert_eq!(total_locked_and_fulfilled, 2, "Expected 2 locked and fulfilled requests");

    // Verify hour boundary formula: (timestamp / 3600) * 3600
    let expected_hour1 = (now / 3600) * 3600;
    let expected_hour2 = (now2 / 3600) * 3600;

    // Check that our actual hour timestamps match the expected formula
    assert!(
        summaries.iter().any(|s| s.period_timestamp == expected_hour1),
        "Expected to find period timestamp {} computed from first order timestamp {}",
        expected_hour1,
        now
    );
    assert!(
        summaries.iter().any(|s| s.period_timestamp == expected_hour2),
        "Expected to find period timestamp {} computed from second order timestamp {}",
        expected_hour2,
        now2
    );

    cli_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_indexer_with_order_stream(pool: sqlx::PgPool) {
    use boundless_market::order_stream_client::OrderStreamClient;
    use order_stream::{run_from_parts, AppState, ConfigBuilder};
    use std::net::{Ipv4Addr, SocketAddr};
    use url::Url;

    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Setup order stream server
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).await.unwrap();
    let order_stream_address = listener.local_addr().unwrap();
    let order_stream_url = Url::parse(&format!("http://{order_stream_address}")).unwrap();

    let order_stream_config = ConfigBuilder::default()
        .rpc_url(anvil.endpoint_url())
        .market_address(ctx.deployment.boundless_market_address)
        .domain(order_stream_address.to_string())
        .build()
        .unwrap();

    let order_stream = AppState::new(&order_stream_config, Some(pool)).await.unwrap();
    let order_stream_clone = order_stream.clone();
    let order_stream_handle = tokio::spawn(async move {
        run_from_parts(order_stream_clone, listener).await.unwrap();
    });

    // Create order stream client and submit test orders
    let order_stream_client = OrderStreamClient::new(
        order_stream_url.clone(),
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
    );

    
    let mut now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
    let provider = ctx.customer_provider.clone();
    let block_timestamp = provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp;
    let start_block = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await.unwrap().unwrap().header.number;

    // For some reason initial anvil block timestamps is in the future. Our time must advance past the anvil block timestampbefore we submit our off chain orders, 
    // since the indexer uses block timestmaps to filter the order stream to avoid re-processing orders from the past.
    while now < block_timestamp {
        tokio::time::sleep(Duration::from_secs(1)).await;
        now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs();
        tracing::info!("Now: {}, anvil block timestamp: {}", now, block_timestamp);
    }
    tracing::info!("Block timestamp: {}", block_timestamp);
    tracing::info!("Now: {}", now);


    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Submit 3 orders to order stream with delays
    let (req1, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req1, &ctx.customer_signer).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let (req2, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        2,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req2, &ctx.customer_signer).await.unwrap();

    tokio::time::sleep(Duration::from_millis(500)).await;

    let (req3, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        3,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req3, &ctx.customer_signer).await.unwrap();

    let orders = order_stream_client.list_orders_v2(None, None, None, None, None).await.unwrap();
    assert_eq!(orders.orders.len(), 3, "Expected 3 orders to be indexed");
    tracing::info!("Orders: {:?}", orders);

    let now_2 = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;
    provider.anvil_set_next_block_timestamp(now_2 + 10).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();
    
    // Start market indexer with order stream URL
    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "1",
        "--order-stream-url",
        order_stream_url.as_str(),
        "--start-block",
        &start_block.to_string(),
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Wait for the indexer to process orders
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Verify all 3 orders were indexed
    let result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let count: i64 = result.get("count");
    assert_eq!(count, 3, "Expected 3 proof requests to be indexed");

    // Verify all orders are marked as offchain
    let result =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
    let offchain_count: i64 = result.get("count");
    assert_eq!(offchain_count, 3, "Expected all 3 requests to have source='offchain'");

    // Verify order stream state is tracked
    let result = sqlx::query("SELECT last_processed_timestamp FROM order_stream_state")
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let timestamp: Option<String> = result.get("last_processed_timestamp");
    assert!(timestamp.is_some(), "Expected last_processed_timestamp to be set");

    // Verify tx_hash is zero for offchain orders
    let result = sqlx::query("SELECT tx_hash FROM proof_requests WHERE request_id = $1")
        .bind(format!("{:x}", req1.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let tx_hash: String = result.get("tx_hash");
    assert_eq!(
        tx_hash, "0000000000000000000000000000000000000000000000000000000000000000",
        "Expected tx_hash to be zero for offchain order"
    );

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[tokio::test]
#[traced_test]
async fn test_both_tx_fetch_strategies_produce_same_results() {
    let anvil = Anvil::new().spawn();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Create two separate test databases - one for each strategy
    let test_db_receipts = TestDb::new().await.unwrap();
    let test_db_tx_by_hash = TestDb::new().await.unwrap();

    let rpc_url = anvil.endpoint_url();

    // Get prover for fulfillment
    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    // Get current timestamp
    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Submit and fulfill 1 request to have some transactions to test
    let (request, client_sig) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.customer_market
        .submit_request_with_signature(&request, client_sig.clone())
        .await
        .unwrap();
    ctx.prover_market
        .lock_request(&request, client_sig.clone(), None)
        .await
        .unwrap();

    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(request.clone(), client_sig.clone())]).await.unwrap();
    let order_fulfilled =
        OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await
        .unwrap();

    // Mine a couple more blocks to ensure transactions are visible to indexers
    let provider = ctx.customer_provider.clone();
    provider.anvil_mine(Some(2), None).await.unwrap();

    // Get the current block number to use as end block for both indexers
    let end_block = provider.get_block_number().await.unwrap();
    let end_block_str = end_block.to_string();

    // Run indexer with block-receipts strategy
    let cmd1 = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd1.get_program().to_string_lossy().to_string();
    
    let args_receipts = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db_receipts.db_url,
        "--interval",
        "1",
        "--retries",
        "3",
        "--batch-size",
        "100",
        "--tx-fetch-strategy",
        "block-receipts",
        "--start-block",
        "0",
        "--end-block",
        &end_block_str,
    ];

    println!("Running indexer with block-receipts strategy: {exe_path} {args_receipts:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process_receipts =
        Command::new(&exe_path).args(args_receipts).spawn().unwrap();

    // Wait for the indexer to process events
    tokio::time::sleep(Duration::from_secs(3)).await;
    cli_process_receipts.kill().ok(); // Kill if still running
    
    // Wait a moment for cleanup
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Run indexer with tx-by-hash strategy
    let args_tx_by_hash = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db_tx_by_hash.db_url,
        "--interval",
        "1",
        "--retries",
        "3",
        "--batch-size",
        "100",
        "--tx-fetch-strategy",
        "tx-by-hash",
        "--start-block",
        "0",
        "--end-block",
        &end_block_str,
    ];

    println!("Running indexer with tx-by-hash strategy: {exe_path} {args_tx_by_hash:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process_tx_by_hash =
        Command::new(&exe_path).args(args_tx_by_hash).spawn().unwrap();

    // Wait for the indexer to process events
    tokio::time::sleep(Duration::from_secs(3)).await;
    cli_process_tx_by_hash.kill().ok(); // Kill if still running
    
    // Wait a moment for cleanup
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Now compare the results from both databases
    // 1. Compare transaction count
    let count_receipts: i64 = sqlx::query("SELECT COUNT(*) as count FROM transactions")
        .fetch_one(&test_db_receipts.pool)
        .await
        .unwrap()
        .get("count");

    let count_tx_by_hash: i64 = sqlx::query("SELECT COUNT(*) as count FROM transactions")
        .fetch_one(&test_db_tx_by_hash.pool)
        .await
        .unwrap()
        .get("count");

    assert_eq!(
        count_receipts, count_tx_by_hash,
        "Transaction counts should match between strategies"
    );
    assert!(count_receipts > 0, "Expected at least some transactions to be indexed");

    // 2. Compare transactions table data (tx_hash, from_address, block_number, block_timestamp, transaction_index)
    let txs_receipts: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
    )
    .fetch_all(&test_db_receipts.pool)
    .await
    .unwrap();

    let txs_tx_by_hash: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
    )
    .fetch_all(&test_db_tx_by_hash.pool)
    .await
    .unwrap();

    assert_eq!(
        txs_receipts.len(),
        txs_tx_by_hash.len(),
        "Should have same number of transactions"
    );

    // Compare each transaction
    for (i, (tx_receipts, tx_tx_by_hash)) in
        txs_receipts.iter().zip(txs_tx_by_hash.iter()).enumerate()
    {
        assert_eq!(
            tx_receipts.0, tx_tx_by_hash.0,
            "Transaction {} tx_hash should match",
            i
        );
        assert_eq!(
            tx_receipts.1, tx_tx_by_hash.1,
            "Transaction {} from_address should match",
            i
        );
        assert_eq!(
            tx_receipts.2, tx_tx_by_hash.2,
            "Transaction {} block_number should match",
            i
        );
        assert_eq!(
            tx_receipts.3, tx_tx_by_hash.3,
            "Transaction {} block_timestamp should match",
            i
        );
        assert_eq!(
            tx_receipts.4, tx_tx_by_hash.4,
            "Transaction {} transaction_index should match",
            i
        );
    }

    // 3. Compare proof requests count
    let count_requests_receipts: i64 =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db_receipts.pool)
            .await
            .unwrap()
            .get("count");

    let count_requests_tx_by_hash: i64 =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
            .fetch_one(&test_db_tx_by_hash.pool)
            .await
            .unwrap()
            .get("count");

    assert_eq!(
        count_requests_receipts, count_requests_tx_by_hash,
        "Proof request counts should match between strategies"
    );
    assert_eq!(
        count_requests_receipts, 1,
        "Expected 1 proof request to be indexed"
    );
}

// Helper struct for request status data
#[derive(Debug)]
struct RequestStatusRow {
    request_digest: String,
    request_id: String,
    request_status: String,
    slashed_status: String,
    source: String,
    created_at: i64,
    updated_at: i64,
    locked_at: Option<i64>,
    fulfilled_at: Option<i64>,
    slashed_at: Option<i64>,
    lock_end: i64,
    slash_recipient: Option<String>,
    slash_transferred_amount: Option<String>,
    slash_burned_amount: Option<String>,
}

async fn get_request_status(pool: &AnyPool, request_id: &str) -> RequestStatusRow {
    let row = sqlx::query(
        "SELECT request_digest, request_id, request_status, slashed_status, source, created_at, updated_at,
                locked_at, fulfilled_at, slashed_at, lock_end, slash_recipient,
                slash_transferred_amount, slash_burned_amount
         FROM request_status
         WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .unwrap();

    RequestStatusRow {
        request_digest: row.get("request_digest"),
        request_id: row.get("request_id"),
        request_status: row.get("request_status"),
        slashed_status: row.get("slashed_status"),
        source: row.get("source"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        locked_at: row.get("locked_at"),
        fulfilled_at: row.get("fulfilled_at"),
        slashed_at: row.get("slashed_at"),
        lock_end: row.get("lock_end"),
        slash_recipient: row.get("slash_recipient"),
        slash_transferred_amount: row.get("slash_transferred_amount"),
        slash_burned_amount: row.get("slash_burned_amount"),
    }
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_happy_path(_pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    let block = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();

    let start_block = block.header.number.saturating_sub(1);

    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let start_block_str = start_block.to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "3",
        "--start-block",
        start_block_str.as_str(),
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut indexer_process = Command::new(exe_path).args(args).spawn().unwrap();

    let provider = ctx.customer_provider.clone();

    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let now = block.header.timestamp;

    // Create and submit request
    let (req, sig) = create_order_with_timeouts(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
        1000000,
        1000000,
    )
    .await;

    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx
        .customer_market
        .submit_request_with_signature(&req, sig.clone())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify submitted status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Submitted.to_string(),
        "Request should be in submitted status"
    );
    assert_eq!(status.source, "onchain", "Request should be marked as onchain");
    assert!(status.locked_at.is_none(), "Request should not be locked yet");
    assert!(status.fulfilled_at.is_none(), "Request should not be fulfilled yet");

    // Lock request
    ctx.prover_market.lock_request(&req, sig.clone(), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify locked status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Locked.to_string(),
        "Request should be in locked status"
    );
    assert!(status.locked_at.is_some(), "Request should have locked_at timestamp");
    assert_eq!(
        status.lock_end,
        req.offer.rampUpStart as i64 + req.offer.lockTimeout as i64,
        "lock_end should be rampUpStart + lockTimeout"
    );

    // Fulfill request
    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(req.clone(), sig.clone())]).await.unwrap();
    let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    ctx
        .prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify fulfilled status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Fulfilled.to_string(),
        "Request should be in fulfilled status"
    );
    assert!(status.fulfilled_at.is_some(), "Request should have fulfilled_at timestamp");

    indexer_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_locked_then_expired(_pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let block = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();

    let start_block = block.header.number.saturating_sub(1);

    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let start_block_str = start_block.to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "3",
        "--start-block",
        start_block_str.as_str(),
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut indexer_process = Command::new(exe_path).args(args).spawn().unwrap();

    let provider = ctx.customer_provider.clone();

    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let now = block.header.timestamp;

    // Create and submit request with short timeout
    let (req, sig) = create_order_with_timeouts(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
        1000,
        1000,
    )
    .await;

    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx
        .customer_market
        .submit_request_with_signature(&req, sig.clone())
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify submitted status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Submitted.to_string(),
        "Request should be in submitted status"
    );

    // Lock request
    ctx.prover_market.lock_request(&req, sig.clone(), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify locked status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Locked.to_string(),
        "Request should be in locked status"
    );

    // Advance time past request expiration
    let expires_at = req.expires_at();
    provider.anvil_set_next_block_timestamp(expires_at + 1).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify expired status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Expired.to_string(),
        "Request should be in expired status"
    );

    indexer_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_lock_expired_then_slashed(_pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    let block = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();

    let start_block = block.header.number.saturating_sub(1);

    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let start_block_str = start_block.to_string();
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "3",
        "--start-block",
        start_block_str.as_str(),
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut indexer_process = Command::new(exe_path).args(args).spawn().unwrap();

    let provider = ctx.customer_provider.clone();

    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let now = block.header.timestamp;

    // Create request with short lock timeout
    let req = ProofRequest::new(
        RequestId::new(ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1),
            rampUpStart: now - 3,
            timeout: 7200,
            rampUpPeriod: 1,
            lockTimeout: 1800,
            lockCollateral: U256::from(0),
        },
    );
    let sig = req
        .sign_request(
            &ctx.customer_signer,
            ctx.deployment.boundless_market_address,
            anvil.chain_id(),
        )
        .await
        .unwrap();
    let sig_bytes: Bytes = sig.as_bytes().into();

    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx
        .customer_market
        .submit_request_with_signature(&req, sig_bytes.clone())
        .await
        .unwrap();

    provider.anvil_mine(Some(3), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify submitted status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Submitted.to_string(),
        "Request should be in submitted status"
    );

    // Lock request
    ctx
        .prover_market
        .lock_request(&req, sig_bytes.clone(), None)
        .await
        .unwrap();

    provider.anvil_mine(Some(2), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify locked status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Locked.to_string(),
        "Request should be in locked status"
    );
    let lock_end = status.lock_end;

    // Advance time past lock expiration
    provider.anvil_set_next_block_timestamp(lock_end as u64 + 1).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Fulfill request (late fulfillment after lock expired)
    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(req.clone(), sig_bytes.clone())]).await.unwrap();
    let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    ctx
        .prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await
        .unwrap();

    provider.anvil_mine(Some(2), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify fulfilled status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Fulfilled.to_string(),
        "Request should be in fulfilled status"
    );
    assert!(status.fulfilled_at.is_some(), "Request should have fulfilled_at timestamp");

    // Advance time and slash the fulfilled request
    provider.anvil_set_next_block_timestamp(req.expires_at() + 1).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    ctx.prover_market.slash(req.id).await.unwrap();

    tokio::time::sleep(Duration::from_secs(2)).await;

    // Verify slashed status (fulfilled takes priority, but slashed_status should be set)
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Fulfilled.to_string(),
        "Request should remain in fulfilled status (fulfilled takes priority)"
    );
    assert_eq!(
        status.slashed_status,
        SlashedStatus::Slashed.to_string(),
        "Request should have slashed status"
    );
    assert!(status.fulfilled_at.is_some(), "Request should still have fulfilled_at timestamp");
    assert!(status.slashed_at.is_some(), "Request should have slashed_at timestamp");
    assert!(status.slash_recipient.is_some(), "Request should have slash_recipient");
    assert!(
        status.slash_transferred_amount.is_some(),
        "Request should have slash_transferred_amount"
    );
    assert!(status.slash_burned_amount.is_some(), "Request should have slash_burned_amount");

    indexer_process.kill().unwrap();
}
