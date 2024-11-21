// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    primitives::{aliases::U192, Address, Signature},
    signers::{k256::ecdsa::SigningKey, local::LocalSigner, Error as SignerErr, Signer},
};
use anyhow::{Context, Error as AnyhowErr, Result};
use async_stream::stream;
use chrono::Utc;
use futures_util::{Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use siwe::Message as SiweMsg;
use std::{error::Error, pin::Pin};
use thiserror::Error;
use time::OffsetDateTime;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite, tungstenite::client::IntoClientRequest, MaybeTlsStream,
    WebSocketStream,
};
use utoipa::ToSchema;

use crate::contracts::ProvingRequest;

pub const ORDER_SUBMISSION_PATH: &str = "api/submit_order";
pub const ORDER_LIST_PATH: &str = "api/orders";
pub const AUTH_GET_NONCE: &str = "api/nonce/";
pub const ORDER_WS_PATH: &str = "ws/orders";

/// Error body for API responses
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ErrMsg {
    /// Error type enum
    pub r#type: String,
    /// Error message body
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
///
/// The contents of this struct match the calldata of the `submitOrder` function in the `ProofMarket` contract.
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq)]
pub struct Order {
    /// Order request
    #[schema(value_type = Object)]
    pub request: ProvingRequest,
    /// Order signature
    #[schema(value_type = Object)]
    pub signature: Signature,
}

/// Order data + order-stream id
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone)]
pub struct OrderData {
    /// Order stream id
    pub id: i64,
    /// Order data
    pub order: Order,
}

/// Nonce object for authentication to order-stream websocket
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone)]
pub struct Nonce {
    /// Nonce hex encoded
    pub nonce: String,
}

/// Response for submitting a new order
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone)]
pub struct SubmitOrderRes {
    /// Status of the order submission
    pub status: String,
    /// Request ID submitted
    #[schema(value_type = Object)]
    pub request_id: U192,
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

/// Authentication message for connecting to order-stream websock
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct AuthMsg {
    /// SIWE message body
    #[schema(value_type = Object)]
    message: SiweMsg,
    /// SIWE Signature of `message` field
    #[schema(value_type = Object)]
    signature: Signature,
}

impl AuthMsg {
    /// Creates a new authentication message from a nonce, origin, signer
    pub async fn new(nonce: Nonce, origin: &Url, signer: &impl Signer) -> Result<Self> {
        let message = format!(
            "{} wants you to sign in with your Ethereum account:\n{}\n\nBoundless Order Stream\n\nURI: {}\nVersion: 1\nChain ID: 1\nNonce: {}\nIssued At: {}",
            origin.authority(), signer.address(), origin, nonce.nonce, Utc::now().to_rfc3339(),
        );
        let message: SiweMsg = message.parse()?;

        let signature = signer
            .sign_hash(&message.eip191_hash().context("Failed to generate eip191 hash")?.into())
            .await?;

        Ok(Self { message, signature })
    }

    /// Verify a [AuthMsg] message + signature
    pub async fn verify(&self, domain: &str, nonce: &str) -> Result<()> {
        let opts = siwe::VerificationOpts {
            domain: Some(domain.parse().context("Invalid domain")?),
            nonce: Some(nonce.into()),
            timestamp: Some(OffsetDateTime::now_utc()),
            ..Default::default()
        };

        self.message
            .verify(&self.signature.as_bytes(), &opts)
            .await
            .context("Failed to verify SIWE message")
    }

    /// [AuthMsg] address in alloy format
    pub fn address(&self) -> Address {
        Address::from(self.message.address)
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

    /// Get the nonce from the order stream service for websocket auth
    pub async fn get_nonce(&self) -> Result<Nonce> {
        let url =
            Url::parse(&format!("{}{AUTH_GET_NONCE}{}", self.base_url, self.signer.address()))?;
        let res = self.client.get(url).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("Http error {} fetching nonce", res.status())
        }
        let nonce = res.json().await?;

        Ok(nonce)
    }

    /// Return a WebSocket stream connected to the order stream server
    ///
    /// An authentication message is sent to the server via the `X-Auth-Data` header.
    /// The authentication message must contain a valid claim of an address holding a (pre-configured)
    /// minimum balance on the boundless market in order to connect to the server.
    /// Only one connection per address is allowed.
    pub async fn connect_async(&self) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let nonce = self.get_nonce().await.context("Failed to fetch nonce from order-stream")?;

        let auth_msg = AuthMsg::new(nonce, &self.base_url, &self.signer).await?;

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
        let (socket, _) = match connect_async(request).await {
            Ok(res) => res,
            Err(tokio_tungstenite::tungstenite::Error::Http(err)) => {
                let http_err = if let Some(http_body) = err.body() {
                    String::from_utf8_lossy(&http_body)
                } else {
                    "Empty http error body".into()
                };
                anyhow::bail!("Failed to connect to ws endpoint: {} {}", self.base_url, http_err);
            }
            Err(err) => {
                anyhow::bail!("Failed to connect to ws endpoint: {} {err:?}", self.base_url);
            }
        };
        Ok(socket)
    }
}

/// Stream of Order messages from a WebSocket
///
/// This function takes a WebSocket stream and returns a stream of `Order` messages.
/// Example usage:
/// ```no_run
/// use boundless_market::order_stream_client::{Client, order_stream, OrderData};
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
) -> Pin<Box<dyn Stream<Item = Result<OrderData, Box<dyn Error + Send + Sync>>> + Send>> {
    Box::pin(stream! {
        while let Some(msg_result) = socket.next().await {
            match msg_result {
                Ok(tungstenite::Message::Text(msg)) => {
                    match serde_json::from_str::<OrderData>(&msg) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auth_msg_verify() {
        let signer = LocalSigner::random();
        let nonce = Nonce { nonce: "TEST_NONCE".to_string() };
        let origin = "http://localhost:8585".parse().unwrap();
        let auth_msg = AuthMsg::new(nonce.clone(), &origin, &signer).await.unwrap();
        auth_msg.verify("localhost:8585", &nonce.nonce).await.unwrap();
    }

    #[tokio::test]
    #[should_panic(expected = "Message domain does not match")]
    async fn auth_msg_bad_origin() {
        let signer = LocalSigner::random();
        let nonce = Nonce { nonce: "TEST_NONCE".to_string() };
        let origin = "http://localhost:8585".parse().unwrap();
        let auth_msg = AuthMsg::new(nonce.clone(), &origin, &signer).await.unwrap();
        auth_msg.verify("boundless.xyz", &nonce.nonce).await.unwrap();
    }

    #[tokio::test]
    #[should_panic(expected = "Message nonce does not match")]
    async fn auth_msg_bad_nonce() {
        let signer = LocalSigner::random();
        let nonce = Nonce { nonce: "TEST_NONCE".to_string() };
        let origin = "http://localhost:8585".parse().unwrap();
        let auth_msg = AuthMsg::new(nonce.clone(), &origin, &signer).await.unwrap();
        auth_msg.verify("localhost:8585", &"BAD_NONCE").await.unwrap();
    }
}
