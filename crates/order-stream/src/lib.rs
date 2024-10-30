// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{collections::HashMap, error::Error, pin::Pin};

use alloy::{
    primitives::{utils::parse_ether, Address, Signature, SignatureError, B256, U256},
    providers::ProviderBuilder,
    signers::{local::PrivateKeySigner, Error as SignerErr, Signer},
};
use anyhow::{anyhow, Context, Error as AnyhowErr, Result};
use async_stream::stream;
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
use boundless_market::contracts::{IProofMarket, ProvingRequest};
use clap::Parser;
use futures_util::{stream::SplitSink, SinkExt, Stream, StreamExt};
use rand::{rngs::SmallRng, seq::SliceRandom, SeedableRng};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{json, to_string};
use std::sync::Arc;
use thiserror::Error;
use tokio::{net::TcpStream, sync::Mutex};
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::error;

pub mod client;

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

/// Order struct, containing a ProvingRequest and its Signature
/// The contents of this struct match the calldata of the `submitOrder` function in the `ProofMarket` contract.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Order {
    pub request: ProvingRequest,
    pub signature: Signature,
}

impl Order {
    /// Create a new Order
    pub fn new(request: ProvingRequest, signature: Signature) -> Self {
        Self { request, signature }
    }

    /// Validate the Order
    pub fn validate(&self, market_address: Address, chain_id: u64) -> Result<(), AppError> {
        self.request.validate().map_err(|e| AppError::InvalidRequest(e))?;
        self.request
            .verify_signature(&self.signature, market_address, chain_id)
            .map_err(|e| AppError::InvalidSignature(e))?;
        Ok(())
    }
}

/// AuthMsg struct, containing a hash, an address, and a signature.
/// It is used to authenticate WebSocket connections, where the authenticated
/// address is used to check the balance in the ProofMarket contract.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AuthMsg {
    pub hash: B256,
    pub address: Address,
    pub signature: Signature,
}

impl AuthMsg {
    /// Create a new AuthMsg
    pub fn new(hash: B256, address: Address, signature: Signature) -> Self {
        Self { hash, address, signature }
    }

    /// Create a new AuthMsg from a PrivateKeySigner. The hash is randomly generated.
    pub async fn new_from_signer(signer: &PrivateKeySigner) -> Result<Self, SignerErr> {
        let rand_bytes: [u8; 32] = rand::random();
        let hash = B256::from(rand_bytes);
        let signature = signer.sign_hash(&hash).await?;
        Ok(Self::new(hash, signer.address(), signature))
    }

    /// Recover the address from the signature and compare it with the address field.
    pub fn verify_signature(&self) -> Result<(), SignerErr> {
        let addr = self.signature.recover_address_from_prehash(&self.hash)?;
        if addr == self.address {
            Ok(())
        } else {
            Err(SignerErr::SignatureError(SignatureError::FromBytes("Address mismatch")))
        }
    }
}

/// Error type for the application
#[derive(Error, Debug)]
pub enum AppError {
    #[error("invalid request: {0}")]
    InvalidRequest(AnyhowErr),
    #[error("invalid signature: {0}")]
    InvalidSignature(SignerErr),
    #[error("internal error")]
    InternalErr(AnyhowErr),
}

