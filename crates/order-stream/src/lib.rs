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

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use alloy::{
    primitives::{utils::parse_ether, Address, U256},
    providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
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
    ORDER_LIST_PATH_V2, ORDER_SUBMISSION_PATH, ORDER_WS_PATH,
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
mod heartbeat;
mod order_db;
mod ws;

use api::{
    __path_find_orders_by_request_id, __path_get_nonce, __path_health, __path_list_orders,
    __path_list_orders_v2, __path_submit_order, find_orders_by_request_id, get_nonce, health,
    list_orders, list_orders_v2, submit_order,
};
use heartbeat::{
    submit_heartbeat, submit_request_heartbeat, BalanceCache, KinesisForwarder, NoopForwarder,
    TelemetryForwarder,
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

    /// Minimum stake balance, in raw units, required to connect to the WebSocket
    #[clap(long)]
    min_balance_raw: U256,

    /// Maximum number of WebSocket connections
    #[clap(long, default_value = "1000")]
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

    /// Kinesis stream name for broker identity heartbeats. If unset, Kinesis forwarding is disabled.
    /// When set, all three Kinesis stream names must be provided.
    #[clap(long, env, requires_all = ["kinesis_evaluations_stream", "kinesis_completions_stream"])]
    pub kinesis_heartbeat_stream: Option<String>,

    /// Kinesis stream name for request evaluation events.
    #[clap(long, env, requires_all = ["kinesis_heartbeat_stream", "kinesis_completions_stream"])]
    pub kinesis_evaluations_stream: Option<String>,

    /// Kinesis stream name for request completion events.
    #[clap(long, env, requires_all = ["kinesis_heartbeat_stream", "kinesis_evaluations_stream"])]
    pub kinesis_completions_stream: Option<String>,
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
    /// Kinesis stream name for broker identity heartbeats
    pub kinesis_heartbeat_stream: String,
    /// Kinesis stream name for request evaluation events
    pub kinesis_evaluations_stream: String,
    /// Kinesis stream name for request completion events
    pub kinesis_completions_stream: String,
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
    kinesis_heartbeat_stream: Option<String>,
    kinesis_evaluations_stream: Option<String>,
    kinesis_completions_stream: Option<String>,
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

    /// Set the Kinesis heartbeat stream name
    pub fn kinesis_heartbeat_stream(self, name: String) -> Self {
        Self { kinesis_heartbeat_stream: Some(name), ..self }
    }

    /// Set the Kinesis evaluations stream name
    pub fn kinesis_evaluations_stream(self, name: String) -> Self {
        Self { kinesis_evaluations_stream: Some(name), ..self }
    }

    /// Set the Kinesis completions stream name
    pub fn kinesis_completions_stream(self, name: String) -> Self {
        Self { kinesis_completions_stream: Some(name), ..self }
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
            kinesis_heartbeat_stream: self.kinesis_heartbeat_stream.unwrap_or_default(),
            kinesis_evaluations_stream: self.kinesis_evaluations_stream.unwrap_or_default(),
            kinesis_completions_stream: self.kinesis_completions_stream.unwrap_or_default(),
        })
    }
}
impl From<&Args> for Config {
    fn from(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            market_address: args.boundless_market_address,
            min_balance: args.min_balance_raw,
            max_connections: args.max_connections,
            queue_size: args.queue_size,
            domain: args.domain.clone(),
            bypass_addrs: args.bypass_addrs.clone(),
            ping_time: args.ping_time,
            rpc_retry_max: args.rpc_retry_max,
            rpc_retry_backoff: args.rpc_retry_backoff,
            rpc_retry_cu: args.rpc_retry_cu,
            kinesis_heartbeat_stream: args.kinesis_heartbeat_stream.clone().unwrap_or_default(),
            kinesis_evaluations_stream: args.kinesis_evaluations_stream.clone().unwrap_or_default(),
            kinesis_completions_stream: args.kinesis_completions_stream.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required field: {0}")]
    MissingRequiredField(&'static str),
}

type ReadOnlyProvider = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

/// Application state struct
pub struct AppState {
    /// Database backend
    db: OrderDb,
    /// Map of WebSocket connections by address
    connections: Arc<RwLock<ConnectionsMap>>,
    /// Map of pending connections by address with their timestamp
    /// Used to track connections that are in the process of being upgraded, and means only 1 reconnnect
    /// can happen every 10s.
    pending_connections: Arc<Mutex<HashMap<Address, Instant>>>,
    /// Ethereum RPC provider
    rpc_provider: ReadOnlyProvider,
    /// Configuration
    config: Config,
    /// chain_id
    chain_id: u64,
    /// Cancelation tokens set when a graceful shutdown is triggered
    shutdown: CancellationToken,
    /// TTL cache for collateral balance checks (used by heartbeat auth)
    balance_cache: BalanceCache,
    /// Telemetry forwarding backend
    telemetry: Arc<dyn TelemetryForwarder>,
    /// Signals that the broadcast task's PgListener is connected and the task is actively
    /// processing order notifications. The health check endpoint gates on this.
    broadcast_ready: AtomicBool,
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
        let rpc_provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_client(client);

        let db = if let Some(db_pool) = db_pool_opt {
            OrderDb::from_pool(db_pool).await.context("Failed to apply DB migrations")?
        } else {
            OrderDb::from_env().await.context("Failed to connect to DB")?
        };
        let chain_id =
            rpc_provider.get_chain_id().await.context("Failed to fetch chain_id from RPC")?;

        let telemetry: Arc<dyn TelemetryForwarder> = if !config.kinesis_heartbeat_stream.is_empty()
        {
            let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
            let kinesis = aws_sdk_kinesis::Client::new(&aws_config);
            Arc::new(KinesisForwarder::new(
                kinesis,
                config.kinesis_heartbeat_stream.clone(),
                config.kinesis_evaluations_stream.clone(),
                config.kinesis_completions_stream.clone(),
            ))
        } else {
            Arc::new(NoopForwarder)
        };

        Ok(Arc::new(Self {
            db,
            connections: Arc::new(RwLock::new(HashMap::new())),
            pending_connections: Arc::new(Mutex::new(HashMap::new())),
            rpc_provider,
            config: config.clone(),
            chain_id,
            shutdown: CancellationToken::new(),
            balance_cache: heartbeat::new_balance_cache(),
            telemetry,
            broadcast_ready: AtomicBool::new(false),
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

const MAX_ORDER_SIZE: usize = 100 * 1024; // 100 KiB
const BROADCAST_TASK_RESPAWN_DELAY: Duration = Duration::from_secs(1);

#[derive(OpenApi, Debug, Deserialize)]
#[openapi(
    paths(
        submit_order,
        list_orders,
        list_orders_v2,
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

/// Maximum size for heartbeat payloads (1 MiB)
const MAX_HEARTBEAT_SIZE: usize = 1024 * 1024;

/// Create the application router
pub fn app(state: Arc<AppState>) -> Router {
    use boundless_market::telemetry::{HEARTBEAT_PATH, HEARTBEAT_REQUESTS_PATH};

    let body_size_limit = RequestBodyLimitLayer::new(MAX_ORDER_SIZE);
    let heartbeat_size_limit = RequestBodyLimitLayer::new(MAX_HEARTBEAT_SIZE);

    Router::new()
        .route(ORDER_SUBMISSION_PATH, post(submit_order).layer(body_size_limit))
        .route(ORDER_LIST_PATH, get(list_orders))
        .route(ORDER_LIST_PATH_V2, get(list_orders_v2))
        .route(&format!("{ORDER_LIST_PATH}/{{request_id}}"), get(find_orders_by_request_id))
        .route(&format!("{AUTH_GET_NONCE}{{addr}}"), get(get_nonce))
        .route(ORDER_WS_PATH, get(websocket_handler))
        .route(HEALTH_CHECK, get(health))
        .route(HEARTBEAT_PATH, post(submit_heartbeat).layer(heartbeat_size_limit))
        .route(HEARTBEAT_REQUESTS_PATH, post(submit_request_heartbeat).layer(heartbeat_size_limit))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .with_state(state)
        .layer((
            TraceLayer::new_for_http(),
            TimeoutLayer::new(tokio::time::Duration::from_secs(30)),
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
            // Mark as not ready during (re)initialization so the health endpoint
            // reflects that the broadcast pipeline is down.
            app_state_clone.broadcast_ready.store(false, Ordering::Release);

            let order_stream = match app_state_clone.db.order_stream().await {
                Ok(stream) => stream,
                Err(e) => {
                    tracing::warn!("Failed to create order stream: {e}. Respawning...");
                    tokio::time::sleep(BROADCAST_TASK_RESPAWN_DELAY).await;
                    continue;
                }
            };

            let broadcast_task = start_broadcast_task(app_state_clone.clone(), order_stream);

            // Signal readiness only after the PgListener is subscribed (via order_stream)
            // and the broadcast task is spawned. This ensures the health endpoint won't
            // return 200 until we can actually deliver order notifications to WebSocket clients.
            app_state_clone.broadcast_ready.store(true, Ordering::Release);
            tracing::info!("Broadcast task PgListener connected and ready");

            match broadcast_task.await {
                Ok(Ok(())) => {
                    tracing::warn!("Broadcast task stream ended unexpectedly. Respawning...");
                    tokio::time::sleep(BROADCAST_TASK_RESPAWN_DELAY).await;
                }
                Ok(Err(e)) => {
                    tracing::warn!("Broadcast task failed: {e}. Respawning...");
                    tokio::time::sleep(BROADCAST_TASK_RESPAWN_DELAY).await;
                }
                Err(e) => {
                    tracing::error!("Broadcast task panicked: {e}. Respawning...");
                    tokio::time::sleep(BROADCAST_TASK_RESPAWN_DELAY).await;
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
            hit_points::default_allowance, Offer, Predicate, ProofRequest, RequestId, Requirements,
        },
        input::GuestEnv,
        order_stream_client::{order_stream, OrderData, OrderStreamClient},
    };
    use boundless_test_utils::market::{create_test_ctx, TestCtx};
    use sqlx::types::chrono::Utc;

    use futures_util::StreamExt;
    use reqwest::Url;
    use risc0_zkvm::sha::Digest;
    use serial_test::serial;
    use sqlx::PgPool;
    use std::{
        net::{Ipv4Addr, SocketAddr},
        pin::Pin,
    };
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

        let ctx = create_test_ctx(&anvil).await.unwrap();

        ctx.prover_market
            .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
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
            kinesis_heartbeat_stream: String::new(),
            kinesis_evaluations_stream: String::new(),
            kinesis_completions_stream: String::new(),
        };

        let app_state = AppState::new(&config, Some(pool)).await.unwrap();

        (app_state, ctx, anvil)
    }

    fn new_request(idx: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, idx),
            Requirements::new(Predicate::prefix_match(Digest::from_bytes([1; 32]), [])),
            "http://image_uri.null",
            GuestEnv::builder().build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                rampUpStart: 1,
                timeout: 100,
                lockTimeout: 100,
                rampUpPeriod: 1,
                lockCollateral: U256::from(10),
            },
        )
    }

    /// Helper to wait for server health with exponential backoff
    async fn wait_for_server_health(
        client: &OrderStreamClient,
        addr: &SocketAddr,
        max_retries: usize,
    ) {
        let mut retry_delay = tokio::time::Duration::from_millis(50);

        let health_url = format!("http://{addr}{HEALTH_CHECK}");
        for attempt in 1..=max_retries {
            match client.client.get(&health_url).send().await {
                Ok(response) if response.status().is_success() => {
                    tracing::info!("Server is healthy after {} attempts", attempt);
                    return;
                }
                _ => {
                    if attempt == max_retries {
                        panic!("Server failed to become healthy after {max_retries} attempts");
                    }
                    println!(
                        "Waiting for server to become healthy (attempt {attempt}/{max_retries})"
                    );
                    tokio::time::sleep(retry_delay).await;
                    retry_delay =
                        std::cmp::min(retry_delay * 2, tokio::time::Duration::from_secs(10));
                }
            }
        }
    }

    // The server registers WebSocket connections in the on_upgrade callback, which runs
    // asynchronously after connect_async returns on the client side. Poll until the
    // connection appears in the map so broadcasts won't miss a client.
    async fn wait_for_connection(app_state: &Arc<AppState>, addr: &Address) {
        for _ in 0..100 {
            if app_state.connections.read().await.contains_key(addr) {
                return;
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }
        panic!("Connection for {addr} not registered within 5s");
    }

    fn spawn_order_observer(
        label: &'static str,
        mut stream: Pin<Box<dyn futures_util::Stream<Item = OrderData> + Send>>,
    ) -> (tokio::sync::oneshot::Receiver<OrderData>, JoinHandle<std::result::Result<(), String>>)
    {
        let (order_tx, order_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let mut order_tx = Some(order_tx);

            while let Some(order) = stream.next().await {
                if let Some(first_order_tx) = order_tx.take() {
                    first_order_tx.send(order).map_err(|_| {
                        format!("{label} observer receiver dropped before the first order")
                    })?;
                }
            }

            if order_tx.is_some() {
                Err(format!("{label} websocket stream closed before receiving an order"))
            } else {
                Ok(())
            }
        });

        (order_rx, task)
    }

    /// Comprehensive test helper that sets up server, client, and test environment
    async fn setup_server_and_client(
        pool: PgPool,
    ) -> (
        OrderStreamClient,
        Arc<AppState>,
        TestCtx<impl Provider + WalletProvider + Clone + 'static>,
        AnvilInstance,
        JoinHandle<Result<()>>,
        SocketAddr,
    ) {
        // Create listener
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        // Setup test environment with 20 second ping time
        let (app_state, ctx, anvil) = setup_test_env(pool, 20, Some(&listener)).await;

        // Create client
        let client = OrderStreamClient::new(
            Url::parse(&format!("http://{addr}")).unwrap(),
            app_state.config.market_address,
            app_state.chain_id,
        );

        // Spawn server (clone app_state so we can return it)
        let app_state_clone = app_state.clone();
        let server_handle =
            tokio::spawn(async move { self::run_from_parts(app_state_clone, listener).await });

        // Wait for health
        wait_for_server_health(&client, &addr, 5).await;

        (client, app_state, ctx, anvil, server_handle, addr)
    }

    #[sqlx::test]
    #[serial]
    async fn integration_test(pool: PgPool) {
        // Create listener first
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();

        // ws.rs drops the connection when a ping is not answered before the next interval tick.
        // tokio::time::interval completes its first tick immediately, so a 1s period allows only ~1s
        // for the client to answer the first ping; use a margin so the test is not scheduler-sensitive.
        let (app_state, ctx, _anvil) = setup_test_env(pool, 5, Some(&listener)).await;

        // Create client
        let client = OrderStreamClient::new(
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

        // Connect to the WebSocket and wait for server-side registration
        let socket = client.connect_async(&ctx.prover_signer).await.unwrap();
        wait_for_connection(&app_state, &ctx.prover_signer.address()).await;

        let customer_client = OrderStreamClient::new(
            Url::parse(&format!("http://{addr}")).unwrap(),
            app_state.config.market_address,
            app_state.chain_id,
        );
        let customer_socket = customer_client.connect_async(&ctx.customer_signer).await.unwrap();
        wait_for_connection(&app_state, &ctx.customer_signer.address()).await;

        let (prover_order_rx, prover_stream_task) =
            spawn_order_observer("prover", order_stream(socket));
        let (customer_order_rx, customer_stream_task) =
            spawn_order_observer("customer", order_stream(customer_socket));

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

        let prover_order_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(15), prover_order_rx).await;
        let customer_order_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(15), customer_order_rx).await;

        let prover_order = match prover_order_result {
            Ok(Ok(received_order)) => received_order,
            Ok(Err(_)) => match prover_stream_task.await.unwrap() {
                Ok(()) => panic!("Prover observer ended before reporting the first order"),
                Err(err) => panic!("Prover observer failed before receiving an order: {err}"),
            },
            Err(_) => panic!("Timed out waiting for order on prover connection"),
        };
        let customer_order = match customer_order_result {
            Ok(Ok(received_order)) => received_order,
            Ok(Err(_)) => match customer_stream_task.await.unwrap() {
                Ok(()) => panic!("Customer observer ended before reporting the first order"),
                Err(err) => panic!("Customer observer failed before receiving an order: {err}"),
            },
            Err(_) => panic!("Timed out waiting for order on customer connection"),
        };

        assert_eq!(
            prover_order.order, order,
            "Received order should match submitted order on prover connection"
        );
        assert_eq!(
            customer_order.order, order,
            "Received order should match submitted order on customer connection"
        );
        assert_eq!(
            prover_order.order, customer_order.order,
            "Both websocket clients should receive the same order"
        );
        assert_eq!(order, db_order.order);

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

        let prover_shutdown_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(10), prover_stream_task).await;
        match prover_shutdown_result {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => panic!("Prover observer failed during shutdown: {err}"),
            Ok(Err(err)) => panic!("Prover observer panicked during shutdown: {err}"),
            Err(_) => panic!("Timed out waiting for prover observer to shut down"),
        }
        let customer_shutdown_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(10), customer_stream_task).await;
        match customer_shutdown_result {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(err))) => panic!("Customer observer failed during shutdown: {err}"),
            Ok(Err(err)) => panic!("Customer observer panicked during shutdown: {err}"),
            Err(_) => panic!("Timed out waiting for customer observer to shut down"),
        }

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

