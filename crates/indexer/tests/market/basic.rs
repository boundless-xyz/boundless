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

use std::{
    process::Command,
    str::FromStr,
    time::Duration,
};

use assert_cmd::Command as AssertCommand;

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, U256, utils::parse_ether, B256},
    providers::{Provider, ext::AnvilApi, WalletProvider},
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use boundless_cli::{DefaultProver, OrderFulfilled};
use boundless_indexer::{
    db::{IndexerDb, market::{RequestStatusType, SlashedStatus, SortDirection}, DbError},
    test_utils::TestDb,
};
use boundless_market::contracts::{
    boundless_market::FulfillmentTx, Offer, Predicate, ProofRequest, RequestId, RequestInput,
    Requirements,
};
use boundless_test_utils::{
    guests::{ASSESSOR_GUEST_ELF, ECHO_ID, ECHO_PATH, SET_BUILDER_ELF},
    market::{create_test_ctx, TestCtx},
};
use sqlx::{AnyPool, Row};
use tracing_test::traced_test;

// Constant for indexer wait time between actions
const INDEXER_WAIT_DURATION: Duration = Duration::from_secs(5);

async fn count_hourly_summaries(pool: &AnyPool) -> i64 {
    let result = sqlx::query("SELECT COUNT(*) as count FROM hourly_market_summary")
        .fetch_one(pool)
        .await
        .unwrap();
    result.get("count")
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

// Helper to setup order stream server and client
async fn setup_order_stream<P: Provider>(
    anvil: &alloy::node_bindings::AnvilInstance,
    ctx: &TestCtx<P>,
    pool: sqlx::PgPool,
) -> (
    url::Url,
    boundless_market::order_stream_client::OrderStreamClient,
    tokio::task::JoinHandle<()>,
) {
    use boundless_market::order_stream_client::OrderStreamClient;
    use order_stream::{run_from_parts, AppState, ConfigBuilder};
    use std::net::{Ipv4Addr, SocketAddr};
    use url::Url;

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

    let order_stream_client = OrderStreamClient::new(
        order_stream_url.clone(),
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
    );

    (order_stream_url, order_stream_client, order_stream_handle)
}

// Helper to get block timestamp and start block number
async fn get_block_info<P: Provider>(provider: &P) -> (u64, u64) {
    let block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let block_timestamp = block.header.timestamp;
    let start_block = block.header.number;
    (block_timestamp, start_block)
}

// Helper to update all order created_at timestamps to be after a given block timestamp
async fn update_order_timestamps(pool: &sqlx::PgPool, block_timestamp: u64) {
    sqlx::query("UPDATE orders SET created_at = to_timestamp($1) WHERE created_at IS NOT NULL")
        .bind((block_timestamp + 1) as i64)
        .execute(pool)
        .await
        .unwrap();
}

// Helper to spawn indexer CLI with common args
fn spawn_indexer_cli(
    db_url: &str,
    rpc_url: &str,
    market_address: &str,
    retries: &str,
) -> std::process::Child {
    let cmd = AssertCommand::cargo_bin("market-indexer")
        .expect("market-indexer binary not found. Run `cargo build --bin market-indexer` first.");
    let exe_path = cmd.get_program().to_string_lossy().to_string();
    let args = [
        "--rpc-url",
        rpc_url,
        "--boundless-market-address",
        market_address,
        "--db",
        db_url,
        "--interval",
        "1",
        "--retries",
        retries,
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    Command::new(exe_path).args(args).spawn().unwrap()
}

// Helper to insert cycle counts with automatic overhead calculation
async fn insert_cycle_counts_with_overhead(
    test_db: &TestDb,
    request_digest: B256,
    program_cycles: u64,
) -> Result<(), DbError> {
    let total_cycles = (program_cycles as f64 * 1.0158) as u64;
    test_db.insert_test_cycle_counts(request_digest, program_cycles, total_cycles).await
}

// Helper to lock and fulfill a request
async fn lock_and_fulfill_request<P: Provider + WalletProvider + Clone + 'static>(
    ctx: &TestCtx<P>,
    prover: &DefaultProver,
    request: &ProofRequest,
    client_sig: Bytes,
) -> Result<(), Box<dyn std::error::Error>> {
    ctx.prover_market.lock_request(request, client_sig.clone(), None).await?;

    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(request.clone(), client_sig)]).await?;
    let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt)?;

    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await?;

    Ok(())
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
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    // Check that the proof was indexed
    let result = sqlx::query("SELECT * FROM proofs WHERE request_id == $1")
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

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Desc, None, None)
        .await
        .unwrap();
    let summary = &summaries[0];

    // Verify hour boundary alignment (timestamp should be divisible by 3600)
    assert_eq!(
        summary.period_timestamp % 3600,
        0,
        "Hour timestamp should be aligned to hour boundary"
    );

    // Verify counts match our test scenario
    assert_eq!(summary.total_fulfilled, 1, "Expected 1 fulfilled request");
    assert_eq!(summary.unique_provers_locking_requests, 1, "Expected 1 unique prover");
    assert_eq!(summary.unique_requesters_submitting_requests, 1, "Expected 1 unique requester");

    // Verify new request count fields
    assert_eq!(summary.total_requests_submitted, 1, "Expected 1 total request submitted");
    assert_eq!(
        summary.total_requests_submitted_onchain, 1,
        "Expected 1 onchain request (has requestSubmitted event)"
    );
    assert_eq!(summary.total_requests_submitted_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(summary.total_requests_locked, 1, "Expected 1 locked request");
    assert_eq!(summary.total_requests_slashed, 0, "Expected 0 slashed requests");

    // Verify fees and collateral are non-zero (the offer had minPrice=0, maxPrice=1, collateral=0)
    // But we should at least have the strings present
    assert!(!summary.total_fees_locked.is_empty(), "total_fees_locked should not be empty");
    assert!(
        !summary.total_collateral_locked.is_empty(),
        "total_collateral_locked should not be empty"
    );

    // Verify new expiration and fulfillment fields
    assert_eq!(summary.total_expired, 0, "Expected 0 expired requests (request was fulfilled)");
    assert_eq!(summary.total_locked_and_expired, 0, "Expected 0 locked and expired requests");
    assert_eq!(summary.total_locked_and_fulfilled, 1, "Expected 1 locked and fulfilled request");
    assert_eq!(
        summary.locked_orders_fulfillment_rate, 100.0,
        "Expected 100% fulfillment rate (1/1)"
    );

    // Verify cycle counts (will be 0 since test uses random address, not hardcoded SIGNAL_REQUESTOR/REQUESTOR_1/2)
    assert_eq!(summary.total_program_cycles, 0, "Expected 0 total_program_cycles for non-hardcoded requestor");
    assert_eq!(summary.total_cycles, 0, "Expected 0 total_cycles for non-hardcoded requestor");

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
    let first_request_id = format!("{:x}", request.id);

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
        tokio::time::sleep(INDEXER_WAIT_DURATION).await;
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
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    let total_fulfillments = monitor.total_proofs().await.unwrap();
    assert_eq!(total_fulfillments, 1);

    let total_fulfillments =
        monitor.total_proofs_from_client(ctx.customer_signer.address()).await.unwrap();
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

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();

    // Sum up all fulfilled across all hours (should be 1, since one was slashed)
    let total_fulfilled_across_hours: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
    assert_eq!(
        total_fulfilled_across_hours, 1,
        "Expected 1 total fulfilled across all hours (one was slashed)"
    );

    // Verify new request count fields across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();
    let total_locked: u64 = summaries.iter().map(|s| s.total_requests_locked).sum();
    let total_slashed: u64 = summaries.iter().map(|s| s.total_requests_slashed).sum();

    assert_eq!(total_requests_submitted, 2, "Expected 2 total requests submitted");
    assert_eq!(
        total_requests_onchain, 2,
        "Expected 2 onchain requests (both used submit_request_with_signature)"
    );
    assert_eq!(total_requests_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(total_locked, 2, "Expected 2 locked requests");
    assert_eq!(total_slashed, 1, "Expected 1 slashed request");

    // Verify new expiration and fulfillment fields across all hours
    let total_expired: u64 = summaries.iter().map(|s| s.total_expired).sum();
    let total_locked_and_expired: u64 = summaries.iter().map(|s| s.total_locked_and_expired).sum();
    let total_locked_and_fulfilled: u64 =
        summaries.iter().map(|s| s.total_locked_and_fulfilled).sum();

    assert_eq!(total_expired, 1, "Expected 1 expired request (the slashed one)");
    assert_eq!(total_locked_and_expired, 1, "Expected 1 locked and expired request");
    assert_eq!(total_locked_and_fulfilled, 1, "Expected 1 locked and fulfilled request");

    // Verify total_locked_and_expired_collateral matches the lock_collateral from the expired request
    let expired_request_collateral = get_lock_collateral(&test_db.pool, &first_request_id).await;
    let total_locked_and_expired_collateral: U256 = summaries
        .iter()
        .map(|s| U256::from_str(&s.total_locked_and_expired_collateral).unwrap_or(U256::ZERO))
        .sum();
    let expected_collateral = U256::from_str(&expired_request_collateral).unwrap_or(U256::ZERO);
    assert_eq!(
        total_locked_and_expired_collateral,
        expected_collateral,
        "total_locked_and_expired_collateral should match the lock_collateral from the expired request"
    );

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
    let test_db = TestDb::new().await.unwrap();

    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let mut cli_process = spawn_indexer_cli(
        &test_db.db_url,
        rpc_url.as_str(),
        &ctx.deployment.boundless_market_address.to_string(),
        "1",
    );

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

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

    // Create and fulfill first order with higher prices to make per-cycle calculations meaningful
    // Using 1 ETH max price (10^18 wei) so that lock_price / cycles gives non-zero values
    let one_eth = parse_ether("1").unwrap().into();
    let request1 = ProofRequest::new(
        RequestId::new(ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: one_eth,
            maxPrice: one_eth,
            rampUpStart: now - 3,
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig1 = request1.sign_request(&ctx.customer_signer, ctx.deployment.boundless_market_address, anvil.chain_id()).await.unwrap();

    ctx.customer_market.deposit(one_eth * U256::from(2)).await.unwrap();
    ctx.customer_market
        .submit_request_with_signature(&request1, client_sig1.as_bytes())
        .await
        .unwrap();

    // Insert cycle counts before locking so status service computes lock_price_per_cycle
    let request1_digest = request1.signing_hash(ctx.deployment.boundless_market_address, anvil.chain_id()).unwrap();
    let program_cycles1 = 50_000_000; // 50M cycles
    let total_cycles1 = (program_cycles1 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&test_db, request1_digest, program_cycles1).await.unwrap();

    lock_and_fulfill_request(&ctx, &prover, &request1, client_sig1.as_bytes().to_vec().into()).await.unwrap();

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

    // Create and fulfill second order in a different hour with higher prices
    let request2 = ProofRequest::new(
        RequestId::new(ctx.customer_signer.address(), 2),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: one_eth,
            maxPrice: one_eth, // Reuse 1 ETH max price
            rampUpStart: now2 - 3,
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig2 = request2.sign_request(&ctx.customer_signer, ctx.deployment.boundless_market_address, anvil.chain_id()).await.unwrap();

    ctx.customer_market
        .submit_request_with_signature(&request2, client_sig2.as_bytes())
        .await
        .unwrap();

    // Insert cycle counts before locking so status service computes lock_price_per_cycle
    let request2_digest = request2.signing_hash(ctx.deployment.boundless_market_address, anvil.chain_id()).unwrap();
    let program_cycles2 = 100_000_000; // 100M cycles
    let total_cycles2 = (program_cycles2 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&test_db, request2_digest, program_cycles2).await.unwrap();

    lock_and_fulfill_request(&ctx, &prover, &request2, client_sig2.as_bytes().to_vec().into()).await.unwrap();

    let now3 = ctx
        .customer_provider
        .clone()
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    provider.anvil_set_next_block_timestamp(now3 + 3700).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    // Wait for the events to be indexed and aggregated
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Check hourly aggregation across multiple hours
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(
        summary_count >= 2,
        "Expected at least 2 hourly summaries (one per hour), got {}",
        summary_count
    );

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();

    // Verify we have multiple hours
    assert!(summaries.len() >= 2, "Expected at least 2 different hours of data");

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
    assert_ne!(hour1, hour2, "Expected different hour timestamps for different hours");

    // Verify time difference is at least 1 hour
    let hour_diff = hour2.saturating_sub(hour1);
    assert!(
        hour_diff >= 3600,
        "Expected at least 1 hour difference between timestamps, got {}",
        hour_diff
    );

    // Total fulfilled across all hours should be 2
    let total_fulfilled_across_hours: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
    assert_eq!(total_fulfilled_across_hours, 2, "Expected 2 total fulfilled across all hours");

    // Verify new request count fields across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();
    let total_locked: u64 = summaries.iter().map(|s| s.total_requests_locked).sum();
    let total_slashed: u64 = summaries.iter().map(|s| s.total_requests_slashed).sum();

    assert_eq!(total_requests_submitted, 2, "Expected 2 total requests submitted");
    assert_eq!(
        total_requests_onchain, 2,
        "Expected 2 onchain requests (both used submit_request_with_signature)"
    );
    assert_eq!(total_requests_offchain, 0, "Expected 0 offchain requests");
    assert_eq!(total_locked, 2, "Expected 2 locked requests");
    assert_eq!(total_slashed, 0, "Expected 0 slashed requests");

    // Verify new expiration and fulfillment fields across all hours
    let total_expired: u64 = summaries.iter().map(|s| s.total_expired).sum();
    let total_locked_and_expired: u64 = summaries.iter().map(|s| s.total_locked_and_expired).sum();
    let total_locked_and_fulfilled: u64 =
        summaries.iter().map(|s| s.total_locked_and_fulfilled).sum();

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

    // Verify fee metrics from fulfilled requests are populated
    // Filter to summaries with fulfilled requests
    let summaries_with_fulfilled: Vec<_> =
        summaries.iter().filter(|s| s.total_fulfilled > 0).collect();

    // We should have at least 1 summary with fulfilled requests (we fulfilled 2 orders)
    assert!(
        !summaries_with_fulfilled.is_empty(),
        "Expected at least 1 summary with fulfilled requests, got {}",
        summaries_with_fulfilled.len()
    );

    // For each summary with fulfilled requests, verify fees and percentiles are populated
    // This validates that get_period_lock_pricing_data is correctly filtering by fulfilled requests
    for summary in summaries_with_fulfilled {
        assert_ne!(
            summary.total_fees_locked.trim_start_matches('0'),
            "",
            "Expected non-zero total_fees_locked for period with {} fulfilled requests",
            summary.total_fulfilled
        );

        assert_ne!(
            summary.p50_lock_price_per_cycle.trim_start_matches('0'),
            "",
            "Expected non-zero p50_lock_price_per_cycle for period with {} fulfilled requests",
            summary.total_fulfilled
        );
    }

    // Verify cycle count aggregations - both total across hours and per-hour values
    let total_cycles_across_hours: u64 = summaries.iter().map(|s| s.total_cycles).sum();
    let total_program_cycles_across_hours: u64 = summaries.iter().map(|s| s.total_program_cycles).sum();

    let expected_total_cycles = total_cycles1 + total_cycles2;
    let expected_total_program_cycles = program_cycles1 + program_cycles2;

    assert_eq!(
        total_cycles_across_hours, expected_total_cycles,
        "Expected total_cycles to equal {} + {} = {}, got {}",
        total_cycles1, total_cycles2, expected_total_cycles, total_cycles_across_hours
    );

    assert_eq!(
        total_program_cycles_across_hours, expected_total_program_cycles,
        "Expected total_program_cycles to equal {} + {} = {}, got {}",
        program_cycles1, program_cycles2, expected_total_program_cycles, total_program_cycles_across_hours
    );

    // Verify each hour has the correct cycle counts for its specific request
    let hours_with_fulfilled: Vec<_> = summaries.iter().filter(|s| s.total_fulfilled > 0).collect();
    assert_eq!(hours_with_fulfilled.len(), 2, "Expected exactly 2 hours with fulfilled requests");

    // First fulfilled hour should have request1's cycles
    assert_eq!(
        hours_with_fulfilled[0].total_program_cycles, program_cycles1,
        "Expected first hour to have {} program cycles (request1), got {}",
        program_cycles1, hours_with_fulfilled[0].total_program_cycles
    );
    assert_eq!(
        hours_with_fulfilled[0].total_cycles, total_cycles1,
        "Expected first hour to have {} total cycles (request1), got {}",
        total_cycles1, hours_with_fulfilled[0].total_cycles
    );

    // Second fulfilled hour should have request2's cycles
    assert_eq!(
        hours_with_fulfilled[1].total_program_cycles, program_cycles2,
        "Expected second hour to have {} program cycles (request2), got {}",
        program_cycles2, hours_with_fulfilled[1].total_program_cycles
    );
    assert_eq!(
        hours_with_fulfilled[1].total_cycles, total_cycles2,
        "Expected second hour to have {} total cycles (request2), got {}",
        total_cycles2, hours_with_fulfilled[1].total_cycles
    );

    // Verify p50_lock_price_per_cycle calculation
    // Since rampUpStart is (now - 3) and rampUpPeriod is 1, by the time requests are locked
    // the ramp-up is complete and lock_price = maxPrice = 1 ETH
    // Therefore: p50_lock_price_per_cycle = lock_price / program_cycles
    //   Request 1: 10^18 / 50,000,000 = 20,000,000,000
    //   Request 2: 10^18 / 100,000,000 = 10,000,000,000

    let expected_p50_request1 = U256::from(one_eth) / U256::from(program_cycles1);
    let expected_p50_request2 = U256::from(one_eth) / U256::from(program_cycles2);

    // First fulfilled hour should have request1's p50
    let p50_hour1: U256 = hours_with_fulfilled[0].p50_lock_price_per_cycle.trim_start_matches('0')
        .parse()
        .expect("Failed to parse p50_lock_price_per_cycle");
    assert_eq!(
        p50_hour1, expected_p50_request1,
        "Expected first hour p50_lock_price_per_cycle to be {} (1 ETH / {} cycles), got {}",
        expected_p50_request1, program_cycles1, p50_hour1
    );

    // Second fulfilled hour should have request2's p50
    let p50_hour2: U256 = hours_with_fulfilled[1].p50_lock_price_per_cycle.trim_start_matches('0')
        .parse()
        .expect("Failed to parse p50_lock_price_per_cycle");
    assert_eq!(
        p50_hour2, expected_p50_request2,
        "Expected second hour p50_lock_price_per_cycle to be {} (1 ETH / {} cycles), got {}",
        expected_p50_request2, program_cycles2, p50_hour2
    );

    tracing::info!(
        "Hour 1 p50: {} (expected {}), Hour 2 p50: {} (expected {})",
        p50_hour1, expected_p50_request1, p50_hour2, expected_p50_request2
    );

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_percentiles() {
    // Test multiple requests with different prices to validate percentile calculations
    // Creates 10 requests with prices from 0.1 ETH to 1.0 ETH
    // All requests use 100M cycles for simplicity

    let test_db = TestDb::new().await.unwrap();

    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let mut cli_process = spawn_indexer_cli(
        &test_db.db_url,
        rpc_url.as_str(),
        &ctx.deployment.boundless_market_address.to_string(),
        "1",
    );

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

    // Create 10 requests with different prices (0.1 ETH to 1.0 ETH)
    // This gives us a good distribution for testing percentiles
    let num_requests = 10;
    let base_price = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
    let cycles_per_request = 100_000_000u64; // 100M cycles for all requests
    let total_cycles_per_request = (cycles_per_request as f64 * 1.0158) as u64;

    // Deposit enough to cover all requests
    let total_deposit = U256::from(1_000_000_000_000_000_000u128) * U256::from(num_requests);
    ctx.customer_market.deposit(total_deposit).await.unwrap();

    let mut request_digests = Vec::new();

    for i in 0..num_requests {
        let request_id = i + 1;
        let max_price = base_price * U256::from(request_id); // 0.1 ETH, 0.2 ETH, ..., 1.0 ETH

        let request = ProofRequest::new(
            RequestId::new(ctx.customer_signer.address(), request_id as u32),
            Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().build_inline().unwrap(),
            Offer {
                minPrice: max_price,
                maxPrice: max_price,
                rampUpStart: now - 3,
                timeout: 300,
                rampUpPeriod: 1,
                lockTimeout: 300,
                lockCollateral: U256::from(0),
            },
        );

        let client_sig = request.sign_request(&ctx.customer_signer, ctx.deployment.boundless_market_address, anvil.chain_id()).await.unwrap();

        ctx.customer_market
            .submit_request_with_signature(&request, client_sig.as_bytes())
            .await
            .unwrap();

        // Insert cycle counts before locking so status service computes lock_price_per_cycle
        let digest = request.signing_hash(ctx.deployment.boundless_market_address, anvil.chain_id()).unwrap();
        insert_cycle_counts_with_overhead(&test_db, digest, cycles_per_request).await.unwrap();

        // Lock the request
        ctx.prover_market.lock_request(&request, client_sig.as_bytes(), None).await.unwrap();

        // Store for fulfillment
        request_digests.push((request.clone(), client_sig, digest));
    }

    // Fulfill all requests
    let fulfillment_requests: Vec<_> = request_digests.iter()
        .map(|(req, sig, _)| (req.clone(), sig.as_bytes().to_vec().into()))
        .collect();

    for chunk in fulfillment_requests.chunks(5) {
        let (fill, root_receipt, assessor_receipt) = prover.fulfill(chunk).await.unwrap();
        let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
        ctx.prover_market
            .fulfill(
                FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                    .with_submit_root(
                        ctx.deployment.set_verifier_address,
                        order_fulfilled.root,
                        order_fulfilled.seal,
                    )
            )
            .await
            .unwrap();
    }

    // Wait for indexer to process all fulfillments
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Query hourly summaries using the helper function
    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();

    tracing::info!("Retrieved {} hourly summaries", summaries.len());

    // Find the hour with all fulfilled requests
    let fulfilled_summary = summaries.iter()
        .find(|s| s.total_fulfilled == num_requests as u64)
        .expect("Should have exactly one hour with all 10 fulfilled requests");

    tracing::info!(
        "Found hour with {} fulfilled requests: p10={}, p50={}, p90={}",
        fulfilled_summary.total_fulfilled,
        fulfilled_summary.p10_lock_price_per_cycle,
        fulfilled_summary.p50_lock_price_per_cycle,
        fulfilled_summary.p90_lock_price_per_cycle
    );

    // Parse actual percentile values from database
    // With 10 requests priced 0.1 ETH to 1.0 ETH (all using 100M cycles),
    // price per cycle ranges from 1e9 wei (0.1 ETH / 100M) to 1e10 wei (1.0 ETH / 100M)
    let actual_p10: U256 = fulfilled_summary.p10_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);
    let actual_p50: U256 = fulfilled_summary.p50_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);
    let actual_p90: U256 = fulfilled_summary.p90_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);

    // Define expected ranges based on ETH prices (0.1 ETH to 1.0 ETH, all using 100M cycles)
    // Price per cycle = ETH_price / 100M cycles
    // 0.1 ETH = 1e17 wei, so 0.1 ETH / 100M = 1e17 / 1e8 = 1e9 wei
    // 1.0 ETH = 1e18 wei, so 1.0 ETH / 100M = 1e18 / 1e8 = 1e10 wei
    let min_price_per_cycle = U256::from(1_000_000_000u64); // 0.1 ETH / 100M = 1e9 wei
    let max_price_per_cycle = U256::from(10_000_000_000u64); // 1.0 ETH / 100M = 1e10 wei

    // Check that percentiles are in reasonable ballpark ranges
    // p10: should be in lower range (around 0.1-0.2 ETH = 1-2e9 wei)
    assert!(
        actual_p10 >= min_price_per_cycle && actual_p10 <= U256::from(2_000_000_000u64),
        "p10 should be in lower range (1-2e9 wei), got {}",
        actual_p10
    );

    // p50: should be around median (around 0.4-0.6 ETH = 4-6e9 wei)
    assert!(
        actual_p50 >= U256::from(4_000_000_000u64) && actual_p50 <= U256::from(6_000_000_000u64),
        "p50 should be around median (3-6e9 wei), got {}",
        actual_p50
    );

    // p90: should be in upper range (around 0.8-1.0 ETH = 8-10e9 wei)
    assert!(
        actual_p90 >= U256::from(8_000_000_000u64) && actual_p90 <= max_price_per_cycle,
        "p90 should be in upper range (7-10e9 wei), got {}",
        actual_p90
    );

    // Verify total cycles
    let expected_total_cycles = total_cycles_per_request * num_requests as u64;
    assert_eq!(
        fulfilled_summary.total_cycles, expected_total_cycles,
        "Total cycles should be {} * {} = {}",
        total_cycles_per_request, num_requests, expected_total_cycles
    );

    tracing::info!(
        "All percentiles are in expected ranges: p10={}, p50={}, p90={}",
        actual_p10, actual_p50, actual_p90
    );

    cli_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_indexer_with_order_stream(pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Setup order stream server and client
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&anvil, &ctx, pool.clone()).await;

    // Get block info
    let (block_timestamp, _start_block) = get_block_info(&ctx.customer_provider).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    // Update all order created_at timestamps to be after the first block timestamp
    update_order_timestamps(&pool, block_timestamp).await;

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
    let provider = ctx.customer_provider.clone();
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
        &"1".to_string(),
    ];

    println!("{exe_path} {args:?}");

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Wait for the indexer to process orders
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    // Verify aggregation statistics include offchain requests
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(
        summary_count >= 1,
        "Expected at least one hourly summary, got {}",
        summary_count
    );

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();

    // Sum up counts across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();

    assert_eq!(total_requests_submitted, 3, "Expected 3 total requests submitted");
    assert_eq!(total_requests_onchain, 0, "Expected 0 onchain requests (all submitted via order stream)");
    assert_eq!(total_requests_offchain, 3, "Expected 3 offchain requests");

    // Verify unique requesters count includes offchain requests
    let unique_requesters: u64 = summaries
        .iter()
        .map(|s| s.unique_requesters_submitting_requests)
        .max()
        .unwrap_or(0);
    assert_eq!(
        unique_requesters, 1,
        "Expected 1 unique requester (all requests from same address)"
    );

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_offchain_and_onchain_mixed_aggregation(pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Setup order stream server and client
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&anvil, &ctx, pool.clone()).await;

    // Get block info
    let (block_timestamp, start_block) = get_block_info(&ctx.customer_provider).await;

    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Submit 2 onchain requests
    let (req1_onchain, sig1) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx.customer_market.submit_request_with_signature(&req1_onchain, sig1).await.unwrap();

    let (req2_onchain, sig2) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        2,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    ctx.customer_market.submit_request_with_signature(&req2_onchain, sig2).await.unwrap();

    // Submit 3 offchain requests to order stream
    let (req1_offchain, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        10,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req1_offchain, &ctx.customer_signer).await.unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let (req2_offchain, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        11,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req2_offchain, &ctx.customer_signer).await.unwrap();

    tokio::time::sleep(Duration::from_secs(1)).await;

    let (req3_offchain, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        12,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req3_offchain, &ctx.customer_signer).await.unwrap();

    // Update all order created_at timestamps to be after the first block timestamp
    update_order_timestamps(&pool, block_timestamp).await;

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
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Verify all 5 orders were indexed (2 onchain + 3 offchain)
    let result = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let count: i64 = result.get("count");
    assert_eq!(count, 5, "Expected 5 proof requests to be indexed");

    // Verify source distribution
    let result =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'onchain'")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
    let onchain_count: i64 = result.get("count");
    assert_eq!(onchain_count, 2, "Expected 2 requests to have source='onchain'");

    let result =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
    let offchain_count: i64 = result.get("count");
    assert_eq!(offchain_count, 3, "Expected 3 requests to have source='offchain'");

    // Verify aggregation statistics include both onchain and offchain requests
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(
        summary_count >= 1,
        "Expected at least one hourly summary, got {}",
        summary_count
    );

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();

    // Sum up counts across all hours
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_requests_onchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
    let total_requests_offchain: u64 =
        summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();

    assert_eq!(total_requests_submitted, 5, "Expected 5 total requests submitted");
    assert_eq!(total_requests_onchain, 2, "Expected 2 onchain requests");
    assert_eq!(total_requests_offchain, 3, "Expected 3 offchain requests");

    // Verify unique requesters count includes both sources
    let unique_requesters: u64 = summaries
        .iter()
        .map(|s| s.unique_requesters_submitting_requests)
        .max()
        .unwrap_or(0);
    assert_eq!(
        unique_requesters, 1,
        "Expected 1 unique requester (all requests from same address)"
    );

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_submission_timestamp_field(pool: sqlx::PgPool) {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Setup order stream server and client
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&anvil, &ctx, pool.clone()).await;

    // Get block info
    let (block_timestamp, start_block) = get_block_info(&ctx.customer_provider).await;

    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    // Submit 1 onchain request
    let (req_onchain, sig_onchain) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    ctx.customer_market.submit_request_with_signature(&req_onchain, sig_onchain).await.unwrap();

    // Get the block timestamp of the onchain submission
    let provider = ctx.customer_provider.clone();
    let onchain_block = provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let onchain_block_timestamp = onchain_block.header.timestamp;

    // Submit 1 offchain request
    let (req_offchain, _) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        10,
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req_offchain, &ctx.customer_signer).await.unwrap();

    // Update all order created_at timestamps to be after the first block timestamp
    update_order_timestamps(&pool, block_timestamp).await;

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
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Verify onchain request has submission_timestamp equal to block_timestamp
    let result = sqlx::query(
        "SELECT block_timestamp, submission_timestamp, source FROM proof_requests WHERE request_id = $1",
    )
    .bind(format!("{:x}", req_onchain.id))
    .fetch_one(&test_db.pool)
    .await
    .unwrap();
    let onchain_block_ts: i64 = result.get("block_timestamp");
    let onchain_submission_ts: i64 = result.get("submission_timestamp");
    let onchain_source: String = result.get("source");
    assert_eq!(onchain_source, "onchain");
    assert_eq!(
        onchain_submission_ts, onchain_block_ts,
        "Onchain request submission_timestamp should equal block_timestamp"
    );
    assert_eq!(
        onchain_block_ts, onchain_block_timestamp as i64,
        "block_timestamp should match the actual block timestamp"
    );

    // Verify offchain request has submission_timestamp != 0 (block_timestamp is 0 for offchain)
    let result = sqlx::query(
        "SELECT block_timestamp, submission_timestamp, source FROM proof_requests WHERE request_id = $1",
    )
    .bind(format!("{:x}", req_offchain.id))
    .fetch_one(&test_db.pool)
    .await
    .unwrap();
    let offchain_block_ts: i64 = result.get("block_timestamp");
    let offchain_submission_ts: i64 = result.get("submission_timestamp");
    let offchain_source: String = result.get("source");
    assert_eq!(offchain_source, "offchain");
    assert_eq!(
        offchain_block_ts, 0,
        "Offchain request block_timestamp should be 0 (sentinel value)"
    );
    assert!(
        offchain_submission_ts > 0,
        "Offchain request submission_timestamp should be > 0 (from order stream created_at)"
    );

    // Verify both requests appear in aggregation (verifies submission_timestamp filtering works)
    let summary_count = count_hourly_summaries(&test_db.pool).await;
    assert!(
        summary_count >= 1,
        "Expected at least one hourly summary, got {}",
        summary_count
    );

    let summaries = test_db.db
        .get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    assert_eq!(
        total_requests_submitted, 2,
        "Expected 2 total requests (1 onchain + 1 offchain) to appear in aggregation"
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
    let mut cli_process_receipts = Command::new(&exe_path).args(args_receipts).spawn().unwrap();

    // Wait for the indexer to process events
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;
    cli_process_receipts.kill().ok(); // Kill if still running

    // Wait a moment for cleanup
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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
    let mut cli_process_tx_by_hash = Command::new(&exe_path).args(args_tx_by_hash).spawn().unwrap();

    // Wait for the indexer to process events
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;
    cli_process_tx_by_hash.kill().ok(); // Kill if still running

    // Wait a moment for cleanup
    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    assert_eq!(txs_receipts.len(), txs_tx_by_hash.len(), "Should have same number of transactions");

    // Compare each transaction
    for (i, (tx_receipts, tx_tx_by_hash)) in
        txs_receipts.iter().zip(txs_tx_by_hash.iter()).enumerate()
    {
        assert_eq!(tx_receipts.0, tx_tx_by_hash.0, "Transaction {} tx_hash should match", i);
        assert_eq!(tx_receipts.1, tx_tx_by_hash.1, "Transaction {} from_address should match", i);
        assert_eq!(tx_receipts.2, tx_tx_by_hash.2, "Transaction {} block_number should match", i);
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
    let count_requests_receipts: i64 = sqlx::query("SELECT COUNT(*) as count FROM proof_requests")
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
    assert_eq!(count_requests_receipts, 1, "Expected 1 proof request to be indexed");
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

async fn get_lock_collateral(pool: &AnyPool, request_id: &str) -> String {
    let row = sqlx::query(
        "SELECT lock_collateral FROM request_status WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .unwrap();
    row.get("lock_collateral")
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

    let block =
        ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();

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

    let block = provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
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
    ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    let block =
        ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();

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

    let block = provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
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
    ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Verify submitted status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Submitted.to_string(),
        "Request should be in submitted status"
    );

    // Lock request
    ctx.prover_market.lock_request(&req, sig.clone(), None).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Verify locked status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Locked.to_string(),
        "Request should be in locked status"
    );

    // Advance time past request expiration
    let expires_at = req.expires_at();
    tracing::info!("Request expires at: {}.", expires_at);
    provider.anvil_set_next_block_timestamp(expires_at).await.unwrap();
    provider.anvil_mine(Some(1), None).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    let block =
        ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();

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

    let block = provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
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
    ctx.customer_market.submit_request_with_signature(&req, sig_bytes.clone()).await.unwrap();

    provider.anvil_mine(Some(3), None).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Verify submitted status
    let status = get_request_status(&test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(
        status.request_status,
        RequestStatusType::Submitted.to_string(),
        "Request should be in submitted status"
    );

    // Lock request
    ctx.prover_market.lock_request(&req, sig_bytes.clone(), None).await.unwrap();

    provider.anvil_mine(Some(2), None).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    // Fulfill request (late fulfillment after lock expired)
    let (fill, root_receipt, assessor_receipt) =
        prover.fulfill(&[(req.clone(), sig_bytes.clone())]).await.unwrap();
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

    provider.anvil_mine(Some(2), None).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

    ctx.prover_market.slash(req.id).await.unwrap();

    tokio::time::sleep(INDEXER_WAIT_DURATION).await;

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
