// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::collections::HashMap;

use alloy::{
    primitives::{utils::parse_ether, Address, U256},
    providers::{Provider, ProviderBuilder, RootProvider},
    transports::http::Http,
};
use anyhow::{anyhow, Context, Error as AnyhowErr, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Json, State, WebSocketUpgrade,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use boundless_market::{
    contracts::IProofMarket,
    order_stream_client::{AuthMsg, Order, OrderError, ORDER_SUBMISSION_PATH, ORDER_WS_PATH},
};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use rand::{seq::SliceRandom, thread_rng};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use thiserror::Error;
use tokio::{
    sync::{broadcast, mpsc, Mutex},
    task::JoinHandle,
};
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing::error;

mod order_db;

#[derive(Debug, Deserialize, Serialize)]
pub struct ErrMsg {
    pub r#type: String,
    pub msg: String,
}
impl ErrMsg {
    pub fn new(r#type: &str, msg: &str) -> Self {
        Self { r#type: r#type.into(), msg: msg.into() }
    }
}
impl std::fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error_type: {} msg: {}", self.r#type, self.msg)
    }
}

/// Error type for the application
#[derive(Error, Debug)]
pub enum AppError {
    #[error("invalid order: {0}")]
    InvalidOrder(OrderError),
    #[error("internal error")]
    InternalErr(AnyhowErr),
}

impl AppError {
    fn type_str(&self) -> String {
        match self {
            Self::InvalidOrder(_) => "InvalidOrder",
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
            Self::InvalidOrder(_) => StatusCode::BAD_REQUEST,
            Self::InternalErr(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        error!("api error, code {code}: {self:?}");

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
    /// Address of the ProofMarket contract
    #[clap(long, env)]
    proof_market_address: Address,
    /// Minimum balance required to connect to the WebSocket
    #[clap(long, value_parser = parse_ether)]
    min_balance: U256,
    /// Maximum number of WebSocket connections
    #[clap(long, default_value = "100")]
    max_connections: usize,
    /// Maximum size of the queue for each WebSocket connection
    #[clap(long, default_value = "100")]
    queue_size: usize,
}

/// Configuration struct
#[derive(Clone)]
pub struct Config {
    /// RPC URL for the Ethereum node
    pub rpc_url: Url,
    /// Address of the ProofMarket contract
    pub market_address: Address,
    /// Minimum balance required to connect to the WebSocket
    pub min_balance: U256,
    /// Maximum number of WebSocket connections
    pub max_connections: usize,
    /// Maximum size of the queue for each WebSocket connection
    pub queue_size: usize,
}

impl From<&Args> for Config {
    fn from(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            market_address: args.proof_market_address,
            min_balance: args.min_balance,
            max_connections: args.max_connections,
            queue_size: args.queue_size,
        }
    }
}

struct ClientConnection {
    sender: mpsc::Sender<String>, // Channel to send messages to this client
}

type ConnectionsMap = HashMap<Address, ClientConnection>;

/// Application state struct
pub struct AppState {
    // Map of WebSocket connections by address
    connections: Arc<Mutex<ConnectionsMap>>,
    // Channel sender for orders
    order_tx: broadcast::Sender<Order>,
    // Ethereum RPC provider
    rpc_provider: RootProvider<Http<Client>>,
    // Configuration
    config: Config,
    // chain_id
    chain_id: u64,
}

impl AppState {
    /// Create a new AppState
    pub async fn new(config: &Config, order_tx: broadcast::Sender<Order>) -> Result<Arc<Self>> {
        let provider = ProviderBuilder::new().on_http(config.rpc_url.clone());
        let chain_id =
            provider.get_chain_id().await.context("Failed to fetch chain_id from RPC")?;
        Ok(Arc::new(Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            order_tx,
            rpc_provider: ProviderBuilder::new().on_http(config.rpc_url.clone()),
            config: config.clone(),
            chain_id,
        }))
    }
}

// Start the broadcast task
fn start_broadcast_task(
    state: Arc<AppState>,
    mut order_rx: broadcast::Receiver<Order>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Ok(order) = order_rx.recv().await {
            broadcast_order(&order, Arc::clone(&state)).await;
        }
    })
}

const MAX_ORDER_SIZE: usize = 25 * 1024 * 1024; // 25 mb

