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

//! Test helpers: MockFleetNode — a real Axum HTTP server that signs requests
//! with a given `PrivateKeySigner`.  Enables true round-trip parity tests
//! against `HttpRemoteSignerBackend` without any pre-programmed stubs.

use std::sync::Arc;

use alloy::{
    hex,
    primitives::B256,
    signers::{local::PrivateKeySigner, Signer},
};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tokio::net::TcpListener;

// ── Shared Axum state ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AppState {
    pub wallet: PrivateKeySigner,
    /// Role name this mock serves (e.g. "prover" or "rewards").
    pub role: String,
}

// ── Mock server ───────────────────────────────────────────────────────────────

/// A mock fleet-node that spins up a real Axum HTTP server on a random local
/// port and signs requests with the provided private key.
///
/// Implements:
/// - `GET  /sign/capabilities` — returns wallet address for the configured role
/// - `POST /sign`              — dispatches on `type`: hash, message, transaction
///
/// Drop the struct to shut down the server (the spawned task is detached;
/// it will be cleaned up when the tokio runtime stops).
pub struct MockFleetNode {
    pub base_url: String,
    // Keep handle alive for the test duration.
    _handle: tokio::task::JoinHandle<()>,
}

impl MockFleetNode {
    /// Spin up the mock server with the given private key and role name.
    pub async fn new(private_key: &str, role: &str) -> Self {
        let wallet: PrivateKeySigner = private_key.parse().expect("valid private key");
        let state = Arc::new(AppState { wallet, role: role.to_owned() });

        let app = Router::new()
            .route("/sign/capabilities", get(capabilities))
            .route("/sign", post(sign))
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind random port");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("axum serve");
        });

        Self { base_url: format!("http://127.0.0.1:{}", addr.port()), _handle: handle }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// GET /sign/capabilities
async fn capabilities(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let addr = format!("{:#x}", s.wallet.address());
    Json(serde_json::json!({
        "roles": {
            s.role.as_str(): { "address": addr }
        }
    }))
}

/// POST /sign body
#[derive(Deserialize)]
struct SignBody {
    #[serde(rename = "type")]
    kind: String,
    hash: Option<String>,
    message: Option<String>,
}

/// POST /sign — signs with the embedded wallet and returns a JSON response
/// matching the fleet-node contract understood by `HttpRemoteSignerBackend`.
async fn sign(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SignBody>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match body.kind.as_str() {
        "hash" => {
            let hex_str = body.hash.ok_or(StatusCode::BAD_REQUEST)?;
            let bytes =
                hex::decode(hex_str.trim_start_matches("0x")).map_err(|_| StatusCode::BAD_REQUEST)?;
            if bytes.len() != 32 {
                return Err(StatusCode::BAD_REQUEST);
            }
            let hash = B256::from_slice(&bytes);
            let sig = s
                .wallet
                .sign_hash(&hash)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let sig_hex = format!("0x{}", hex::encode(sig.as_bytes()));
            Ok(Json(serde_json::json!({ "signature": sig_hex })))
        }
        "message" => {
            let hex_str = body.message.ok_or(StatusCode::BAD_REQUEST)?;
            let bytes =
                hex::decode(hex_str.trim_start_matches("0x")).map_err(|_| StatusCode::BAD_REQUEST)?;
            let sig = s
                .wallet
                .sign_message(&bytes)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            let sig_hex = format!("0x{}", hex::encode(sig.as_bytes()));
            Ok(Json(serde_json::json!({ "signature": sig_hex })))
        }
        "transaction" => {
            // Return a placeholder tx hash — broadcast is out of scope for these tests.
            Ok(Json(serde_json::json!({ "tx_hash": format!("0x{}", "00".repeat(32)) })))
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}
