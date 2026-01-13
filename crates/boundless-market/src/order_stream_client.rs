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

use std::{fmt::Display, pin::Pin};

use alloy::{
    primitives::{Address, Signature, U256},
    signers::{Error as SignerErr, Signer},
};
use alloy_primitives::B256;
use anyhow::{Context, Result};
use async_stream::stream;
use chrono::{DateTime, Utc};
use futures_util::{SinkExt, Stream, StreamExt};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use siwe::Message as SiweMsg;
use thiserror::Error;
use time::OffsetDateTime;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite, tungstenite::client::IntoClientRequest, MaybeTlsStream,
    WebSocketStream,
};
use utoipa::ToSchema;

use crate::contracts::{ProofRequest, RequestError};

/// Order stream submission API path.
pub const ORDER_SUBMISSION_PATH: &str = "/api/v1/submit_order";
/// Order stream order list API path.
pub const ORDER_LIST_PATH: &str = "/api/v1/orders";
/// Order stream order list API path (v2 with cursor pagination).
pub const ORDER_LIST_PATH_V2: &str = "/api/v2/orders";
/// Order stream nonce API path.
pub const AUTH_GET_NONCE: &str = "/api/v1/nonce/";
/// Order stream health check API path.
pub const HEALTH_CHECK: &str = "/api/v1/health";
/// Order stream websocket path.
pub const ORDER_WS_PATH: &str = "/ws/v1/orders";

/// Error body for API responses
#[derive(Debug, Deserialize, Serialize, ToSchema)]
pub struct ErrMsg {
    /// Error type enum
    pub r#type: String,
    /// Error message body
    pub msg: String,
}
impl ErrMsg {
    /// Create a new error message.
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
#[non_exhaustive]
pub enum OrderError {
    #[error("invalid signature: {0}")]
    /// Invalid signature error.
    InvalidSignature(SignerErr),
    #[error("request error: {0}")]
    /// Request error.
    RequestError(#[from] RequestError),
}

/// Order struct, containing a ProofRequest and its Signature
///
/// The contents of this struct match the calldata of the `submitOrder` function in the `BoundlessMarket` contract.
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone, PartialEq)]
pub struct Order {
    /// Order request
    #[schema(value_type = Object)]
    pub request: ProofRequest,
    /// Request digest
    #[schema(value_type = Object)]
    pub request_digest: B256,
    /// Order signature
    // TODO: This should not be Signature. It should be Bytes or Vec<u8>.
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
    /// Time the order was submitted
    #[schema(value_type = String)]
    pub created_at: DateTime<Utc>,
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
    pub request_id: U256,
}

/// Sort direction for listing orders
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    /// Ascending order (oldest first)
    Asc,
    /// Descending order (newest first)
    Desc,
}

/// Response for v2 list orders API with pagination info
#[derive(Serialize, Deserialize, ToSchema, Debug, Clone)]
pub struct ListOrdersV2Response {
    /// List of orders
    pub orders: Vec<OrderData>,
    /// Cursor for the next page (if available)
    pub next_cursor: Option<String>,
    /// Whether there are more results available
    pub has_more: bool,
}

impl Order {
    /// Create a new Order
    pub fn new(request: ProofRequest, request_digest: B256, signature: Signature) -> Self {
        Self { request, request_digest, signature }
    }

    /// Validate the Order
    pub fn validate(&self, market_address: Address, chain_id: u64) -> Result<(), OrderError> {
        self.request.validate()?;
        let hash = self.request.signing_hash(market_address, chain_id)?;
        if hash != self.request_digest {
            return Err(OrderError::RequestError(RequestError::DigestMismatch));
        }
        self.request.verify_signature(
            &self.signature.as_bytes().into(),
            market_address,
            chain_id,
        )?;
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

/// VersionInfo struct for SIWE message
#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct VersionInfo {
    /// Version of the Boundless client
    pub version: String,
    /// Git hash of the Boundless client
    pub git_hash: String,
}

impl Display for VersionInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Boundless Order Stream\nVersion={}\nGit hash={}", self.version, self.git_hash)
    }
}

impl From<&SiweMsg> for VersionInfo {
    fn from(msg: &SiweMsg) -> Self {
        let mut version = "unknown".to_string();
        let mut git_hash = "unknown".to_string();
        if let Some(statement) = &msg.statement {
            let parts: Vec<&str> = statement.split(':').collect();
            if parts.len() == 3 && parts[0] == "Boundless Order Stream" {
                version = parts[1].to_string();
                git_hash = parts[2].to_string();
            }
        }
        Self { version, git_hash }
    }
}

