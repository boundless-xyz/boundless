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

//! Common test fixtures and helpers for market indexer tests

use std::{process::Command, sync::Arc, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, U256},
    providers::{ext::AnvilApi, Provider, WalletProvider},
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use assert_cmd::Command as AssertCommand;
use boundless_cli::OrderFulfiller;
use broker::provers::DefaultProver as BrokerDefaultProver;

use boundless_indexer::{
    db::{market::SortDirection, IndexerDb, MarketDb},
    test_utils::TestDb,
};
use boundless_market::contracts::{
    Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
};
use boundless_test_utils::{
    guests::{ECHO_ID, ECHO_PATH},
    market::{create_test_ctx, TestCtx},
};
use sqlx::{PgPool, Row};

/// Test fixture containing all common test setup
pub struct MarketTestFixture<P: Provider + WalletProvider + Clone + 'static> {
    pub test_db: TestDb,
    pub anvil: alloy::node_bindings::AnvilInstance,
    pub ctx: TestCtx<P>,
    pub prover: OrderFulfiller,
}

impl<P: Provider + WalletProvider + Clone + 'static> MarketTestFixture<P> {
    // Methods for MarketTestFixture with a specific provider type
}

/// Create a new test fixture with all setup
pub async fn new_market_test_fixture(
    pool: PgPool,
) -> Result<
    MarketTestFixture<impl Provider + WalletProvider + Clone + 'static>,
    Box<dyn std::error::Error>,
> {
    let db_url = get_db_url_from_pool(&pool).await;
    let test_db = TestDb::from_pool(db_url, pool).await?;
    let anvil = Anvil::new().spawn();
    let ctx = create_test_ctx(&anvil).await?;

    let client = boundless_market::Client::new(ctx.prover_market.clone(), ctx.set_verifier.clone());
    let prover = OrderFulfiller::initialize(Arc::new(BrokerDefaultProver::default()), &client)
        .await
        .unwrap();

    Ok(MarketTestFixture { test_db, anvil, ctx, prover })
}