impl AppError {
    fn type_str(&self) -> String {
        match self {
            Self::InvalidRequest(_) => "InvalidRequest",
            Self::InvalidSignature(_) => "InvalidSignature",
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

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let code = match self {
            Self::InvalidRequest(_) | Self::InvalidSignature(_) => StatusCode::BAD_REQUEST,
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
    #[clap(long, env, default_value = "0.0.0.0:8080")]
    bind_addr: String,
    /// RPC URL for the Ethereum node
    #[clap(long, env)]
    rpc_url: Url,
    /// Address of the ProofMarket contract
    #[clap(long, env)]
    proof_market_address: Address,
    /// Chain ID of the Ethereum network
    #[clap(long, env)]
    chain_id: u64,
    /// Minimum balance required to connect to the WebSocket
    #[clap(long, value_parser = parse_ether)]
    min_balance: U256,
}

/// Configuration struct
#[derive(Clone)]
pub struct Config {
    /// RPC URL for the Ethereum node
    pub rpc_url: Url,
    /// Address of the ProofMarket contract
    pub market_address: Address,
    /// Chain ID of the Ethereum network
    pub chain_id: u64,
    /// Minimum balance required to connect to the WebSocket
    pub min_balance: U256,
}

impl From<&Args> for Config {
    fn from(args: &Args) -> Self {
        Self {
            rpc_url: args.rpc_url.clone(),
            market_address: args.proof_market_address,
            chain_id: args.chain_id,
            min_balance: args.min_balance,
        }
    }
}

/// Application state struct
pub struct AppState {
    // Map of WebSocket connections by address
    connections: Arc<Mutex<HashMap<Address, Arc<Mutex<SplitSink<WebSocket, Message>>>>>>,
    // Configuration
    config: Config,
}

impl AppState {
    /// Create a new AppState
    pub fn new(config: &Config) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            config: config.clone(),
        }))
    }
}

const MAX_ORDER_SIZE: usize = 25 * 1024 * 1024; // 25 mb

const ORDER_SUBMISSION_PATH: &str = "orders";
// Submit order handler
async fn submit_order(
    State(state): State<Arc<AppState>>,
    Json(order): Json<Order>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Validate the order
    order.validate(state.config.market_address, state.config.chain_id)?;
    let id = order.request.id.clone();

    // Check the balance
    let provider = ProviderBuilder::new().on_http(state.config.rpc_url.clone());
    let proof_market = IProofMarket::new(state.config.market_address, provider);
    let balance = proof_market.balanceOf(order.request.client_address()).call().await.unwrap()._0;
    if balance < U256::from(order.request.offer.maxPrice) {
        return Err(AppError::InvalidRequest(anyhow!(
            "Insufficient balance to cover request: {} < {}",
            balance,
            order.request.offer.maxPrice
        )));
    }

    // Spawn a background task for broadcasting
    let state_clone = Arc::clone(&state);
    let order_clone = order.clone();
    tokio::spawn(async move {
        broadcast_order(&order_clone, state_clone).await;
    });
    tracing::debug!("Proving request {:?} submitted", id);

    Ok(Json(json!({ "status": "success", "request_id": id })))
}

fn parse_auth_msg(value: &HeaderValue) -> Result<AuthMsg, AnyhowErr> {
    let json_str = value.to_str().context("Invalid header encoding")?;
    serde_json::from_str(json_str).context("Failed to parse JSON")
}

const ORDER_WS_PATH: &str = "orders/ws";
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

    // Check the balance
    let provider = ProviderBuilder::new().on_http(state.config.rpc_url.clone());
    let proof_market = IProofMarket::new(state.config.market_address, provider);
    let balance = proof_market.balanceOf(auth_msg.address).call().await.unwrap()._0;
    if balance < state.config.min_balance {
        return (StatusCode::UNAUTHORIZED, "Insufficient balance").into_response();
    }

    // Check if the address is already connected
    let cloned_state = state.clone();
    let connections = cloned_state.connections.lock().await;
    if connections.contains_key(&auth_msg.address) {
        return (StatusCode::CONFLICT, "Client already connected").into_response();
    }

    // Proceed with WebSocket upgrade
    tracing::debug!("New webSocket connection from {}", auth_msg.address);
    ws.on_upgrade(move |socket| websocket_connection(socket, auth_msg.address, state))
}

// Function to broadcast an order to all WebSocket clients in random order
async fn broadcast_order(order: &Order, state: Arc<AppState>) {
    let json = match to_string(&order) {
        Ok(json) => json,
        Err(err) => {
            error!("Failed to serialize Order: {}", err);
            return;
        }
    };

    let shuffled_connections = {
        let connections = state.connections.lock().await;
        let mut connections_vec: Vec<_> = connections.values().cloned().collect();
        let mut rng = SmallRng::from_entropy();
        connections_vec.shuffle(&mut rng);
        connections_vec
    };

    // Send the message to each connection in random order
    for connection in shuffled_connections {
        let mut conn = connection.lock().await;
        if let Err(err) = conn.send(Message::Text(json.clone())).await {
            error!("Failed to send message: {}", err);
        }
    }
    tracing::debug!("Order {:?} broadcasted", order.request.id);
}

