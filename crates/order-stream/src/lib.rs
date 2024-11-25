// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::collections::HashMap;

use alloy::{
    primitives::{utils::parse_ether, Address, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    transports::http::Http,
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
    AuthMsg, ErrMsg, Order, OrderError, AUTH_GET_NONCE, ORDER_LIST_PATH, ORDER_SUBMISSION_PATH,
    ORDER_WS_PATH,
};
use clap::Parser;
use reqwest::{Client, Url};
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

mod api;
mod order_db;
mod ws;

use api::{
    __path_get_nonce, __path_list_orders, __path_submit_order, get_nonce, list_orders, submit_order,
};
use order_db::OrderDb;
use ws::{start_broadcast_task, websocket_handler, ConnectionsMap, __path_websocket_handler};

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

    /// Minimum balance required to connect to the WebSocket
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
}

/// Configuration struct
#[derive(Clone)]
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
        }
    }
}

/// Application state struct
pub struct AppState {
    // Database backend
    db: OrderDb,
    // Map of WebSocket connections by address
    connections: Arc<Mutex<ConnectionsMap>>,
    // Ethereum RPC provider
    rpc_provider: RootProvider<Http<Client>>,
    // Configuration
    config: Config,
    // chain_id
    chain_id: u64,
}

impl AppState {
    /// Create a new AppState
    pub async fn new(config: &Config, db_pool_opt: Option<PgPool>) -> Result<Arc<Self>> {
        let provider = ProviderBuilder::new().on_http(config.rpc_url.clone());
        let db = if let Some(db_pool) = db_pool_opt {
            OrderDb::from_pool(db_pool).await?
        } else {
            OrderDb::from_env().await.context("Failed to connect to DB")?
        };
        let chain_id =
            provider.get_chain_id().await.context("Failed to fetch chain_id from RPC")?;
        Ok(Arc::new(Self {
            db,
            connections: Arc::new(Mutex::new(HashMap::new())),
            rpc_provider: ProviderBuilder::new().on_http(config.rpc_url.clone()),
            config: config.clone(),
            chain_id,
        }))
    }
}

const MAX_ORDER_SIZE: usize = 25 * 1024 * 1024; // 25 mb

#[derive(OpenApi, Debug, Deserialize)]
#[openapi(
    paths(submit_order, list_orders, get_nonce, websocket_handler),
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
        .route(&format!("/{ORDER_SUBMISSION_PATH}"), post(submit_order).layer(body_size_limit))
        .route(&format!("/{ORDER_LIST_PATH}"), get(list_orders))
        .route(&format!("/{AUTH_GET_NONCE}:addr"), get(get_nonce))
        .route(&format!("/{ORDER_WS_PATH}"), get(websocket_handler))
        .merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Run the REST API service
pub async fn run(args: &Args) -> Result<()> {
    let config: Config = args.into();

    let app_state = AppState::new(&config, None).await?;
    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .context("Failed to bind a TCP listener")?;

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

    tracing::info!("REST API listening on: {}", args.bind_addr);
    axum::serve(listener, self::app(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("REST API service failed")?;

    Ok(())
}

async fn shutdown_signal() {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order_db::{DbOrder, OrderDbErr};
    use alloy::{
        node_bindings::Anvil,
        primitives::{B256, U256},
    };
    use boundless_market::{
        contracts::{test_utils::TestCtx, Input, Offer, Predicate, ProofRequest, Requirements},
        order_stream_client::Client,
    };
    use futures_util::StreamExt;
    use reqwest::Url;
    use sqlx::PgPool;
    use std::{
        future::IntoFuture,
        net::{Ipv4Addr, SocketAddr},
    };
    use tokio::task::JoinHandle;

    fn new_request(idx: u32, addr: &Address) -> ProofRequest {
        ProofRequest::new(
            idx,
            addr,
            Requirements { imageId: B256::from([1u8; 32]), predicate: Predicate::default() },
            "http://image_uri.null",
            Input::default(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                biddingStart: 1,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U256::from(10),
            },
        )
    }

    #[sqlx::test]
    async fn integration_test(pool: PgPool) {
        let anvil = Anvil::new().spawn();
        let rpc_url = anvil.endpoint_url();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        ctx.prover_market.deposit(parse_ether("2").unwrap()).await.unwrap();

        let config = Config {
            rpc_url,
            market_address: *ctx.prover_market.instance().address(),
            min_balance: parse_ether("2").unwrap(),
            max_connections: 1,
            queue_size: 10,
            domain: "0.0.0.0:8585".parse().unwrap(),
            bypass_addrs: vec![],
        };
        let app_state = AppState::new(&config, Some(pool)).await.unwrap();
        let app_state_clone = app_state.clone();

        let task: JoinHandle<Result<DbOrder, OrderDbErr>> = tokio::spawn(async move {
            let mut new_orders = app_state_clone.db.order_stream().await.unwrap();
            let order = new_orders.next().await.unwrap().unwrap();
            Ok(order)
        });

        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, self::app(app_state.clone())).into_future());

        let client = Client::new(
            Url::parse(&format!("http://{addr}", addr = addr)).unwrap(),
            ctx.prover_signer.clone(),
            config.market_address,
            app_state.chain_id,
        );

        // 2. Requestor submits a request
        let order =
            client.submit_request(&new_request(1, &ctx.prover_signer.address())).await.unwrap();

        // 3. Broker receives the request
        let db_order = task.await.unwrap().unwrap();

        assert_eq!(order, db_order.order);
    }
}
