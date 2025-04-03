// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use alloy::providers::fillers::{
    BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
};
use alloy::providers::Identity;
use alloy::{
    primitives::{utils::parse_ether, Address, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    rpc::client::RpcClient,
    transports::layers::RetryBackoffLayer,
};
use anyhow::{Context, Error as AnyhowErr, Result};
use axum::{
    extract::Json,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use boundless_market::order_stream_client::{
    AuthMsg, ErrMsg, Order, OrderError, AUTH_GET_NONCE, HEALTH_CHECK, ORDER_LIST_PATH,
    ORDER_SUBMISSION_PATH, ORDER_WS_PATH,
};
use clap::Parser;
use reqwest::Url;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use tower_http::{limit::RequestBodyLimitLayer, timeout::TimeoutLayer, trace::TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod api;
mod order_db;
mod ws;

use api::{
    __path_find_orders_by_request_id, __path_get_nonce, __path_health, __path_list_orders,
    __path_submit_order, find_orders_by_request_id, get_nonce, health, list_orders, submit_order,
};
use order_db::OrderDb;
use ws::{__path_websocket_handler, start_broadcast_task, websocket_handler, ConnectionsMap};

/// Error type for the application
#[derive(Error, Debug)]
pub enum AppError {
    #[error("invalid order: {0}")]
    InvalidOrder(OrderError),

    #[error("invalid query parameter")]
    QueryParamErr(&'static str),

    #[error("address not found")]
    AddrNotFound(Address),

    #[error("internal error")]
    InternalErr(AnyhowErr),
}

impl AppError {
    fn type_str(&self) -> String {
        match self {
            Self::InvalidOrder(_) => "InvalidOrder",
            Self::QueryParamErr(_) => "QueryParamErr",
            Self::AddrNotFound(_) => "AddrNotFound",
            Self::InternalErr(_) => "InternalErr",
        }
        .into()
    }
}

impl From<AnyhowErr> for AppError {
    fn from(err: AnyhowErr) -> Self {
        Self::InternalErr(err)
    }
}

impl From<OrderError> for AppError {
    fn from(err: OrderError) -> Self {
        Self::InvalidOrder(err)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code = match self {
            Self::InvalidOrder(_) | Self::QueryParamErr(_) => StatusCode::BAD_REQUEST,
            Self::AddrNotFound(_) => StatusCode::NOT_FOUND,
            Self::InternalErr(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        tracing::error!("api error, code {code}: {self:?}");

        (code, Json(ErrMsg { r#type: self.type_str(), msg: self.to_string() })).into_response()
    }
}

/// Command line arguments
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
#[non_exhaustive]
pub struct Args {
    /// Bind address for REST api
    #[clap(long, env, default_value = "0.0.0.0:8585")]
    bind_addr: String,

    /// RPC URL for the Ethereum node
    #[clap(long, env, default_value = "http://localhost:8545")]
    rpc_url: Url,

    /// Address of the BoundlessMarket contract
    #[clap(long, env)]
    boundless_market_address: Address,

    /// Minimum stake balance required to connect to the WebSocket
    #[clap(long, value_parser = parse_ether)]
    min_balance: U256,

    /// Maximum number of WebSocket connections
    #[clap(long, default_value = "100")]
    max_connections: usize,

    /// Maximum size of the queue for each WebSocket connection
    #[clap(long, default_value = "100")]
    queue_size: usize,

    /// Domain for SIWE checks
    #[clap(long, default_value = "localhost:8585")]
    domain: String,

    /// List of addresses to skip balance checks when connecting them as brokers
    #[clap(long, value_delimiter = ',')]
    bypass_addrs: Vec<Address>,

    /// Time between sending websocket pings (in seconds)
    #[clap(long, default_value_t = 120)]
    ping_time: u64,

    /// RPC HTTP retry rate limit max retry
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 10)]
    pub rpc_retry_max: u32,

    /// RPC HTTP retry backoff (in ms)
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 1000)]
    pub rpc_retry_backoff: u64,

    /// RPC HTTP retry compute-unit per second
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 100)]
    pub rpc_retry_cu: u64,
}

/// Configuration struct
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Config {
    /// RPC URL for the Ethereum node
    pub rpc_url: Url,
    /// Address of the BoundlessMarket contract
    pub market_address: Address,
    /// Minimum balance required to connect to the WebSocket
    pub min_balance: U256,
    /// Maximum number of WebSocket connections
    pub max_connections: usize,
    /// Maximum size of the queue for each WebSocket connection
    pub queue_size: usize,
    /// Domain for SIWE auth checks
    pub domain: String,
    /// List of address to skip balance checks
    pub bypass_addrs: Vec<Address>,
    /// Time between sending WS Ping's (in seconds)
    pub ping_time: u64,
    /// RPC HTTP retry rate limit max retry
    pub rpc_retry_max: u32,
    /// RPC HTTP retry backoff (in ms)
    pub rpc_retry_backoff: u64,
    /// RPC HTTP retry compute-unit per second
    pub rpc_retry_cu: u64,
}

impl Config {
    /// Creates a new ConfigBuilder with default values
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

#[derive(Default)]
pub struct ConfigBuilder {
    rpc_url: Option<Url>,
    market_address: Option<Address>,
    min_balance: Option<U256>,
    max_connections: Option<usize>,
    queue_size: Option<usize>,
    domain: Option<String>,
    bypass_addrs: Option<Vec<Address>>,
    ping_time: Option<u64>,
    rpc_retry_max: Option<u32>,
    rpc_retry_backoff: Option<u64>,
    rpc_retry_cu: Option<u64>,
}

impl ConfigBuilder {
    /// Set the RPC URL
    pub fn rpc_url(self, url: Url) -> Self {
        Self { rpc_url: Some(url), ..self }
    }

    /// Set the market address
    pub fn market_address(self, address: Address) -> Self {
        Self { market_address: Some(address), ..self }
    }

    /// Set the minimum balance
    pub fn min_balance(self, balance: U256) -> Self {
        Self { min_balance: Some(balance), ..self }
    }

    /// Set the maximum number of connections
    pub fn max_connections(self, max: usize) -> Self {
        Self { max_connections: Some(max), ..self }
    }

    /// Set the queue size
    pub fn queue_size(self, size: usize) -> Self {
        Self { queue_size: Some(size), ..self }
    }

    /// Set the domain
    pub fn domain(self, domain: String) -> Self {
        Self { domain: Some(domain), ..self }
    }

    /// Set the bypass addresses
    pub fn bypass_addrs(self, addrs: Vec<Address>) -> Self {
        Self { bypass_addrs: Some(addrs), ..self }
    }

    /// Set the ping time
    pub fn ping_time(self, time: u64) -> Self {
        Self { ping_time: Some(time), ..self }
    }

    /// Set the maximum number of RPC retries
    pub fn rpc_retry_max(self, max: u32) -> Self {
        Self { rpc_retry_max: Some(max), ..self }
    }

    /// Set the RPC retry backoff time
    pub fn rpc_retry_backoff(self, backoff: u64) -> Self {
        Self { rpc_retry_backoff: Some(backoff), ..self }
    }

    /// Set the RPC retry compute units
    pub fn rpc_retry_cu(self, cu: u64) -> Self {
        Self { rpc_retry_cu: Some(cu), ..self }
    }

    /// Build the Config with default values for any unset fields
    pub fn build(self) -> Result<Config, ConfigError> {
        Ok(Config {
            rpc_url: self.rpc_url.ok_or(ConfigError::MissingRequiredField("rpc_url"))?,
            market_address: self
                .market_address
                .ok_or(ConfigError::MissingRequiredField("market_address"))?,
            min_balance: self.min_balance.unwrap_or_else(|| parse_ether("2").unwrap()),
            max_connections: self.max_connections.unwrap_or(100),
            queue_size: self.queue_size.unwrap_or(10),
            domain: self.domain.unwrap_or_else(|| "0.0.0.0:8585".to_string()),
            bypass_addrs: self.bypass_addrs.unwrap_or_default(),
            ping_time: self.ping_time.unwrap_or(60),
            rpc_retry_max: self.rpc_retry_max.unwrap_or(10),
            rpc_retry_backoff: self.rpc_retry_backoff.unwrap_or(1000),
            rpc_retry_cu: self.rpc_retry_cu.unwrap_or(100),
        })
    }
}
impl From<&Args> for Config {
    fn from(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            market_address: args.boundless_market_address,
            min_balance: args.min_balance,
            max_connections: args.max_connections,
            queue_size: args.queue_size,
            domain: args.domain.clone(),
            bypass_addrs: args.bypass_addrs.clone(),
            ping_time: args.ping_time,
            rpc_retry_max: args.rpc_retry_max,
            rpc_retry_backoff: args.rpc_retry_backoff,
            rpc_retry_cu: args.rpc_retry_cu,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required field: {0}")]
    MissingRequiredField(&'static str),
}

type WalletProvider = FillProvider<
    JoinFill<
        Identity,
        JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
    >,
    RootProvider,
>;

/// Application state struct
pub struct AppState {
    /// Database backend
    db: OrderDb,
    /// Map of WebSocket connections by address
    connections: Arc<RwLock<ConnectionsMap>>,
    /// Map of pending connections by address with their timestamp
    pending_connections: Arc<Mutex<HashMap<Address, Instant>>>,
    /// Ethereum RPC provider
    rpc_provider: WalletProvider,
    /// Configuration
    config: Config,
    /// chain_id
    chain_id: u64,
    /// Cancelation tokens set when a graceful shutdown is triggered
    shutdown: CancellationToken,
}

impl AppState {
    /// Create a new AppState
    pub async fn new(config: &Config, db_pool_opt: Option<PgPool>) -> Result<Arc<Self>> {
        // Build the RPC provider.
        let retry_layer = RetryBackoffLayer::new(
            config.rpc_retry_max,
            config.rpc_retry_backoff,
            config.rpc_retry_cu,
        );
        let client = RpcClient::builder().layer(retry_layer).http(config.rpc_url.clone());
        let rpc_provider = ProviderBuilder::new().on_client(client);

        let db = if let Some(db_pool) = db_pool_opt {
            OrderDb::from_pool(db_pool).await?
        } else {
            OrderDb::from_env().await.context("Failed to connect to DB")?
        };
        let chain_id =
            rpc_provider.get_chain_id().await.context("Failed to fetch chain_id from RPC")?;

        Ok(Arc::new(Self {
            db,
            connections: Arc::new(RwLock::new(HashMap::new())),
            pending_connections: Arc::new(Mutex::new(HashMap::new())),
            rpc_provider,
            config: config.clone(),
            chain_id,
            shutdown: CancellationToken::new(),
        }))
    }

    /// Pending connection timeout on failed upgrade.
    const PENDING_CONNECTION_TIMEOUT: Duration = Duration::from_secs(10);

    /// Set a pending connection and return true if the connection is not already pending
    /// or if the existing pending connection has timed out.
    pub(crate) async fn set_pending_connection(&self, addr: Address) -> bool {
        let mut pending_connections = self.pending_connections.lock().await;
        let now = Instant::now();

        match pending_connections.entry(addr) {
            Entry::Occupied(mut entry) => {
                if now.duration_since(*entry.get()) < Self::PENDING_CONNECTION_TIMEOUT {
                    // Connection is still pending and within timeout
                    false
                } else {
                    // Connection has timed out, update the timestamp
                    entry.insert(now);
                    true
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(now);
                true
            }
        }
    }

    /// Remove a pending connection for a given address.
    pub(crate) async fn remove_pending_connection(&self, addr: &Address) {
        let mut pending_connections = self.pending_connections.lock().await;
        pending_connections.remove(addr);
    }

    /// Removes connection for a given address.
    pub(crate) async fn remove_connection(&self, addr: &Address) {
        let mut connections = self.connections.write().await;
        connections.remove(addr);
    }
}

const MAX_ORDER_SIZE: usize = 25 * 1024 * 1024; // 25 mb

#[derive(OpenApi, Debug, Deserialize)]
#[openapi(
    paths(
        submit_order,
        list_orders,
        find_orders_by_request_id,
        get_nonce,
        health,
        websocket_handler
    ),
    components(schemas(AuthMsg)),
    info(
        title = "Boundless Order Stream service",
        description = r#"
Service for offchain order submission and fetching
            "#,
        version = "0.0.1",
    )
)]
struct ApiDoc;

/// Create the application router
pub fn app(state: Arc<AppState>) -> Router {
    let body_size_limit = RequestBodyLimitLayer::new(MAX_ORDER_SIZE);

    Router::new()
        .route(ORDER_SUBMISSION_PATH, post(submit_order).layer(body_size_limit))
        .route(ORDER_LIST_PATH, get(list_orders))
        .route(&format!("{ORDER_LIST_PATH}/:request_id"), get(find_orders_by_request_id))
        .route(&format!("{AUTH_GET_NONCE}:addr"), get(get_nonce))
        .route(ORDER_WS_PATH, get(websocket_handler))
        .route(HEALTH_CHECK, get(health))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .with_state(state)
        .layer((
            TraceLayer::new_for_http(),
            TimeoutLayer::new(tokio::time::Duration::from_secs(10)),
        ))
}

/// Run the REST API service
pub async fn run(args: &Args) -> Result<()> {
    let config: Config = args.into();

    let app_state = AppState::new(&config, None).await?;
    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .context("Failed to bind a TCP listener")?;
    run_from_parts(app_state, listener).await
}

/// Run the REST API service from parts
pub async fn run_from_parts(
    app_state: Arc<AppState>,
    listener: tokio::net::TcpListener,
) -> Result<()> {
    let app_state_clone = app_state.clone();
    tokio::spawn(async move {
        loop {
            let order_stream = app_state_clone.db.order_stream().await.unwrap();
            let broadcast_task = start_broadcast_task(app_state_clone.clone(), order_stream);

            match broadcast_task.await {
                Ok(_) => {
                    tracing::info!("Broadcast task completed successfully");
                    break;
                }
                Err(e) => {
                    tracing::warn!("Broadcast task failed with error: {}. Respawning...", e);
                }
            }
        }
    });

    tracing::info!("REST API listening on: {}", listener.local_addr().unwrap());
    axum::serve(listener, self::app(app_state.clone()))
        .with_graceful_shutdown(async { shutdown_signal(app_state).await })
        .await
        .context("REST API service failed")?;

    Ok(())
}

async fn shutdown_signal(state: Arc<AppState>) {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Triggering shutdown");
    state.shutdown.cancel();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_db::{DbOrder, OrderDbErr};
    use alloy::{
        node_bindings::{Anvil, AnvilInstance},
        primitives::U256,
        providers::{Provider, WalletProvider},
    };
    use boundless_market::{
        contracts::{
            hit_points::default_allowance,
            test_utils::{create_test_ctx, TestCtx},
            Offer, Predicate, ProofRequest, RequestId, Requirements,
        },
        input::InputBuilder,
        order_stream_client::{order_stream, Client},
    };
    use futures_util::StreamExt;
    use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
    use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
    use reqwest::Url;
    use risc0_zkvm::sha::Digest;
    use sqlx::PgPool;
    use std::net::{Ipv4Addr, SocketAddr};
    use tokio::task::JoinHandle;

    /// Test setup helper that creates common test infrastructure
    async fn setup_test_env(
        pool: PgPool,
        ping_time: u64,
        listener: Option<&tokio::net::TcpListener>, // Optional listener for domain configuration
    ) -> (Arc<AppState>, TestCtx<impl Provider + WalletProvider + Clone + 'static>, AnvilInstance)
    {
        let anvil = Anvil::new().spawn();
        let rpc_url = anvil.endpoint_url();

        let ctx = create_test_ctx(
            &anvil,
            SET_BUILDER_ID,
            format!("file://{SET_BUILDER_PATH}"),
            ASSESSOR_GUEST_ID,
            format!("file://{ASSESSOR_GUEST_PATH}"),
        )
        .await
        .unwrap();

        ctx.prover_market
            .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        // Set domain based on listener if provided
        let domain = if let Some(l) = listener {
            l.local_addr().unwrap().to_string()
        } else {
            "0.0.0.0:8585".to_string()
        };

        let config = Config {
            rpc_url,
            market_address: *ctx.prover_market.instance().address(),
            min_balance: parse_ether("2").unwrap(),
            max_connections: 2,
            queue_size: 10,
            domain,
            bypass_addrs: vec![ctx.prover_signer.address(), ctx.customer_signer.address()],
            ping_time,
            rpc_retry_max: 10,
            rpc_retry_backoff: 1000,
            rpc_retry_cu: 100,
        };

        let app_state = AppState::new(&config, Some(pool)).await.unwrap();

        (app_state, ctx, anvil)
    }

    fn new_request(idx: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, idx),
            Requirements::new(Digest::from_bytes([1; 32]), Predicate::prefix_match([])),
            "http://image_uri.null",
            InputBuilder::new().build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: 1,
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockStake: U256::from(10),
            },
        )
    }

    /// Helper to wait for server health with exponential backoff
    async fn wait_for_server_health(client: &Client, addr: &SocketAddr, max_retries: usize) {
        let mut retry_delay = tokio::time::Duration::from_millis(50);

        let health_url = format!("http://{}{}", addr, HEALTH_CHECK);
        for attempt in 1..=max_retries {
            match client.client.get(&health_url).send().await {
                Ok(response) if response.status().is_success() => {
                    tracing::info!("Server is healthy after {} attempts", attempt);
                    return;
                }
                _ => {
                    if attempt == max_retries {
                        panic!("Server failed to become healthy after {} attempts", max_retries);
                    }
                    println!(
                        "Waiting for server to become healthy (attempt {}/{})",
                        attempt, max_retries
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay =
                        std::cmp::min(retry_delay * 2, tokio::time::Duration::from_secs(10));
                }
            }
        }
    }

    #[sqlx::test]
    async fn integration_test(pool: PgPool) {
        // Set the ping interval to 500ms for this test
        std::env::set_var("ORDER_STREAM_CLIENT_PING_MS", "500");

        // Create listener first
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        // Setup with the prover address in bypass list and 1 second ping time
        let (app_state, ctx, _anvil) = setup_test_env(pool, 1, Some(&listener)).await;

        // Create client
        let client = Client::new(
            Url::parse(&format!("http://{addr}")).unwrap(),
            app_state.config.market_address,
            app_state.chain_id,
        );

        // Start server
        let app_state_clone = app_state.clone();
        let server_handle = tokio::spawn(async move {
            self::run_from_parts(app_state_clone, listener).await.unwrap();
        });

        // Poll the health endpoint with exponential backoff
        wait_for_server_health(&client, &addr, 5).await;

        // Create channels to communicate with the order stream task
        let (order_tx, mut order_rx) = tokio::sync::mpsc::channel(1);

        // Connect to the WebSocket and start listening in a separate task
        let socket = client.connect_async(&ctx.prover_signer).await.unwrap();

        // Connect customer signer as well
        let customer_client = Client::new(
            Url::parse(&format!("http://{addr}")).unwrap(),
            app_state.config.market_address,
            app_state.chain_id,
        );
        let customer_socket = customer_client.connect_async(&ctx.customer_signer).await.unwrap();
        let stream_task = tokio::spawn(async move {
            let mut stream = order_stream(socket);
            let mut customer_order_stream = order_stream(customer_socket);

            loop {
                // Wait for either order to come through
                let (res1, res2) = tokio::join!(stream.next(), customer_order_stream.next());

                // Handle potential errors from both streams
                match (res1, res2) {
                    (Some(Ok(order1)), Some(Ok(order2))) => {
                        if order1.order == order2.order {
                            order_tx.send(order1).await.unwrap();
                        } else {
                            panic!("Orders don't match: {:?} vs {:?}", order1.order, order2.order);
                        }
                    }

                    (None, None) => {
                        // Handle the case on shutdown where both will be closed.
                        break;
                    }
                    (_, _) => {
                        panic!("Unexpected error in order stream clients");
                    }
                }
            }
        });

        let app_state_clone = app_state.clone();
        let watch_task: JoinHandle<Result<DbOrder, OrderDbErr>> = tokio::spawn(async move {
            let mut new_orders = app_state_clone.db.order_stream().await.unwrap();
            let order = new_orders.next().await.unwrap().unwrap();
            Ok(order)
        });

        // Submit an order to ensure the connection is working
        let order = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let db_order = watch_task.await.unwrap().unwrap();

        // Wait for the order to be received
        let order_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(4), order_rx.recv()).await;

        match order_result {
            Ok(Some(received_order)) => {
                assert_eq!(
                    received_order.order, order,
                    "Received order should match submitted order"
                );
                assert_eq!(order, db_order.order);
            }
            Ok(None) => {
                panic!("Order channel closed unexpectedly");
            }
            Err(_) => {
                panic!("Timed out waiting for order");
            }
        }

        // Wait a bit to ensure ping-pong is working (no errors)
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        // Verify the connections are in the connections map
        {
            let connections = app_state.connections.read().await;
            assert!(
                connections.contains_key(&ctx.prover_signer.address()),
                "Connection should still be active after ping-pong exchanges"
            );
            assert!(
                connections.contains_key(&ctx.customer_signer.address()),
                "Customer connection should also be active"
            );
        }

        // Now simulate server disconnection by aborting the server task
        app_state.shutdown.cancel();

        // Ensure that the client streams have been closed.
        tokio::time::timeout(tokio::time::Duration::from_secs(10), stream_task)
            .await
            .unwrap()
            .unwrap();

        // Clean up
        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_pending_connection_timeout(pool: PgPool) {
        // No need for a listener in this test
        let (app_state, ctx, _anvil) = setup_test_env(pool, 20, None).await;
        let addr = ctx.prover_signer.address();

        // Test case 1: New connection (vacant entry)
        let pending_connection = app_state.set_pending_connection(addr).await;
        assert!(pending_connection, "Should return true for a new connection");

        // Test case 2: Existing connection within timeout (occupied entry, not timed out)
        let pending_connection = app_state.set_pending_connection(addr).await;
        assert!(!pending_connection, "Should return false for a connection within timeout");

        // Test case 3: Existing connection that has timed out
        // Manually set the timestamp to be older than the timeout
        {
            let mut pending_connections = app_state.pending_connections.lock().await;
            let old_time =
                Instant::now() - (AppState::PENDING_CONNECTION_TIMEOUT + Duration::from_secs(1));
            pending_connections.insert(addr, old_time);
        }

        // Now it should allow a new connection since the old one timed out
        let pending_connection = app_state.set_pending_connection(addr).await;
        assert!(pending_connection, "Should return true for a timed out connection");

        // Newly set connection should result in pending_connection == false
        let pending_connection = app_state.set_pending_connection(addr).await;
        assert!(
            !pending_connection,
            "Should return false for a replaced connection within timeout"
        );

        // Test removing a pending connection
        app_state.remove_pending_connection(&addr).await;
        let pending_connection = app_state.set_pending_connection(addr).await;
        assert!(pending_connection, "Should return true after removing the connection");
    }
}