impl From<&AuthMsg> for VersionInfo {
    fn from(auth_msg: &AuthMsg) -> Self {
        VersionInfo::from(&auth_msg.message)
    }
}

impl AuthMsg {
    /// Creates a new authentication message from a nonce, origin, signer
    pub async fn new(nonce: Nonce, origin: &Url, signer: &impl Signer) -> Result<Self> {
        let version = env!("CARGO_PKG_VERSION");
        let git_hash = option_env!("BOUNDLESS_GIT_HASH").unwrap_or("unknown");
        let message = format!(
            "{} wants you to sign in with your Ethereum account:\n{}\n\nBoundless Order Stream:{version}:{git_hash}\n\nURI: {}\nVersion: 1\nChain ID: 1\nNonce: {}\nIssued At: {}",
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

    /// Return the version info from the message
    pub fn version_info(&self) -> VersionInfo {
        VersionInfo::from(self)
    }
}

/// Client for interacting with the order stream server
#[derive(Clone, Debug)]
pub struct OrderStreamClient {
    /// HTTP client
    pub client: reqwest::Client,
    /// Base URL of the order stream server
    pub base_url: Url,
    /// Address of the market contract
    pub boundless_market_address: Address,
    /// Chain ID of the network
    pub chain_id: u64,
    /// Optional API key for authenticated requests
    pub api_key: Option<String>,
}

impl OrderStreamClient {
    /// Create a new client
    pub fn new(base_url: Url, boundless_market_address: Address, chain_id: u64) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            boundless_market_address,
            chain_id,
            api_key: None,
        }
    }