pub async fn create_isolated_db_pool(base_name: &str) -> (String, PgPool) {
    use std::time::SystemTime;
    use url::Url;

    let base_db_url =
        std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
    let test_db_name = format!(
        "{}_{}",
        base_name,
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos()
    );

    let mut parsed = Url::parse(&base_db_url).expect("Invalid DATABASE_URL");
    parsed.set_path(&format!("/{test_db_name}"));
    let db_url = parsed.to_string();

    let admin_pool = PgPool::connect(&base_db_url).await.expect("Failed to connect to database");
    sqlx::query(&format!(r#"CREATE DATABASE "{test_db_name}""#))
        .execute(&admin_pool)
        .await
        .expect("Failed to create test database");

    let pool = PgPool::connect(&db_url).await.expect("Failed to connect to database");

    (db_url, pool)
}

/// Extract the database connection string from a sqlx::test PgPool.
/// sqlx::test creates an isolated database per test with a unique name.
async fn get_db_url_from_pool(pool: &PgPool) -> String {
    let base_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .expect("failed to query current_database()");

    if let Some(last_slash) = base_url.rfind('/') {
        format!("{}/{}", &base_url[..last_slash], db_name)
    } else {
        format!("{}/{}", base_url, db_name)
    }
}

/// Builder for spawning indexer CLI with various configurations
pub struct IndexerCliBuilder {
    db_url: String,
    rpc_url: String,
    market_address: String,
    interval: String,
    retries: String,
    start_block: Option<String>,
    end_block: Option<String>,
    batch_size: Option<String>,
    tx_fetch_strategy: Option<String>,
    order_stream_url: Option<String>,
}

impl IndexerCliBuilder {
    pub fn new(db_url: String, rpc_url: String, market_address: String) -> Self {
        Self {
            db_url,
            rpc_url,
            market_address,
            interval: "1".to_string(),
            retries: "0".to_string(),
            start_block: None,
            end_block: None,
            batch_size: None,
            tx_fetch_strategy: None,
            order_stream_url: None,
        }
    }

    #[allow(dead_code)]
    pub fn interval(mut self, interval: &str) -> Self {
        self.interval = interval.to_string();
        self
    }

    pub fn retries(mut self, retries: &str) -> Self {
        self.retries = retries.to_string();
        self
    }

    pub fn start_block(mut self, start_block: &str) -> Self {
        self.start_block = Some(start_block.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn end_block(mut self, end_block: &str) -> Self {
        self.end_block = Some(end_block.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn batch_size(mut self, batch_size: &str) -> Self {
        self.batch_size = Some(batch_size.to_string());
        self
    }

    #[allow(dead_code)]
    pub fn tx_fetch_strategy(mut self, strategy: &str) -> Self {
        self.tx_fetch_strategy = Some(strategy.to_string());
        self
    }

    pub fn order_stream_url(mut self, url: &str) -> Self {
        self.order_stream_url = Some(url.to_string());
        self
    }

    pub fn spawn(self) -> std::io::Result<std::process::Child> {
        let cmd = AssertCommand::cargo_bin("market-indexer").expect(
            "market-indexer binary not found. Run `cargo build --bin market-indexer` first.",
        );
        let exe_path = cmd.get_program().to_string_lossy().to_string();

        let mut args = vec![
            "--rpc-url".to_string(),
            self.rpc_url,
            "--boundless-market-address".to_string(),
            self.market_address,
            "--db".to_string(),
            self.db_url,
            "--interval".to_string(),
            self.interval,
            "--retries".to_string(),
            self.retries,
        ];

        if let Some(start_block) = self.start_block {
            args.push("--start-block".to_string());
            args.push(start_block);
        }

        if let Some(end_block) = self.end_block {
            args.push("--end-block".to_string());
            args.push(end_block);
        }

        if let Some(batch_size) = self.batch_size {
            args.push("--batch-size".to_string());
            args.push(batch_size);
        }

        if let Some(strategy) = self.tx_fetch_strategy {
            args.push("--tx-fetch-strategy".to_string());
            args.push(strategy);
        }

        if let Some(url) = self.order_stream_url {
            args.push("--order-stream-url".to_string());
            args.push(url);
        }

        println!("{} {:?}", exe_path, args);

        #[allow(clippy::zombie_processes)]
        Command::new(exe_path).env("DB_POOL_SIZE", "5").args(args).spawn()
    }
}

/// Builder for spawning backfill CLI with various configurations
pub struct BackfillCliBuilder {
    db_url: String,
    rpc_url: String,
    market_address: String,
    mode: String,
    start_block: u64,
    end_block: Option<u64>,
    tx_fetch_strategy: Option<String>,
}

impl BackfillCliBuilder {
    pub fn new(db_url: String, rpc_url: String, market_address: String) -> Self {
        Self {
            db_url,
            rpc_url,
            market_address,
            mode: "aggregates".to_string(),
            start_block: 0,
            end_block: None,
            tx_fetch_strategy: None,
        }
    }

    pub fn mode(mut self, mode: &str) -> Self {
        self.mode = mode.to_string();
        self
    }

    pub fn start_block(mut self, start_block: u64) -> Self {
        self.start_block = start_block;
        self
    }

    pub fn end_block(mut self, end_block: u64) -> Self {
        self.end_block = Some(end_block);
        self
    }

    #[allow(dead_code)]
    pub fn tx_fetch_strategy(mut self, strategy: &str) -> Self {
        self.tx_fetch_strategy = Some(strategy.to_string());
        self
    }

    pub fn spawn(self) -> std::io::Result<std::process::Child> {
        let cmd = AssertCommand::cargo_bin("market-indexer-backfill").expect(
            "market-indexer-backfill binary not found. Run `cargo build --bin market-indexer-backfill` first.",
        );
        let exe_path = cmd.get_program().to_string_lossy().to_string();

        let mut args = vec![
            "--mode".to_string(),
            self.mode,
            "--rpc-url".to_string(),
            self.rpc_url,
            "--boundless-market-address".to_string(),
            self.market_address,
            "--db".to_string(),
            self.db_url,
            "--start-block".to_string(),
            self.start_block.to_string(),
        ];

        if let Some(end_block) = self.end_block {
            args.push("--end-block".to_string());
            args.push(end_block.to_string());
        }

        if let Some(strategy) = self.tx_fetch_strategy {
            args.push("--tx-fetch-strategy".to_string());
            args.push(strategy);
        }

        println!("{} {:?}", exe_path, args);

        #[allow(clippy::zombie_processes)]
        Command::new(exe_path).env("DB_POOL_SIZE", "5").args(args).spawn()
    }
}

/// Helper to create an order with default configuration
pub async fn create_order(
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
            timeout: 24,
            rampUpPeriod: 1,
            lockTimeout: 24,
            lockCollateral: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

/// Helper to create an order with custom timeouts
#[allow(clippy::too_many_arguments)]
pub async fn create_order_with_timeouts(
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

/// Helper to create an order with custom collateral
pub async fn create_order_with_collateral(
    signer: &impl Signer,
    signer_addr: Address,
    order_id: u32,
    contract_addr: Address,
    chain_id: u64,
    now: u64,
    lock_collateral: U256,
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
            timeout: 3600, // Long timeout to ensure request doesn't expire
            rampUpPeriod: 1,
            lockTimeout: 3600, // Long lock timeout to ensure we have time to lock
            lockCollateral: lock_collateral,
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

/// Helper to setup order stream server and client
pub async fn setup_order_stream<P: Provider>(
    anvil: &alloy::node_bindings::AnvilInstance,
    ctx: &TestCtx<P>,
    pool: sqlx::PgPool,
) -> (url::Url, boundless_market::order_stream_client::OrderStreamClient, tokio::task::JoinHandle<()>)
{
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

/// Helper to get block timestamp and start block number
pub async fn get_block_info<P: Provider>(provider: &P) -> (u64, u64) {
    let block = provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
    let block_timestamp = block.header.timestamp;
    let start_block = block.header.number;
    (block_timestamp, start_block)
}

/// Helper to get latest block timestamp
pub async fn get_latest_block_timestamp<P: Provider>(provider: &P) -> u64 {
    provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap().header.timestamp
}

/// Helper to update all order created_at timestamps to be after a given block timestamp
pub async fn update_order_timestamps(pool: &sqlx::PgPool, block_timestamp: u64) {
    sqlx::query("UPDATE orders SET created_at = to_timestamp($1) WHERE created_at IS NOT NULL")
        .bind((block_timestamp + 1) as i64)
        .execute(pool)
        .await
        .unwrap();
}

/// Helper to submit a request with automatic deposit if needed
/// Ensures the customer has sufficient balance to cover the lock price
pub async fn submit_request_with_deposit<P: Provider + WalletProvider + Clone + 'static>(
    ctx: &TestCtx<P>,
    request: &ProofRequest,
    client_sig: Bytes,
) -> Result<(), Box<dyn std::error::Error>> {
    // Check current balance
    let current_balance = ctx.customer_market.balance_of(ctx.customer_signer.address()).await?;

    // Calculate required balance (use maxPrice as a safe estimate)
    let required_balance = request.offer.maxPrice;

    // If balance is insufficient, deposit more
    if current_balance < required_balance {
        let needed = required_balance - current_balance;
        // Add a small buffer for safety
        let deposit_amount = needed + U256::from(10);
        ctx.customer_market.deposit(deposit_amount).await?;
    }

    ctx.customer_market.submit_request_with_signature(request, client_sig).await?;

    Ok(())
}

/// Helper to lock and fulfill a request
pub async fn lock_and_fulfill_request<P: Provider + WalletProvider + Clone + 'static>(
    ctx: &TestCtx<P>,
    prover: &OrderFulfiller,
    request: &ProofRequest,
    client_sig: Bytes,
) -> Result<(), Box<dyn std::error::Error>> {
    use boundless_cli::OrderFulfilled;
    use boundless_market::contracts::boundless_market::FulfillmentTx;

    tracing::debug!("Locking request {}", request.id);
    ctx.prover_market.lock_request(request, client_sig.clone()).await?;

    tracing::debug!("Fulfilling request {}", request.id);
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

/// Helper to lock and fulfill a request with collateral setup
/// This ensures the prover has sufficient collateral deposited before locking
pub async fn lock_and_fulfill_request_with_collateral<
    P: Provider + WalletProvider + Clone + 'static,
>(
    ctx: &TestCtx<P>,
    prover: &OrderFulfiller,
    request: &ProofRequest,
    client_sig: Bytes,
) -> Result<(), Box<dyn std::error::Error>> {
    use boundless_cli::OrderFulfilled;
    use boundless_market::contracts::boundless_market::FulfillmentTx;

    // If request requires collateral, ensure prover has it deposited
    if request.offer.lockCollateral > U256::ZERO {
        // Check current collateral balance
        let mut current_balance =
            ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await?;

        // If balance is insufficient, deposit more
        if current_balance < request.offer.lockCollateral {
            let needed = request.offer.lockCollateral - current_balance;
            tracing::debug!(
                "Prover needs {} collateral, current balance: {}, depositing {}",
                request.offer.lockCollateral,
                current_balance,
                needed
            );
            ctx.prover_market.approve_deposit_collateral(needed).await?;
            ctx.prover_market.deposit_collateral(needed).await?;

            // Verify deposit succeeded - deposit_collateral already waits for confirmation
            current_balance =
                ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await?;
            tracing::debug!("After deposit, prover collateral balance: {}", current_balance);
            if current_balance < request.offer.lockCollateral {
                return Err(format!(
                    "Failed to deposit sufficient collateral: have {}, need {}",
                    current_balance, request.offer.lockCollateral
                )
                .into());
            }
        }
    }

    ctx.prover_market.lock_request(request, client_sig.clone()).await?;

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

/// Helper to fulfill a request that's already locked
pub async fn fulfill_request<P: Provider + WalletProvider + Clone + 'static>(
    ctx: &TestCtx<P>,
    prover: &OrderFulfiller,
    request: &ProofRequest,
    client_sig: Bytes,
) -> Result<(), Box<dyn std::error::Error>> {
    use boundless_cli::OrderFulfilled;
    use boundless_market::contracts::boundless_market::FulfillmentTx;

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

/// Helper to insert cycle counts with automatic overhead calculation.
/// Returns the updated_at timestamp that was set, so callers can ensure mining happens beyond it.
pub async fn insert_cycle_counts_with_overhead(
    test_db: &TestDb,
    request_digest: alloy::primitives::B256,
    program_cycles: u64,
) -> Result<u64, boundless_indexer::db::DbError> {
    let total_cycles = (program_cycles as f64 * 1.0158) as u64;
    test_db
        .insert_test_cycle_counts(
            request_digest,
            U256::from(program_cycles),
            U256::from(total_cycles),
        )
        .await
}

/// Helper to advance time and mine blocks
pub async fn advance_time_and_mine<P: Provider + WalletProvider + Clone + 'static>(
    provider: &P,
    advance_seconds: u64,
    blocks: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let now = get_latest_block_timestamp(provider).await;
    provider.anvil_set_next_block_timestamp(now + advance_seconds).await?;
    provider.anvil_mine(Some(blocks), None).await?;
    Ok(())
}

/// Helper to advance time and mine blocks
pub async fn advance_time_to_and_mine<P: Provider + WalletProvider + Clone + 'static>(
    provider: &P,
    target_timestamp: u64,
    blocks: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    provider.anvil_set_next_block_timestamp(target_timestamp).await?;
    provider.anvil_mine(Some(blocks), None).await?;
    Ok(())
}

/// Wait for indexer to process up to the current block number
pub async fn wait_for_indexer<P: Provider>(provider: &P, pool: &PgPool) {
    // Get current block number from the chain
    let current_block = provider.get_block_number().await.unwrap();
    // Get block timestamp from the chain
    let block =
        provider.get_block_by_number(BlockNumberOrTag::Number(current_block)).await.unwrap();
    let block_timestamp = block.unwrap().header.timestamp;

    // Wait for indexer to process up to current block with timeout
    let timeout = Duration::from_secs(30);
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    loop {
        // Query last processed block from main indexer (id=0) and aggregation indexer (id=1)
        let main_result: Result<(String,), _> =
            sqlx::query_as("SELECT block FROM last_block WHERE id = 0").fetch_one(pool).await;
        let aggregation_result: Result<(String,), _> =
            sqlx::query_as("SELECT block FROM last_block WHERE id = 1").fetch_one(pool).await;

        if let (Ok(main_res), Ok(agg_res)) = (&main_result, &aggregation_result) {
            let main_block: u64 = main_res.0.parse::<u64>().unwrap_or(0);
            let agg_block: u64 = agg_res.0.parse::<u64>().unwrap_or(0);

            let main_caught_up = main_block >= current_block;
            let aggregation_caught_up = agg_block >= current_block;

            if main_caught_up && aggregation_caught_up {
                tracing::info!(
                    "Indexer caught up to block {} at timestamp {}",
                    current_block,
                    block_timestamp
                );
                return;
            }
        }

        if start.elapsed() > timeout {
            let main_block_info = main_result
                .as_ref()
                .ok()
                .and_then(|r| r.0.parse::<u64>().ok())
                .map_or("NO_BLOCK".to_string(), |b| b.to_string());
            let aggregation_block_info = aggregation_result
                .as_ref()
                .ok()
                .and_then(|r| r.0.parse::<u64>().ok())
                .map_or("NO_BLOCK".to_string(), |b| b.to_string());
            panic!(
                "Timeout waiting for indexers to reach block {}. Main indexer: {}, Aggregation indexer: {}",
                current_block, main_block_info, aggregation_block_info
            );
        }

        tokio::time::sleep(poll_interval).await;
    }
}

/// Count rows in any table
pub async fn count_table_rows(pool: &PgPool, table_name: &str) -> i64 {
    let query = format!("SELECT COUNT(*) as count FROM {}", table_name);
    let result = sqlx::query(&query).fetch_one(pool).await.unwrap();
    result.get("count")
}

/// Verify a request exists in a specific table
pub async fn verify_request_in_table(pool: &PgPool, request_id: &str, table_name: &str) -> String {
    let query = format!("SELECT * FROM {} WHERE request_id = $1", table_name);
    let result = sqlx::query(&query).bind(request_id).fetch_one(pool).await.unwrap();
    result.get::<String, _>("request_id")
}

/// Get lock collateral for a request
#[allow(dead_code)]
pub async fn get_lock_collateral(pool: &PgPool, request_id: &str) -> String {
    let row = sqlx::query("SELECT lock_collateral FROM request_status WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .unwrap();
    row.get("lock_collateral")
}

/// Expected values for summary verification
#[derive(Debug, Default)]
pub struct SummaryExpectations {
    pub total_requests_submitted: Option<u64>,
    pub total_requests_onchain: Option<u64>,
    pub total_requests_offchain: Option<u64>,
    pub total_locked: Option<u64>,
    pub total_slashed: Option<u64>,
    pub total_fulfilled: Option<u64>,
    pub total_expired: Option<u64>,
    pub total_locked_and_expired: Option<u64>,
    pub total_locked_and_fulfilled: Option<u64>,
    pub total_secondary_fulfillments: Option<u64>,
}

/// Verify summary totals across all summaries
pub fn verify_summary_totals(
    summaries: &[boundless_indexer::db::market::HourlyMarketSummary],
    expectations: SummaryExpectations,
) {
    if let Some(expected) = expectations.total_requests_submitted {
        let total: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
        assert_eq!(
            total, expected,
            "Expected {} total requests submitted, got {}",
            expected, total
        );
    }

    if let Some(expected) = expectations.total_requests_onchain {
        let total: u64 = summaries.iter().map(|s| s.total_requests_submitted_onchain).sum();
        assert_eq!(total, expected, "Expected {} onchain requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_requests_offchain {
        let total: u64 = summaries.iter().map(|s| s.total_requests_submitted_offchain).sum();
        assert_eq!(total, expected, "Expected {} offchain requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_locked {
        let total: u64 = summaries.iter().map(|s| s.total_requests_locked).sum();
        assert_eq!(total, expected, "Expected {} locked requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_slashed {
        let total: u64 = summaries.iter().map(|s| s.total_requests_slashed).sum();
        assert_eq!(total, expected, "Expected {} slashed requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_fulfilled {
        let total: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
        assert_eq!(total, expected, "Expected {} fulfilled requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_expired {
        let total: u64 = summaries.iter().map(|s| s.total_expired).sum();
        assert_eq!(total, expected, "Expected {} expired requests, got {}", expected, total);
    }

    if let Some(expected) = expectations.total_locked_and_expired {
        let total: u64 = summaries.iter().map(|s| s.total_locked_and_expired).sum();
        assert_eq!(
            total, expected,
            "Expected {} locked and expired requests, got {}",
            expected, total
        );
    }

    if let Some(expected) = expectations.total_locked_and_fulfilled {
        let total: u64 = summaries.iter().map(|s| s.total_locked_and_fulfilled).sum();
        assert_eq!(
            total, expected,
            "Expected {} locked and fulfilled requests, got {}",
            expected, total
        );
    }

    if let Some(expected) = expectations.total_secondary_fulfillments {
        let total: u64 = summaries.iter().map(|s| s.total_secondary_fulfillments).sum();
        assert_eq!(total, expected, "Expected {} secondary fulfillments, got {}", expected, total);
    }
}

/// Verify hour boundary alignment for summaries
pub fn verify_hour_boundaries(summaries: &[boundless_indexer::db::market::HourlyMarketSummary]) {
    for summary in summaries {
        assert_eq!(
            summary.period_timestamp % 3600,
            0,
            "Period timestamp {} should be aligned to hour boundary",
            summary.period_timestamp
        );
    }
}

/// Get hourly summaries from database
pub async fn get_hourly_summaries(
    db: &Arc<MarketDb>,
) -> Vec<boundless_indexer::db::market::HourlyMarketSummary> {
    db.get_hourly_market_summaries(None, 10000, SortDirection::Asc, None, None).await.unwrap()
}

/// Get all-time summaries from database using direct SQL query
pub async fn get_all_time_summaries(
    pool: &PgPool,
) -> Vec<boundless_indexer::db::market::AllTimeMarketSummary> {
    use boundless_indexer::db::market::AllTimeMarketSummary;
    use std::str::FromStr;

    // Helper to parse padded U256 strings
    let parse_u256 = |s: &str| -> U256 {
        let trimmed = s.trim_start_matches('0');
        let parse_str = if trimmed.is_empty() { "0" } else { trimmed };
        U256::from_str(parse_str).unwrap_or(U256::ZERO)
    };

    let rows = sqlx::query(
        "SELECT 
            period_timestamp,
            total_fulfilled,
            unique_provers_locking_requests,
            unique_requesters_submitting_requests,
            total_fees_locked,
            total_collateral_locked,
            total_locked_and_expired_collateral,
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            total_secondary_fulfillments,
            locked_orders_fulfillment_rate,
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id,
            best_peak_prove_mhz_v2,
            best_effective_prove_mhz_v2
        FROM all_time_market_summary
        ORDER BY period_timestamp ASC",
    )
    .fetch_all(pool)
    .await
    .unwrap();

    rows.into_iter()
        .map(|row| AllTimeMarketSummary {
            period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
            total_fulfilled: row.get::<i64, _>("total_fulfilled") as u64,
            unique_provers_locking_requests: row.get::<i64, _>("unique_provers_locking_requests")
                as u64,
            unique_requesters_submitting_requests: row
                .get::<i64, _>("unique_requesters_submitting_requests")
                as u64,
            total_fees_locked: parse_u256(&row.get::<String, _>("total_fees_locked")),
            total_collateral_locked: parse_u256(&row.get::<String, _>("total_collateral_locked")),
            total_locked_and_expired_collateral: parse_u256(
                &row.get::<String, _>("total_locked_and_expired_collateral"),
            ),
            total_requests_submitted: row.get::<i64, _>("total_requests_submitted") as u64,
            total_requests_submitted_onchain: row.get::<i64, _>("total_requests_submitted_onchain")
                as u64,
            total_requests_submitted_offchain: row
                .get::<i64, _>("total_requests_submitted_offchain")
                as u64,
            total_requests_locked: row.get::<i64, _>("total_requests_locked") as u64,
            total_requests_slashed: row.get::<i64, _>("total_requests_slashed") as u64,
            total_expired: row.get::<i64, _>("total_expired") as u64,
            total_locked_and_expired: row.get::<i64, _>("total_locked_and_expired") as u64,
            total_locked_and_fulfilled: row.get::<i64, _>("total_locked_and_fulfilled") as u64,
            total_secondary_fulfillments: row.get::<i64, _>("total_secondary_fulfillments") as u64,
            locked_orders_fulfillment_rate: row.get::<f64, _>("locked_orders_fulfillment_rate")
                as f32,
            total_program_cycles: parse_u256(&row.get::<String, _>("total_program_cycles")),
            total_cycles: parse_u256(&row.get::<String, _>("total_cycles")),
            best_peak_prove_mhz: row.get::<f64, _>("best_peak_prove_mhz_v2"),
            best_peak_prove_mhz_prover: row.try_get("best_peak_prove_mhz_prover").ok(),
            best_peak_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_peak_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
            best_effective_prove_mhz: row.get::<f64, _>("best_effective_prove_mhz_v2"),
            best_effective_prove_mhz_prover: row.try_get("best_effective_prove_mhz_prover").ok(),
            best_effective_prove_mhz_request_id: row
                .try_get::<Option<String>, _>("best_effective_prove_mhz_request_id")
                .ok()
                .flatten()
                .and_then(|s| U256::from_str(&s).ok()),
        })
        .collect()
}
