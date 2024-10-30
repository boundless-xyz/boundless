// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    primitives::Address,
    signers::{k256::ecdsa::SigningKey, local::LocalSigner},
};
use anyhow::{Context, Result};
use boundless_market::contracts::ProvingRequest;
use reqwest::Url;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async, tungstenite::client::IntoClientRequest, MaybeTlsStream, WebSocketStream,
};

use crate::{AuthMsg, Order, ORDER_SUBMISSION_PATH, ORDER_WS_PATH};

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