// Submit order handler
async fn submit_order(
    State(state): State<Arc<AppState>>,
    Json(order): Json<Order>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Validate the order
    order.validate(state.config.market_address, state.chain_id)?;
    let id = order.request.id.clone();

    // Send the order to the channel for broadcasting
    if let Err(err) = state.order_tx.send(order.clone()) {
        error!("Failed to send order to broadcast task: {}", err);
        return Err(AppError::InternalErr(anyhow!("Internal server error")));
    }

    tracing::debug!("Order 0x{id:x} submitted");
    Ok(Json(json!({ "status": "success", "request_id": id })))
}

fn parse_auth_msg(value: &HeaderValue) -> Result<AuthMsg> {
    let json_str = value.to_str().context("Invalid header encoding")?;
    serde_json::from_str(json_str).context("Failed to parse JSON")
}

// WebSocket upgrade handler
async fn websocket_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Response {
    let auth_header = match headers.get("X-Auth-Data") {
        Some(value) => value,
        None => return (StatusCode::BAD_REQUEST, "Missing auth header").into_response(),
    };

    // Decode and parse the JSON header into `AuthMsg`
    let auth_msg: AuthMsg = match parse_auth_msg(auth_header) {
        Ok(auth_msg) => auth_msg,
        Err(_) => return (StatusCode::BAD_REQUEST, "Invalid auth message format").into_response(),
    };

    // Check the signature
    if let Err(err) = auth_msg.verify_signature() {
        return (StatusCode::UNAUTHORIZED, err.to_string()).into_response();
    }

    // Check if the address is already connected
    {
        let connections = state.connections.lock().await;
        if connections.contains_key(&auth_msg.address) {
            return (
                StatusCode::CONFLICT,
                // TODO: send serialized Error types for the client to de-serialize vs strings.
                format!("Client {} already connected", auth_msg.address),
            )
                .into_response();
        }
        if connections.len() >= state.config.max_connections {
            return (StatusCode::SERVICE_UNAVAILABLE, "Server at capacity").into_response();
        }
    }

    // Check the balance
    // TODO: This check has several issues:
    // - The balance could change between the check and the connection lifetime
    // - It opens up to an unbounded number of RPC requests to the Ethereum node
    // As such, a more robust solution would be to use a separate task that keeps track of the balances
    // by subscribing to events from the ProofMarket contract. Then, the WebSocket connection would be allowed
    // if the balance is above the threshold and the connection would be dropped if the balance falls below the threshold.
    let proof_market = IProofMarket::new(state.config.market_address, state.rpc_provider.clone());
    let balance = proof_market.balanceOf(auth_msg.address).call().await.unwrap()._0;
    if balance < state.config.min_balance {
        return (
            StatusCode::UNAUTHORIZED,
            format!("Insufficient balance: {} < {}", balance, state.config.min_balance),
        )
            .into_response();
    }

    // Proceed with WebSocket upgrade
    tracing::debug!("New webSocket connection from {}", auth_msg.address);
    ws.on_upgrade(move |socket| websocket_connection(socket, auth_msg.address, state))
}

// Function to broadcast an order to all WebSocket clients in random order
async fn broadcast_order(order: &Order, state: Arc<AppState>) {
    let order_json = match serde_json::to_string(&order) {
        Ok(order_json) => order_json,
        Err(err) => {
            error!("Failed to serialize order 0x{:x}: {}", order.request.id, err);
            return;
        }
    };

    // Shuffle the connections
    let connections_list = {
        let connections = state.connections.lock().await;
        let mut connections_list: Vec<_> =
            connections.iter().map(|(addr, conn)| (addr.clone(), conn.sender.clone())).collect();
        connections_list.shuffle(&mut thread_rng());
        connections_list
    };

    let mut clients_to_remove = Vec::new();
    for (address, sender) in connections_list {
        match sender.try_send(order_json.clone()) {
            Ok(_) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Client {}'s message queue is full, message dropped", address);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!("Client {}'s message queue is closed, removing client", address);
                // Add the client to the list of clients to remove
                clients_to_remove.push(address.clone());
            }
        }
    }
    // Remove the clients that have closed their connections
    if !clients_to_remove.is_empty() {
        let mut connections = state.connections.lock().await;
        for address in clients_to_remove {
            connections.remove(&address);
        }
    }

    tracing::debug!("Order 0x{:x} broadcasted", order.request.id);
}

