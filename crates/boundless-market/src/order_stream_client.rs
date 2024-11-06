// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{error::Error, pin::Pin};

use alloy::{
    primitives::{Address, Signature, SignatureError, B256},
    signers::{
        k256::ecdsa::SigningKey, local::LocalSigner, local::PrivateKeySigner, Error as SignerErr,
        Signer,
    },
};
use anyhow::{Context, Error as AnyhowErr, Result};
use async_stream::stream;
use futures_util::{Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite, tungstenite::client::IntoClientRequest, MaybeTlsStream,
    WebSocketStream,
};

use crate::contracts::ProvingRequest;

pub const ORDER_SUBMISSION_PATH: &str = "orders";
pub const ORDER_WS_PATH: &str = "ws/orders";

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

/// Error type for the Order
#[derive(Error, Debug)]
pub enum OrderError {
    #[error("invalid request: {0}")]
    InvalidRequest(AnyhowErr),
    #[error("invalid signature: {0}")]
    InvalidSignature(SignerErr),
}

impl From<AnyhowErr> for OrderError {
    fn from(err: AnyhowErr) -> Self {
        Self::InvalidRequest(err)
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
    pub fn validate(&self, market_address: Address, chain_id: u64) -> Result<(), OrderError> {
        self.request.validate().map_err(|e| OrderError::InvalidRequest(e))?;
        self.request
            .verify_signature(&self.signature.as_bytes().into(), market_address, chain_id)
            .map_err(|e| OrderError::InvalidSignature(e))?;
        Ok(())
    }
}

/// Client for interacting with the order stream server
#[derive(Clone, Debug)]
pub struct Client {
    /// HTTP client
    pub client: reqwest::Client,
    /// Base URL of the order stream server
    pub base_url: Url,
    /// Signer for signing requests
    pub signer: LocalSigner<SigningKey>,
    /// Address of the proof market contract
    pub proof_market_address: Address,
    /// Chain ID of the network
    pub chain_id: u64,
}

impl Client {
    /// Create a new client
    pub fn new(
        base_url: Url,
        signer: LocalSigner<SigningKey>,
        proof_market_address: Address,
        chain_id: u64,
    ) -> Self {
        Self { client: reqwest::Client::new(), base_url, signer, proof_market_address, chain_id }
    }

    /// Submit a proving request to the order stream server
    pub async fn submit_request(&self, request: &ProvingRequest) -> Result<Order> {
        let url = Url::parse(&format!("{}{ORDER_SUBMISSION_PATH}", self.base_url))?;
        let signature =
            request.sign_request(&self.signer, self.proof_market_address, self.chain_id)?;
        let order = Order { request: request.clone(), signature };
        order.validate(self.proof_market_address, self.chain_id)?;
        let order_json = serde_json::to_value(&order)?;
        let response = self
            .client
            .post(url)
            .header("Content-Type", "application/json")
            .json(&order_json)
            .send()
            .await?;

        // Check for any errors in the response
        if let Err(err) = response.error_for_status_ref() {
            let error_message = match response.json::<serde_json::Value>().await {
                Ok(json_body) => {
                    json_body["msg"].as_str().unwrap_or("Unknown server error").to_string()
                }
                Err(_) => "Failed to read server error message".to_string(),
            };

            return Err(anyhow::Error::new(err).context(error_message));
        }

        Ok(order)
    }

    /// Return a WebSocket stream connected to the order stream server
    ///
    /// An authentication message is sent to the server via the `X-Auth-Data` header.
    /// The authentication message must contain a valid claim of an address holding a (pre-configured)
    /// minimum balance on the boundless market in order to connect to the server.
    /// Only one connection per address is allowed.
    pub async fn connect_async(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        // Create the authentication message
        let auth_msg = AuthMsg::new_from_signer(&self.signer)
            .await
            .context("failed to create auth message")?;

        // Serialize the `AuthMsg` to JSON
        let auth_json =
            serde_json::to_string(&auth_msg).context("failed to serialize auth message")?;

        // Construct the WebSocket URL
        let host = self.base_url.host().context("missing host")?.to_string();
        let ws_url = match self.base_url.port() {
            Some(port) => format!("ws://{host}:{port}/{ORDER_WS_PATH}"),
            None => format!("ws://{host}{ORDER_WS_PATH}"),
        };

        // Create the WebSocket request
        let mut request = ws_url.into_client_request().context("failed to create request")?;
        request
            .headers_mut()
            .insert("X-Auth-Data", auth_json.parse().context("failed to parse auth message")?);

        // Connect to the WebSocket server and return the socket
        let (socket, _) =
            connect_async(request).await.context("failed to connect to server at {ws_url}")?;
        Ok(socket)
    }
}

/// Stream of Order messages from a WebSocket
///
/// This function takes a WebSocket stream and returns a stream of `Order` messages.
/// Example usage:
/// ```no_run
/// use boundless_market::order_stream_client::{Client, order_stream, Order};
/// use futures_util::StreamExt;
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