    #[sqlx::test]
    #[serial]
    async fn test_websocket_connection_replacement(pool: PgPool) {
        // Set up server and client
        let (client, app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        // Connect first websocket connection
        let first_socket = client.connect_async(&ctx.prover_signer).await.unwrap();
        let mut first_stream = order_stream(first_socket);

        // Verify first connection is active in connections map
        {
            let connections = app_state.connections.read().await;
            assert!(
                connections.contains_key(&ctx.prover_signer.address()),
                "First connection should be active"
            );
        }

        // Create channels to track messages on both connections
        let (first_order_tx, mut first_order_rx) = tokio::sync::mpsc::channel(1);
        let (second_order_tx, mut second_order_rx) = tokio::sync::mpsc::channel(1);

        // Spawn task to listen on first connection
        let first_stream_task = tokio::spawn(async move {
            while let Some(order) = first_stream.next().await {
                // Send order through channel, or break if channel is closed
                if first_order_tx.send(order).await.is_err() {
                    break;
                }
            }
        });

        // Wait a bit to ensure first connection is fully established
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Connect second websocket connection for the same address
        let second_socket = client.connect_async(&ctx.prover_signer).await.unwrap();
        let mut second_stream = order_stream(second_socket);

        // Wait a bit for the old connection to be closed
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;

        // Verify new connection is active in connections map
        {
            let connections = app_state.connections.read().await;
            assert!(
                connections.contains_key(&ctx.prover_signer.address()),
                "New connection should be active"
            );
            assert_eq!(connections.len(), 1, "Should only have one connection for this address");
        }

        // Spawn task to listen on second connection
        let second_stream_task = tokio::spawn(async move {
            while let Some(order) = second_stream.next().await {
                // Send order through channel, or break if channel is closed
                if second_order_tx.send(order).await.is_err() {
                    break;
                }
            }
        });

        // Submit an order and verify it's received on the new connection (not the old one)
        let app_state_clone = app_state.clone();
        let watch_task: JoinHandle<Result<DbOrder, OrderDbErr>> = tokio::spawn(async move {
            let mut new_orders = app_state_clone.db.order_stream().await.unwrap();
            let order = new_orders.next().await.unwrap().unwrap();
            Ok(order)
        });

        let order = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let db_order = watch_task.await.unwrap().unwrap();

        // Wait for the order to be received on the new connection
        let order_result =
            tokio::time::timeout(tokio::time::Duration::from_secs(4), second_order_rx.recv()).await;

        match order_result {
            Ok(Some(received_order)) => {
                assert_eq!(
                    received_order.order, order,
                    "Received order should match submitted order on new connection"
                );
                assert_eq!(order, db_order.order);
            }
            Ok(None) => {
                panic!("Order channel closed unexpectedly on new connection");
            }
            Err(_) => {
                panic!("Timed out waiting for order on new connection");
            }
        }

        // Verify old connection did not receive the order
        // The old connection should be closed, so it shouldn't receive any messages
        let old_order_result =
            tokio::time::timeout(tokio::time::Duration::from_millis(500), first_order_rx.recv())
                .await;
        assert!(
            old_order_result.is_err() || matches!(old_order_result, Ok(None)),
            "Old connection should not receive the order"
        );

        // Clean up - abort tasks directly
        first_stream_task.abort();
        second_stream_task.abort();
        app_state.shutdown.cancel();
        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_list_orders_with_sort(pool: PgPool) {
        let (client, _app_state, ctx, _anvil, server_handle, addr) =
            setup_server_and_client(pool).await;

        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order3 = client
            .submit_request(&new_request(3, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let url_asc = format!("http://{addr}{ORDER_LIST_PATH}?offset=0&limit=10");
        let response_asc = client.client.get(&url_asc).send().await.unwrap();
        assert!(response_asc.status().is_success());
        let orders_asc: Vec<OrderData> = response_asc.json().await.unwrap();
        assert_eq!(orders_asc.len(), 3);
        assert_eq!(orders_asc[0].order.request.id, order1.request.id);
        assert_eq!(orders_asc[1].order.request.id, order2.request.id);
        assert_eq!(orders_asc[2].order.request.id, order3.request.id);

        let url_desc = format!("http://{addr}{ORDER_LIST_PATH}?sort=desc&limit=10");
        let response_desc = client.client.get(&url_desc).send().await.unwrap();
        assert!(response_desc.status().is_success());
        let orders_desc: Vec<OrderData> = response_desc.json().await.unwrap();
        assert_eq!(orders_desc.len(), 3);
        assert_eq!(orders_desc[0].order.request.id, order3.request.id);
        assert_eq!(orders_desc[1].order.request.id, order2.request.id);
        assert_eq!(orders_desc[2].order.request.id, order1.request.id);

        let after_time = orders_desc[1].created_at.to_rfc3339();
        let url_after = format!(
            "http://{addr}{ORDER_LIST_PATH}?sort=desc&after={}&limit=10",
            urlencoding::encode(&after_time)
        );
        let response_after = client.client.get(&url_after).send().await.unwrap();
        assert!(response_after.status().is_success());
        let orders_after: Vec<OrderData> = response_after.json().await.unwrap();
        assert_eq!(orders_after.len(), 1);
        assert_eq!(orders_after[0].order.request.id, order3.request.id);

        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_list_orders_v2_cursor_pagination(pool: PgPool) {
        use boundless_market::order_stream_client::SortDirection;

        let (client, _app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order3 = client
            .submit_request(&new_request(3, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let first_page = client
            .list_orders_v2(None, Some(2), Some(SortDirection::Desc), None, None)
            .await
            .unwrap();
        assert_eq!(first_page.orders.len(), 2);
        assert_eq!(first_page.orders[0].order.request.id, order3.request.id);
        assert_eq!(first_page.orders[1].order.request.id, order2.request.id);
        assert!(first_page.orders[0].created_at > first_page.orders[1].created_at);
        assert!(first_page.has_more);
        assert!(first_page.next_cursor.is_some());

        let second_page = client
            .list_orders_v2(first_page.next_cursor, Some(2), Some(SortDirection::Desc), None, None)
            .await
            .unwrap();
        assert_eq!(second_page.orders.len(), 1);
        assert_eq!(second_page.orders[0].order.request.id, order1.request.id);
        assert!(!second_page.has_more);
        assert!(second_page.next_cursor.is_none());

        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_list_orders_v2_timestamp_filters(pool: PgPool) {
        use boundless_market::order_stream_client::SortDirection;

        let (client, _app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let middle_time = Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let after_orders = client
            .list_orders_v2(None, Some(10), Some(SortDirection::Desc), None, Some(middle_time))
            .await
            .unwrap();
        assert_eq!(after_orders.orders.len(), 1);
        assert_eq!(after_orders.orders[0].order.request.id, order2.request.id);

        let before_orders = client
            .list_orders_v2(None, Some(10), Some(SortDirection::Desc), Some(middle_time), None)
            .await
            .unwrap();
        assert_eq!(before_orders.orders.len(), 1);
        assert_eq!(before_orders.orders[0].order.request.id, order1.request.id);

        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_list_orders_v2_bidirectional_sort(pool: PgPool) {
        use boundless_market::order_stream_client::SortDirection;

        let (client, _app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order3 = client
            .submit_request(&new_request(3, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();

        let orders_asc = client
            .list_orders_v2(None, Some(10), Some(SortDirection::Asc), None, None)
            .await
            .unwrap();
        assert_eq!(orders_asc.orders.len(), 3);
        assert_eq!(orders_asc.orders[0].order.request.id, order1.request.id);
        assert_eq!(orders_asc.orders[1].order.request.id, order2.request.id);
        assert_eq!(orders_asc.orders[2].order.request.id, order3.request.id);

        let orders_desc = client
            .list_orders_v2(None, Some(10), Some(SortDirection::Desc), None, None)
            .await
            .unwrap();
        assert_eq!(orders_desc.orders.len(), 3);
        assert_eq!(orders_desc.orders[0].order.request.id, order3.request.id);
        assert_eq!(orders_desc.orders[1].order.request.id, order2.request.id);
        assert_eq!(orders_desc.orders[2].order.request.id, order1.request.id);

        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_cursor_isolation_with_concurrent_inserts_desc(pool: PgPool) {
        use boundless_market::order_stream_client::SortDirection;

        let (client, _app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        // Submit initial orders
        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order3 = client
            .submit_request(&new_request(3, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Get the first page with a limit of 2 (descending order, so order3 and order2)
        let first_page = client
            .list_orders_v2(None, Some(2), Some(SortDirection::Desc), None, None)
            .await
            .unwrap();

        assert_eq!(first_page.orders.len(), 2);
        assert_eq!(first_page.orders[0].order.request.id, order3.request.id);
        assert_eq!(first_page.orders[1].order.request.id, order2.request.id);
        assert!(first_page.has_more);
        assert!(first_page.next_cursor.is_some());

        // NOW submit new orders AFTER we got the cursor but BEFORE using it
        let order4 = client
            .submit_request(&new_request(4, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order5 = client
            .submit_request(&new_request(5, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Use the cursor to get the next page
        // This should ONLY return order1, NOT the newly inserted order4 or order5
        let second_page = client
            .list_orders_v2(
                first_page.next_cursor.clone(),
                Some(10),
                Some(SortDirection::Desc),
                None,
                None,
            )
            .await
            .unwrap();

        // We should only get order1, not the new orders 4 and 5
        assert_eq!(second_page.orders.len(), 1);
        assert_eq!(second_page.orders[0].order.request.id, order1.request.id);
        assert!(!second_page.has_more);

        // Verify that the new orders exist but weren't returned by the cursor query
        let all_orders = client
            .list_orders_v2(None, Some(10), Some(SortDirection::Desc), None, None)
            .await
            .unwrap();

        assert_eq!(all_orders.orders.len(), 5);
        assert_eq!(all_orders.orders[0].order.request.id, order5.request.id);
        assert_eq!(all_orders.orders[1].order.request.id, order4.request.id);
        assert_eq!(all_orders.orders[2].order.request.id, order3.request.id);
        assert_eq!(all_orders.orders[3].order.request.id, order2.request.id);
        assert_eq!(all_orders.orders[4].order.request.id, order1.request.id);

        server_handle.abort();
    }

    #[sqlx::test]
    async fn test_cursor_isolation_with_concurrent_inserts_asc(pool: PgPool) {
        use boundless_market::order_stream_client::SortDirection;

        let (client, _app_state, ctx, _anvil, server_handle, _addr) =
            setup_server_and_client(pool).await;

        // Submit initial orders
        let order1 = client
            .submit_request(&new_request(1, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order2 = client
            .submit_request(&new_request(2, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order3 = client
            .submit_request(&new_request(3, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order4 = client
            .submit_request(&new_request(4, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order5 = client
            .submit_request(&new_request(5, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Get the first page with ascending order (order1 and order2)
        let first_page_asc = client
            .list_orders_v2(None, Some(2), Some(SortDirection::Asc), None, None)
            .await
            .unwrap();

        assert_eq!(first_page_asc.orders.len(), 2);
        assert_eq!(first_page_asc.orders[0].order.request.id, order1.request.id);
        assert_eq!(first_page_asc.orders[1].order.request.id, order2.request.id);

        // Submit another new order BEFORE using the cursor
        let order6 = client
            .submit_request(&new_request(6, &ctx.prover_signer.address()), &ctx.prover_signer)
            .await
            .unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Use cursor to get next page in ascending order
        // In ascending order, the cursor returns all orders > cursor_point
        // Since order6 was inserted after order2 (has a later timestamp), it WILL be included
        let second_page_asc = client
            .list_orders_v2(
                first_page_asc.next_cursor,
                Some(10),
                Some(SortDirection::Asc),
                None,
                None,
            )
            .await
            .unwrap();

        assert_eq!(second_page_asc.orders.len(), 4);
        assert_eq!(second_page_asc.orders[0].order.request.id, order3.request.id);
        assert_eq!(second_page_asc.orders[1].order.request.id, order4.request.id);
        assert_eq!(second_page_asc.orders[2].order.request.id, order5.request.id);
        assert_eq!(second_page_asc.orders[3].order.request.id, order6.request.id);

        // Verify that order1 and order2 are not in the second page (already seen)
        assert!(!second_page_asc.orders.iter().any(|o| o.order.request.id == order1.request.id));
        assert!(!second_page_asc.orders.iter().any(|o| o.order.request.id == order2.request.id));

        server_handle.abort();
    }

    mod heartbeat_tests {
        use super::*;
        use alloy::signers::local::PrivateKeySigner;
        use boundless_market::telemetry::{
            BrokerHeartbeat, CommitmentOutcome, CompletionOutcome, EvalOutcome, RequestCompleted,
            RequestEvaluated, RequestHeartbeat, HEARTBEAT_PATH, HEARTBEAT_REQUESTS_PATH,
        };

        async fn sign_payload(body: &[u8], signer: &impl alloy::signers::Signer) -> String {
            let hash = alloy::primitives::eip191_hash_message(body);
            let sig = signer.sign_hash(&hash).await.unwrap();
            let mut sig_bytes = [0u8; 65];
            sig_bytes[..64].copy_from_slice(&sig.as_bytes()[..64]);
            sig_bytes[64] = sig.v() as u8;
            format!("0x{}", hex::encode(sig_bytes))
        }

        fn make_heartbeat(broker_address: Address) -> BrokerHeartbeat {
            BrokerHeartbeat {
                broker_address,
                config: serde_json::json!({"max_proofs": 4}),
                committed_orders_count: 2,
                pending_preflight_count: 5,
                version: "0.1.0-test".to_string(),
                uptime_secs: 3600,
                events_dropped: 0,
                timestamp: Utc::now(),
            }
        }

        fn make_request_heartbeat(broker_address: Address) -> RequestHeartbeat {
            RequestHeartbeat {
                broker_address,
                evaluated: vec![RequestEvaluated {
                    broker_address,
                    order_id: "0x01-0x00-LockAndFulfill".to_string(),
                    request_id: "0x01".to_string(),
                    request_digest: "0x00".to_string(),
                    requestor: Address::ZERO,
                    outcome: EvalOutcome::Locked,
                    skip_code: None,
                    skip_reason: None,
                    total_cycles: Some(1_000_000),
                    fulfillment_type: "LockAndFulfill".to_string(),
                    queue_duration_ms: Some(100),
                    preflight_duration_ms: Some(500),
                    received_at_timestamp: 0,
                    evaluated_at: Utc::now(),
                    commitment_outcome: Some(CommitmentOutcome::Committed),
                    commitment_skip_code: None,
                    commitment_skip_reason: None,
                    estimated_proving_time_secs: Some(30),
                    estimated_proving_time_no_load_secs: Some(10),
                    monitor_wait_duration_ms: Some(200),
                    peak_prove_khz: Some(100),
                    max_capacity: Some(4),
                    pending_commitment_count: Some(1),
                    concurrent_proving_jobs: Some(1),
                    lock_duration_secs: Some(2),
                }],
                completed: vec![RequestCompleted {
                    broker_address,
                    order_id: "0x02-0x00-LockAndFulfill".to_string(),
                    request_id: "0x02".to_string(),
                    request_digest: "0x00".to_string(),
                    proof_type: "Merkle".to_string(),
                    outcome: CompletionOutcome::Fulfilled,
                    error_code: None,
                    error_reason: None,
                    lock_duration_secs: Some(5),
                    proving_duration_secs: Some(25),
                    aggregation_duration_secs: Some(10),
                    submission_duration_secs: Some(3),
                    total_duration_secs: 45,
                    estimated_proving_time_secs: Some(30),
                    actual_total_proving_time_secs: Some(25),
                    concurrent_proving_jobs_start: Some(1),
                    concurrent_proving_jobs_end: Some(1),
                    total_cycles: Some(1_000_000),
                    fulfillment_type: "LockAndFulfill".to_string(),
                    stark_proving_secs: None,
                    proof_compression_secs: None,
                    set_builder_proving_secs: None,
                    assessor_proving_secs: None,
                    assessor_compression_proof_secs: None,
                    received_at_timestamp: 0,
                    completed_at: Utc::now(),
                }],
                events_dropped: 0,
                timestamp: Utc::now(),
            }
        }

        #[sqlx::test]
        async fn test_heartbeat_valid_signature(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let heartbeat = make_heartbeat(ctx.prover_signer.address());
            let body = serde_json::to_vec(&heartbeat).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 202, "Valid heartbeat should return 202");

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_heartbeat_wrong_signer(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let heartbeat = make_heartbeat(ctx.prover_signer.address());
            let body = serde_json::to_vec(&heartbeat).unwrap();

            // Sign with a different key than the claimed broker_address
            let wrong_signer = PrivateKeySigner::random();
            let sig = sign_payload(&body, &wrong_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 401, "Wrong signer should return 401");

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_heartbeat_missing_signature(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let heartbeat = make_heartbeat(ctx.prover_signer.address());
            let body = serde_json::to_vec(&heartbeat).unwrap();

            let url = format!("http://{addr}{HEARTBEAT_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 400, "Missing signature should return 400");

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_request_heartbeat_valid(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let heartbeat = make_request_heartbeat(ctx.prover_signer.address());
            let body = serde_json::to_vec(&heartbeat).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_REQUESTS_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 202, "Valid request heartbeat should return 202");

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_heartbeat_accepts_unknown_fields(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let payload = serde_json::json!({
                "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                "config": {"test": true},
                "committed_orders_count": 0,
                "pending_preflight_count": 0,
                "version": "test",
                "uptime_secs": 0,
                "events_dropped": 0,
                "timestamp": "2026-01-01T00:00:00Z",
                "unknown_field": "should_not_cause_400",
                "another_new_field": 42
            });
            let body = serde_json::to_vec(&payload).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 202, "Payload with unknown fields should return 202");

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_request_heartbeat_accepts_unknown_fields(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let payload = serde_json::json!({
                "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                "evaluated": [{
                    "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                    "order_id": "0x01-0x00-LockAndFulfill",
                    "request_id": "0x01",
                    "request_digest": "0x00",
                    "outcome": "Locked",
                    "fulfillment_type": "LockAndFulfill",
                    "brand_new_field": "hello",
                    "future_metric": 999
                }],
                "completed": [],
                "events_dropped": 0,
                "timestamp": "2026-01-01T00:00:00Z"
            });
            let body = serde_json::to_vec(&payload).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_REQUESTS_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                202,
                "Request heartbeat with unknown fields should return 202"
            );

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_request_heartbeat_drops_oversized_records(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let padding = "x".repeat(3_000);
            let payload = serde_json::json!({
                "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                "evaluated": [
                    {
                        "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                        "order_id": "0x01-0x00-LockAndFulfill",
                        "oversized_field": padding
                    },
                    {
                        "broker_address": format!("{:#x}", ctx.prover_signer.address()),
                        "order_id": "0x02-0x00-LockAndFulfill"
                    }
                ],
                "completed": [],
                "events_dropped": 0,
                "timestamp": "2026-01-01T00:00:00Z"
            });
            let body = serde_json::to_vec(&payload).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_REQUESTS_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(
                response.status(),
                202,
                "Request with oversized records should still succeed (records silently dropped)"
            );

            server_handle.abort();
        }

        #[sqlx::test]
        async fn test_request_heartbeat_empty_lists(pool: PgPool) {
            let (client, _app_state, ctx, _anvil, server_handle, addr) =
                setup_server_and_client(pool).await;

            let heartbeat = RequestHeartbeat {
                broker_address: ctx.prover_signer.address(),
                evaluated: vec![],
                completed: vec![],
                events_dropped: 0,
                timestamp: Utc::now(),
            };
            let body = serde_json::to_vec(&heartbeat).unwrap();
            let sig = sign_payload(&body, &ctx.prover_signer).await;

            let url = format!("http://{addr}{HEARTBEAT_REQUESTS_PATH}");
            let response = client
                .client
                .post(&url)
                .header("Content-Type", "application/json")
                .header("X-Signature", &sig)
                .body(body)
                .send()
                .await
                .unwrap();

            assert_eq!(response.status(), 202, "Empty request heartbeat should return 202");

            server_handle.abort();
        }
    }
}