    /// Create a new client with API key
    pub fn new_with_api_key(
        base_url: Url,
        boundless_market_address: Address,
        chain_id: u64,
        api_key: String,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            boundless_market_address,
            chain_id,
            api_key: Some(api_key),
        }
    }

    /// Helper to add API key header to request if configured
    fn add_api_key_header(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(api_key) = &self.api_key {
            request.header("x-api-key", api_key)
        } else {
            request
        }
    }

    /// Submit a proof request to the order stream server
    pub async fn submit_request(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<Order> {
        let url = self.base_url.join(ORDER_SUBMISSION_PATH)?;
        let signature =
            request.sign_request(signer, self.boundless_market_address, self.chain_id).await?;
        let request_digest = request.signing_hash(self.boundless_market_address, self.chain_id)?;
        let order = Order { request: request.clone(), request_digest, signature };
        order.validate(self.boundless_market_address, self.chain_id)?;
        let order_json = serde_json::to_value(&order)?;
        let response = self
            .add_api_key_header(
                self.client.post(url).header("Content-Type", "application/json").json(&order_json),
            )
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

    /// Fetch an order from the order stream server.
    ///
    /// If multiple orders are found, the `request_digest` must be provided to select the correct order.
    pub async fn fetch_order(&self, id: U256, request_digest: Option<B256>) -> Result<Order> {
        let url = self.base_url.join(&format!("{ORDER_LIST_PATH}/{id}"))?;
        let response = self.add_api_key_header(self.client.get(url)).send().await?;

        if !response.status().is_success() {
            let error_message = match response.json::<serde_json::Value>().await {
                Ok(json_body) => {
                    json_body["msg"].as_str().unwrap_or("Unknown server error").to_string()
                }
                Err(_) => "Failed to read server error message".to_string(),
            };

            return Err(anyhow::Error::msg(error_message));
        }

        let order_data: Vec<OrderData> = response.json().await?;
        let orders: Vec<Order> = order_data.into_iter().map(|data| data.order).collect();
        if orders.is_empty() {
            return Err(anyhow::Error::msg("No order found"));
        } else if orders.len() == 1 {
            return Ok(orders[0].clone());
        }
        match request_digest {
            Some(digest) => {
                for order in orders {
                    if order.request_digest == digest {
                        return Ok(order);
                    }
                }
                Err(anyhow::Error::msg("No order found"))
            }
            None => {
                Err(anyhow::Error::msg("Multiple orders found, please provide a request digest"))
            }
        }
    }

    /// Fetch an order with its creation timestamp from the order stream server.
    ///
    /// Returns both the Order and the timestamp when it was created.
    /// If multiple orders are found, the `request_digest` must be provided to select the correct order.
    pub async fn fetch_order_with_timestamp(
        &self,
        id: U256,
        request_digest: Option<B256>,
    ) -> Result<(Order, DateTime<Utc>)> {
        let url = self.base_url.join(&format!("{ORDER_LIST_PATH}/{id}"))?;
        let response = self.add_api_key_header(self.client.get(url)).send().await?;

        if !response.status().is_success() {
            let error_message = match response.json::<serde_json::Value>().await {
                Ok(json_body) => {
                    json_body["msg"].as_str().unwrap_or("Unknown server error").to_string()
                }
                Err(_) => "Failed to read server error message".to_string(),
            };

            return Err(anyhow::Error::msg(error_message));
        }

        let order_data: Vec<OrderData> = response.json().await?;
        if order_data.is_empty() {
            return Err(anyhow::Error::msg("No order found"));
        } else if order_data.len() == 1 {
            let data = &order_data[0];
            return Ok((data.order.clone(), data.created_at));
        }
        match request_digest {
            Some(digest) => {
                for data in order_data {
                    if data.order.request_digest == digest {
                        return Ok((data.order, data.created_at));
                    }
                }
                Err(anyhow::Error::msg("No order found"))
            }
            None => {
                Err(anyhow::Error::msg("Multiple orders found, please provide a request digest"))
            }
        }
    }

    /// Get the nonce from the order stream service for websocket auth
    pub async fn get_nonce(&self, address: Address) -> Result<Nonce> {
        let url = self.base_url.join(AUTH_GET_NONCE)?.join(&address.to_string())?;
        let res = self.add_api_key_header(self.client.get(url)).send().await?;
        if !res.status().is_success() {
            anyhow::bail!("Http error {} fetching nonce", res.status())
        }
        let nonce = res.json().await?;

        Ok(nonce)
    }

    /// List orders sorted by creation time descending (most recent first)
    ///
    /// Returns orders created after the given timestamp (if provided), up to the specified limit.
    /// If `after` is None, returns the most recent orders.
    pub async fn list_orders_by_creation(
        &self,
        after: Option<DateTime<Utc>>,
        limit: u64,
    ) -> Result<Vec<OrderData>> {
        let mut url = self.base_url.join(ORDER_LIST_PATH)?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("sort", "desc");
            query.append_pair("limit", &limit.to_string());
            if let Some(ts) = after {
                query.append_pair("after", &ts.to_rfc3339());
            }
        }

        let response = self.add_api_key_header(self.client.get(url)).send().await?;

        if !response.status().is_success() {
            let error_message = match response.json::<serde_json::Value>().await {
                Ok(json_body) => {
                    json_body["msg"].as_str().unwrap_or("Unknown server error").to_string()
                }
                Err(_) => "Failed to read server error message".to_string(),
            };

            return Err(anyhow::Error::msg(error_message));
        }

        let orders: Vec<OrderData> = response.json().await?;
        Ok(orders)
    }

    /// List orders with cursor-based pagination and flexible filtering (v2)
    ///
    /// Provides cursor-based pagination for stable results, bidirectional sorting,
    /// and timestamp range filtering. This is the recommended method for paginating
    /// through orders as it handles concurrent inserts correctly.
    ///
    /// # Arguments
    /// * `cursor` - Opaque cursor string from previous response for pagination
    /// * `limit` - Maximum number of orders to return
    /// * `sort` - Sort direction (Asc for oldest first, Desc for newest first)
    /// * `before` - Optional timestamp to filter orders created before this time (EXCLUSIVE: `created_at < before`)
    /// * `after` - Optional timestamp to filter orders created after this time (EXCLUSIVE: `created_at > after`)
    ///
    /// # Boundary Semantics
    /// Both `before` and `after` parameters use exclusive comparison operators:
    /// - `before`: Returns orders where `created_at < before` (does NOT include orders at the exact timestamp)
    /// - `after`: Returns orders where `created_at > after` (does NOT include orders at the exact timestamp)
    ///
    /// To include orders at a specific timestamp boundary, add/subtract a small time delta (e.g., 1 second).
    pub async fn list_orders_v2(
        &self,
        cursor: Option<String>,
        limit: Option<u64>,
        sort: Option<SortDirection>,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
    ) -> Result<ListOrdersV2Response> {
        let mut url = self.base_url.join(ORDER_LIST_PATH_V2)?;

        {
            let mut query = url.query_pairs_mut();
            if let Some(cursor_str) = cursor {
                query.append_pair("cursor", &cursor_str);
            }
            if let Some(limit_val) = limit {
                query.append_pair("limit", &limit_val.to_string());
            }
            if let Some(sort_val) = sort {
                let sort_str = match sort_val {
                    SortDirection::Asc => "asc",
                    SortDirection::Desc => "desc",
                };
                query.append_pair("sort", sort_str);
            }
            if let Some(ts) = before {
                query.append_pair("before", &ts.to_rfc3339());
            }
            if let Some(ts) = after {
                query.append_pair("after", &ts.to_rfc3339());
            }
        }

        let response = self.add_api_key_header(self.client.get(url)).send().await?;

        if !response.status().is_success() {
            let error_message = match response.json::<serde_json::Value>().await {
                Ok(json_body) => {
                    json_body["msg"].as_str().unwrap_or("Unknown server error").to_string()
                }
                Err(_) => "Failed to read server error message".to_string(),
            };

            return Err(anyhow::Error::msg(error_message));
        }

        let response_data: ListOrdersV2Response = response.json().await?;
        Ok(response_data)
    }

    /// Return a WebSocket stream connected to the order stream server
    ///
    /// An authentication message is sent to the server via the `X-Auth-Data` header.
    /// The authentication message must contain a valid claim of an address holding a (pre-configured)
    /// minimum balance on the boundless market in order to connect to the server.
    /// Only one connection per address is allowed.
    pub async fn connect_async(
        &self,
        signer: &impl Signer,
    ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        let nonce = self
            .get_nonce(signer.address())
            .await
            .context("Failed to fetch nonce from order-stream")?;

        let auth_msg = AuthMsg::new(nonce, &self.base_url, signer).await?;

        // Serialize the `AuthMsg` to JSON
        let auth_json =
            serde_json::to_string(&auth_msg).context("failed to serialize auth message")?;

        // Construct the WebSocket URL
        let host = self.base_url.host().context("missing host")?.to_string();
        // Select TLS vs not
        let ws_scheme = if self.base_url.scheme() == "https" { "wss" } else { "ws" };

        let ws_url = match self.base_url.port() {
            Some(port) => format!("{ws_scheme}://{host}:{port}{ORDER_WS_PATH}"),
            None => format!("{ws_scheme}://{host}{ORDER_WS_PATH}"),
        };

        // Create the WebSocket request
        let mut request =
            ws_url.clone().into_client_request().context("failed to create request")?;
        request
            .headers_mut()
            .insert("X-Auth-Data", auth_json.parse().context("failed to parse auth message")?);

        // Add API key header if configured
        if let Some(api_key) = &self.api_key {
            request
                .headers_mut()
                .insert("x-api-key", api_key.parse().context("failed to parse api key")?);
        }

        // Connect to the WebSocket server and return the socket
        let (socket, _) = match connect_async(request).await {
            Ok(res) => res,
            Err(tokio_tungstenite::tungstenite::Error::Http(err)) => {
                let http_err = if let Some(http_body) = err.body() {
                    String::from_utf8_lossy(http_body)
                } else {
                    "Empty http error body".into()
                };
                anyhow::bail!(
                    "Failed to connect to ws endpoint ({}): {} {}",
                    ws_url,
                    self.base_url,
                    http_err
                );
            }
            Err(err) => {
                anyhow::bail!(
                    "Failed to connect to ws endpoint ({}): {} {}",
                    ws_url,
                    self.base_url,
                    err
                );
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
/// use alloy::signers::Signer;
/// use boundless_market::order_stream_client::{OrderStreamClient, order_stream, OrderData};
/// use futures_util::StreamExt;
/// async fn example_stream(client: OrderStreamClient, signer: &impl Signer) {
///     let socket = client.connect_async(signer).await.unwrap();
///     let mut order_stream = order_stream(socket);
///     while let Some(order) = order_stream.next().await {
///         println!("Received order: {:?}", order)
///     }
/// }
/// ```
#[allow(clippy::type_complexity)]
pub fn order_stream(
    mut socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Pin<Box<dyn Stream<Item = OrderData> + Send>> {
    Box::pin(stream! {
        // Create a ping interval - configurable via environment variable
        let ping_duration = match std::env::var("ORDER_STREAM_CLIENT_PING_MS") {
            Ok(ms) => match ms.parse::<u64>() {
                Ok(ms) => {
                    tracing::debug!("Using custom ping interval of {}ms", ms);
                    tokio::time::Duration::from_millis(ms)
                },
                Err(_) => {
                    tracing::warn!("Invalid ORDER_STREAM_CLIENT_PING_MS value: {}, using default", ms);
                    tokio::time::Duration::from_secs(30)
                }
            },
            Err(_) => tokio::time::Duration::from_secs(30),
        };

        let mut ping_interval = tokio::time::interval(ping_duration);
        // Track the last ping we sent
        let mut ping_data: Option<Vec<u8>> = None;

        loop {
            tokio::select! {
                // Handle incoming messages
                msg_result = socket.next() => {
                    match msg_result {
                        Some(Ok(tungstenite::Message::Text(msg))) => {
                            match serde_json::from_str::<OrderData>(&msg) {
                                Ok(order) => yield order,
                                Err(err) => {
                                    tracing::warn!("Failed to parse order: {:?}", err);
                                    continue;
                                }
                            }
                        }
                        // Reply to Ping's inline
                        Some(Ok(tungstenite::Message::Ping(data))) => {
                            tracing::trace!("Responding to ping");
                            if let Err(err) = socket.send(tungstenite::Message::Pong(data)).await {
                                tracing::warn!("Failed to send pong: {:?}", err);
                                break;
                            }
                        }
                        // Handle Pong responses
                        Some(Ok(tungstenite::Message::Pong(data))) => {
                            tracing::trace!("Received pong from server");
                            if let Some(expected_data) = ping_data.take() {
                                if data != expected_data {
                                    tracing::warn!("Server responded with invalid pong data");
                                    break;
                                }
                            } else {
                                tracing::warn!("Received unexpected pong from order-stream server");
                            }
                        }
                        Some(Ok(tungstenite::Message::Close(msg))) => {
                            tracing::warn!("Server closed the order stream connection with reason: {:?}", msg.map(|m| m.to_string()));
                            break;
                        }
                        Some(Ok(other)) => {
                            tracing::debug!("Ignoring non-text message: {:?}", other);
                            continue;
                        }
                        Some(Err(err)) => {
                            tracing::warn!("order stream socket error: {:?}", err);
                            break;
                        }
                        None => {
                            tracing::warn!("order stream socket closed unexpectedly");
                            break;
                        }
                    }
                }
                // Send periodic pings
                _ = ping_interval.tick() => {
                    // If we still have a pending ping that hasn't been responded to
                    if ping_data.is_some() {
                        tracing::warn!("Server did not respond to ping, closing connection");
                        break;
                    }

                    tracing::trace!("Sending ping to server");
                    let random_bytes: Vec<u8> = (0..16).map(|_| rand::random::<u8>()).collect();
                    if let Err(err) = socket.send(tungstenite::Message::Ping(random_bytes.clone())).await {
                        tracing::warn!("Failed to send ping: {:?}", err);
                        break;
                    }
                    ping_data = Some(random_bytes);
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::signers::local::LocalSigner;
    use std::borrow::Cow;
    use std::time::Duration;
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;
    use tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};

    #[tokio::test]
    async fn auth_msg_verify() {
        let version = env!("CARGO_PKG_VERSION");
        let git_hash = option_env!("BOUNDLESS_GIT_HASH").unwrap_or("unknown");
        let signer = LocalSigner::random();
        let nonce = Nonce { nonce: "TEST_NONCE".to_string() };
        let origin = "http://localhost:8585".parse().unwrap();
        let auth_msg = AuthMsg::new(nonce.clone(), &origin, &signer).await.unwrap();
        let version_info = auth_msg.version_info();
        println!("VersionInfo: {}", version_info);
        assert!(version_info.version == version);
        assert!(version_info.git_hash == git_hash);
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
        auth_msg.verify("localhost:8585", "BAD_NONCE").await.unwrap();
    }

    #[tokio::test]
    #[tracing_test::traced_test]
    async fn order_stream_ends_on_close_with_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = accept_async(stream).await.unwrap();
            let close_frame =
                CloseFrame { code: CloseCode::Normal, reason: Cow::Borrowed("test reason") };
            ws.send(tungstenite::Message::Close(Some(close_frame))).await.unwrap();
        });

        let url = format!("ws://{}/ws", addr);
        let (socket, _) = connect_async(url).await.unwrap();
        let mut stream = order_stream(socket);

        let next = tokio::time::timeout(Duration::from_secs(2), stream.next()).await.unwrap();
        assert!(next.is_none());
        assert!(logs_contain("Server closed the order stream connection with reason:"));
        assert!(logs_contain("test reason"));

        server_task.await.unwrap();
    }
}