async fn websocket_connection(socket: WebSocket, address: Address, state: Arc<AppState>) {
    let (mut sender_ws, mut recver_ws) = socket.split();

    let (sender_channel, mut receiver_channel) = mpsc::channel::<String>(state.config.queue_size);

    // Add sender to the list of connections
    {
        let mut connections = state.connections.lock().await;
        connections.insert(address, ClientConnection { sender: sender_channel.clone() });
    }

    let mut errors_counter = 0usize;

    loop {
        tokio::select! {
            msg = receiver_channel.recv() => {
                match msg {
                    Some(msg) => {
                        match sender_ws.send(Message::Text(msg)).await {
                            Ok(_) => {
                                // Reset the error counter on successful send
                                errors_counter = 0;
                            }
                            Err(err) => {
                                tracing::warn!("Failed to send message to client {}: {}", address, err);
                                errors_counter += 1;
                                if errors_counter > 10 {
                                    tracing::warn!(
                                        "Too many consecutive send errors to client {}; disconnecting",
                                        address
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            ws_msg = recver_ws.next() => {
                // This polls on the recv side of the websocket connection, once a connection closes
                // either via Err or graceful Message::Close, the nex() will return None and we can close the
                // connection.
                match ws_msg {
                    Some(_) => {
                        // TODO: cleaner management of Some(Ok(Message::Close))
                    }
                    None => {
                        tracing::debug!("Empty recv, closing connections");
                        break;
                    }
                }
            }
        }
    }

    // Remove the connection when the send loop exits
    let mut connections = state.connections.lock().await;
    connections.remove(&address);
    tracing::debug!("WebSocket connection closed: {}", address);
}

/// Create the application router
pub fn app(state: Arc<AppState>) -> Router {
    let body_size_limit = RequestBodyLimitLayer::new(MAX_ORDER_SIZE);

    Router::new()
        .route(&format!("/{ORDER_SUBMISSION_PATH}"), post(submit_order).layer(body_size_limit))
        .route(&format!("/{ORDER_WS_PATH}"), get(websocket_handler))
        .with_state(state)
        .layer(TraceLayer::new_for_http())
}

/// Run the REST API service
pub async fn run(args: &Args) -> Result<()> {
    let config: Config = args.into();
    let (order_tx, _) = broadcast::channel(config.max_connections);
    let app_state = AppState::new(&config, order_tx).await?;
    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .context("Failed to bind a TCP listener")?;

    let app_state_clone = Arc::clone(&app_state);
    tokio::spawn(async move {
        loop {
            let order_rx = app_state_clone.order_tx.subscribe();
            let broadcast_task = start_broadcast_task(Arc::clone(&app_state_clone), order_rx);

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
    use alloy::{
        node_bindings::Anvil,
        primitives::{aliases::U96, B256},
    };
    use boundless_market::{
        contracts::{test_utils::TestCtx, Input, Offer, Predicate, ProvingRequest, Requirements},
        order_stream_client::{order_stream, Client},
    };
    use reqwest::Url;
    use std::{
        future::IntoFuture,
        net::{Ipv4Addr, SocketAddr},
    };

    fn new_request(idx: u32, addr: &Address) -> ProvingRequest {
        ProvingRequest::new(
            idx,
            addr,
            Requirements { imageId: B256::from([1u8; 32]), predicate: Predicate::default() },
            "http://image_uri.null",
            Input::default(),
            Offer {
                minPrice: U96::from(20000000000000u64),
                maxPrice: U96::from(40000000000000u64),
                biddingStart: 1,
                timeout: 100,
                rampUpPeriod: 1,
                lockinStake: U96::from(10),
            },
        )
    }

    #[tokio::test]
    async fn integration_test() {
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
        };
        let (order_tx, order_rx) = broadcast::channel(config.max_connections);
        let app_state = AppState::new(&config, order_tx).await.unwrap();
        start_broadcast_task(app_state.clone(), order_rx);

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

        // 1. Broker connects to the WebSocket
        let socket = client.connect_async().await.unwrap();

        // 2. Requestor submits a request
        let order =
            client.submit_request(&new_request(1, &ctx.prover_signer.address())).await.unwrap();

        // 3. Broker receives the request
        let received_order = order_stream(socket).next().await.unwrap().unwrap();

        assert_eq!(order, received_order);
    }
}