async fn websocket_connection(socket: WebSocket, address: Address, state: Arc<AppState>) {
    let (sender, mut receiver) = socket.split();

    let sender = Arc::new(Mutex::new(sender));

    {
        // Add sender to the list of connections
        let mut connections = state.connections.lock().await;
        connections.insert(address, sender.clone());
    }

    // Handle incoming messages (if needed) or keep the connection open
    while let Some(Ok(_)) = receiver.next().await {}

    // Remove the connection when the socket closes
    let mut connections = state.connections.lock().await;
    connections.remove(&address);
    tracing::debug!("WebSocket connection closed: {}", address);
}

/// Stream of Order messages from a WebSocket
///
/// This function takes a WebSocket stream and returns a stream of `Order` messages.
/// Example usage:
/// ```no_run
/// use futures_util::StreamExt;
/// use order_stream::{client::Client, order_stream, Order};
/// async fn example_stream(client: Client) {
///     let socket = client.connect_async().await.unwrap();
///     let mut order_stream = order_stream(socket);
///     while let Some(order) = order_stream.next().await {
///         match order {
///             Ok(order) => println!("Received order: {:?}", order),
///             Err(err) => eprintln!("Error: {}", err),
///         }
///     }
/// }
/// ```
pub fn order_stream(
    mut socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Pin<Box<dyn Stream<Item = Result<Order, Box<dyn Error + Send + Sync>>> + Send>> {
    Box::pin(stream! {
        while let Some(msg_result) = socket.next().await {
            match msg_result {
                Ok(tungstenite::Message::Text(msg)) => {
                    match serde_json::from_str::<Order>(&msg) {
                        Ok(order) => yield Ok(order),
                        Err(err) => yield Err(Box::new(err) as Box<dyn Error + Send + Sync>),
                    }
                }
                Ok(other) => {
                    tracing::debug!("Ignoring non-text message: {:?}", other);
                    continue;
                }
                Err(err) => yield Err(Box::new(err) as Box<dyn Error + Send + Sync>),
            }
        }
    })
}

/// Create the application router
pub fn app(state: Arc<AppState>) -> Router {
    let body_size_limit = RequestBodyLimitLayer::new(MAX_ORDER_SIZE);

    Router::new()
        .route(&format!("/{ORDER_SUBMISSION_PATH}"), post(submit_order).layer(body_size_limit))
        .route(&format!("/{ORDER_WS_PATH}"), get(websocket_handler))
        .with_state(state)
}

/// Run the REST API service
pub async fn run(args: &Args) -> Result<()> {
    let config = args.into();
    let app_state = AppState::new(&config).context("Failed to initialize AppState")?;
    let listener = tokio::net::TcpListener::bind(&args.bind_addr)
        .await
        .context("Failed to bind a TCP listener")?;

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
    use boundless_market::contracts::{test_utils::TestCtx, Input, Offer, Predicate, Requirements};
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
        let chain_id = anvil.chain_id();
        let rpc_url = anvil.endpoint_url();

        let ctx = TestCtx::new(&anvil).await.unwrap();

        ctx.prover_market.deposit(parse_ether("2").unwrap()).await.unwrap();

        let config = Config {
            rpc_url,
            market_address: *ctx.prover_market.instance().address(),
            chain_id,
            min_balance: parse_ether("2").unwrap(),
        };
        let app_state = AppState::new(&config).unwrap();
        let listener = tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, self::app(app_state.clone())).into_future());

        let client = client::Client::new(
            Url::parse(&format!("http://{addr}", addr = addr)).unwrap(),
            ctx.prover_signer.clone(),
            config.market_address,
            config.chain_id,
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
