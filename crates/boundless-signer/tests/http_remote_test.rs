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

//! Parity tests for `HttpRemoteSignerBackend` against `LocalSignerBackend`.
//!
//! Uses a real `MockFleetNode` Axum server (see `tests/helpers/mod.rs`) that
//! signs with the same private key as the local backend.  This validates the
//! full HTTP wire protocol without any pre-programmed stub responses.
//!
//! Run with: `cargo test -p boundless-signer --features http-remote`

#![cfg(feature = "http-remote")]

mod helpers;

use std::{sync::Arc, time::Duration};

use alloy::{
    primitives::B256,
    providers::ProviderBuilder,
    signers::{local::PrivateKeySigner, Signer},
};
use boundless_signer::{
    ConcreteSignerBackend, HttpRemoteSignerBackend, LocalSignerBackend, SignerBackend,
    SignerBackendBridge, SignerError, SignerRole,
};
use helpers::MockFleetNode;
use httpmock::MockServer;

/// Anvil account 0 — deterministic, well-known.
const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Short timeout for test HTTP clients.
const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn make_local_backend(wallet: PrivateKeySigner) -> Arc<ConcreteSignerBackend> {
    let provider =
        ProviderBuilder::new().connect_http("http://127.0.0.1:8545".parse().expect("valid url"));
    Arc::new(ConcreteSignerBackend::Local(LocalSignerBackend::from_signer(wallet, provider)))
}

async fn make_remote_backend(base_url: &str) -> Arc<ConcreteSignerBackend> {
    Arc::new(ConcreteSignerBackend::HttpRemote(
        HttpRemoteSignerBackend::new_with_timeout(base_url, SignerRole::Prover, TEST_TIMEOUT)
            .await
            .expect("connect to mock server"),
    ))
}

// ── capabilities / address ────────────────────────────────────────────────────

#[tokio::test]
async fn http_capabilities_returns_correct_address() {
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();
    let expected = wallet.address();

    let mock = MockFleetNode::new(TEST_KEY, "prover").await;
    let remote = make_remote_backend(&mock.base_url).await;

    assert_eq!(remote.sender_address(), expected);
}

// ── sign_hash parity ──────────────────────────────────────────────────────────

#[tokio::test]
async fn http_sign_hash_parity() {
    let hash = B256::from([0x42u8; 32]);
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let mock = MockFleetNode::new(TEST_KEY, "prover").await;
    let local = make_local_backend(wallet);
    let remote = make_remote_backend(&mock.base_url).await;

    let sig_local = local.sign_hash(hash).await.unwrap();
    let sig_remote = remote.sign_hash(hash).await.unwrap();

    assert_eq!(sig_local, sig_remote, "sign_hash: HTTP remote must match local backend");
}

// ── sign_message parity ───────────────────────────────────────────────────────

#[tokio::test]
async fn http_sign_message_parity() {
    let msg = b"Round-trip fleet-node signing test";
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let mock = MockFleetNode::new(TEST_KEY, "prover").await;
    let local = make_local_backend(wallet);
    let remote = make_remote_backend(&mock.base_url).await;

    let sig_local = local.sign_message(msg).await.unwrap();
    let sig_remote = remote.sign_message(msg).await.unwrap();

    assert_eq!(sig_local, sig_remote, "sign_message: HTTP remote must match local backend");
}

// ── bridge parity ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn http_sign_via_bridge_parity() {
    let hash = B256::from([0xCAu8; 32]);
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let mock = MockFleetNode::new(TEST_KEY, "prover").await;
    let local_bridge = SignerBackendBridge::new(make_local_backend(wallet));
    let remote_bridge = SignerBackendBridge::new(make_remote_backend(&mock.base_url).await);

    let sig_local = local_bridge.sign_hash(&hash).await.unwrap();
    let sig_remote = remote_bridge.sign_hash(&hash).await.unwrap();

    assert_eq!(sig_local, sig_remote, "sign_hash via SignerBackendBridge: HTTP remote must match local");
}

// ── error mapping ─────────────────────────────────────────────────────────────

/// Creates an `HttpRemoteSignerBackend` pointed at a mock server that has been
/// pre-configured to respond to `GET /sign/capabilities` successfully (so
/// construction succeeds), then returns the given status code for `POST /sign`.
async fn make_error_backend(capabilities_addr: &str, post_status: u16) -> (MockServer, Arc<dyn SignerBackend>) {
    let server = MockServer::start_async().await;

    // Capabilities — always succeed so construction doesn't fail.
    server.mock(|when, then| {
        when.method("GET").path("/sign/capabilities");
        then.status(200).json_body(serde_json::json!({
            "roles": { "prover": { "address": capabilities_addr } }
        }));
    });

    // Sign — return the target error code.
    server.mock(|when, then| {
        when.method("POST").path("/sign");
        then.status(post_status).json_body(serde_json::json!({
            "error": "test_error",
            "message": "injected for test"
        }));
    });

    let backend = Arc::new(
        HttpRemoteSignerBackend::new_with_timeout(&server.base_url(), SignerRole::Prover, TEST_TIMEOUT)
            .await
            .expect("construction with mock capabilities"),
    ) as Arc<dyn SignerBackend>;

    (server, backend)
}

// We use a fixed valid address for the capabilities stub.
const STUB_ADDR: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

#[tokio::test]
async fn http_error_403_maps_to_rejected() {
    let (_server, backend) = make_error_backend(STUB_ADDR, 403).await;
    let err = backend.sign_hash(B256::ZERO).await.unwrap_err();
    assert!(
        matches!(err, SignerError::Rejected { .. }),
        "HTTP 403 must map to SignerError::Rejected, got: {err:?}"
    );
}

#[tokio::test]
async fn http_error_503_maps_to_unavailable() {
    let (_server, backend) = make_error_backend(STUB_ADDR, 503).await;
    let err = backend.sign_hash(B256::ZERO).await.unwrap_err();
    assert!(
        matches!(err, SignerError::Unavailable { .. }),
        "HTTP 503 must map to SignerError::Unavailable, got: {err:?}"
    );
}

#[tokio::test]
async fn http_error_408_maps_to_timeout() {
    let (_server, backend) = make_error_backend(STUB_ADDR, 408).await;
    let err = backend.sign_hash(B256::ZERO).await.unwrap_err();
    assert!(
        matches!(err, SignerError::Timeout { .. }),
        "HTTP 408 must map to SignerError::Timeout, got: {err:?}"
    );
}
