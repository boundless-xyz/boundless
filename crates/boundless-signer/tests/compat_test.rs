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

//! Parity tests: `LocalSignerBackend` (direct and via `SignerBackendBridge`)
//! must produce byte-identical signatures to `PrivateKeySigner` for every
//! signing surface used in production.
//!
//! Run with: `cargo test -p boundless-signer`

use std::sync::Arc;

use alloy::{
    consensus::{TxEip1559, TxLegacy},
    network::TxSigner,
    primitives::{address, bytes, Bytes, TxKind, B256, U256},
    providers::ProviderBuilder,
    signers::{local::PrivateKeySigner, Signer},
};
use boundless_signer::{ConcreteSignerBackend, LocalSignerBackend, SignerBackend, SignerBackendBridge};

/// Anvil account 0 — deterministic, well-known.
const TEST_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Build a `ConcreteSignerBackend::Local` using an offline HTTP provider.
/// Signing methods never touch the provider; the URL is never contacted.
fn make_backend(wallet: PrivateKeySigner) -> Arc<ConcreteSignerBackend> {
    let provider =
        ProviderBuilder::new().connect_http("http://127.0.0.1:8545".parse().expect("valid url"));
    Arc::new(ConcreteSignerBackend::Local(LocalSignerBackend::from_signer(wallet, provider)))
}

// ── sign_hash ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sign_hash_parity() {
    let hash = B256::from([0x42u8; 32]);
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let sig_direct = wallet.sign_hash(&hash).await.unwrap();
    let backend = make_backend(wallet);
    let sig_backend = backend.sign_hash(hash).await.unwrap();

    assert_eq!(sig_direct, sig_backend, "sign_hash: local backend must be byte-identical to direct PrivateKeySigner");
}

#[tokio::test]
async fn sign_hash_via_bridge_parity() {
    let hash = B256::from([0xABu8; 32]);
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let sig_direct = wallet.sign_hash(&hash).await.unwrap();
    let bridge = SignerBackendBridge::new(make_backend(wallet));
    let sig_bridge = bridge.sign_hash(&hash).await.unwrap();

    assert_eq!(sig_direct, sig_bridge, "sign_hash via bridge must equal direct PrivateKeySigner");
}

// ── sign_message ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn sign_message_parity() {
    let msg = b"Hello, Boundless!";
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let sig_direct = wallet.sign_message(msg).await.unwrap();
    let backend = make_backend(wallet);
    let sig_backend = backend.sign_message(msg).await.unwrap();

    assert_eq!(sig_direct, sig_backend, "sign_message: local backend must be byte-identical");
}

#[tokio::test]
async fn sign_message_via_bridge_parity() {
    let msg = b"EIP-191 test message";
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let sig_direct = wallet.sign_message(msg).await.unwrap();
    let bridge = SignerBackendBridge::new(make_backend(wallet));
    let sig_bridge = bridge.sign_message(msg).await.unwrap();

    assert_eq!(sig_direct, sig_bridge, "sign_message via bridge must equal direct PrivateKeySigner");
}

// ── TxSigner::sign_transaction — EIP-1559 ────────────────────────────────────

fn eip1559_tx() -> TxEip1559 {
    TxEip1559 {
        chain_id: 1,
        nonce: 7,
        max_fee_per_gas: 2_000_000_000,
        max_priority_fee_per_gas: 1_000_000_000,
        gas_limit: 21_000,
        to: TxKind::Call(address!("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045")),
        value: U256::from(1_000_000_000_000_000_u64), // 0.001 ETH
        input: Bytes::new(),
        access_list: Default::default(),
    }
}

#[tokio::test]
async fn tx_sign_eip1559_parity() {
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let mut tx_direct = eip1559_tx();
    let mut tx_bridge = eip1559_tx();

    let sig_direct = wallet.sign_transaction(&mut tx_direct).await.unwrap();
    let bridge = SignerBackendBridge::new(make_backend(wallet));
    let sig_bridge = bridge.sign_transaction(&mut tx_bridge).await.unwrap();

    assert_eq!(
        sig_direct, sig_bridge,
        "EIP-1559 TxSigner::sign_transaction via bridge must equal direct PrivateKeySigner"
    );
}

// ── TxSigner::sign_transaction — legacy (EIP-155) ────────────────────────────

fn legacy_tx() -> TxLegacy {
    TxLegacy {
        chain_id: Some(1),
        nonce: 3,
        gas_price: 20_000_000_000,
        gas_limit: 21_000,
        to: TxKind::Call(address!("0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045")),
        value: U256::from(500_000_000_000_000_u64),
        input: bytes!("deadbeef"),
    }
}

#[tokio::test]
async fn tx_sign_legacy_parity() {
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();

    let mut tx_direct = legacy_tx();
    let mut tx_bridge = legacy_tx();

    let sig_direct = wallet.sign_transaction(&mut tx_direct).await.unwrap();
    let bridge = SignerBackendBridge::new(make_backend(wallet));
    let sig_bridge = bridge.sign_transaction(&mut tx_bridge).await.unwrap();

    assert_eq!(
        sig_direct, sig_bridge,
        "Legacy (EIP-155) TxSigner::sign_transaction via bridge must equal direct PrivateKeySigner"
    );
}

// ── EIP-712 hash (permit / collateral path) ───────────────────────────────────

#[tokio::test]
async fn eip712_hash_parity() {
    // Simulate an EIP-712 typed-data hash: keccak256(domain_sep ++ struct_hash)
    // Using fixed bytes to keep the test deterministic and dependency-free.
    let domain_sep = B256::from([0xD0u8; 32]);
    let struct_hash = B256::from([0x57u8; 32]);
    // eip712 hash = keccak256(0x1901 ++ domain_sep ++ struct_hash)
    let mut encoded = Vec::with_capacity(2 + 32 + 32);
    encoded.extend_from_slice(&[0x19, 0x01]);
    encoded.extend_from_slice(domain_sep.as_slice());
    encoded.extend_from_slice(struct_hash.as_slice());
    let eip712_hash = alloy::primitives::keccak256(&encoded);

    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();
    let sig_direct = wallet.sign_hash(&eip712_hash).await.unwrap();

    let bridge = SignerBackendBridge::new(make_backend(wallet));
    let sig_bridge = bridge.sign_hash(&eip712_hash).await.unwrap();

    assert_eq!(sig_direct, sig_bridge, "EIP-712 hash signing via bridge must equal direct PrivateKeySigner");
}

// ── sender_address consistency ────────────────────────────────────────────────

#[tokio::test]
async fn sender_address_consistent() {
    let wallet: PrivateKeySigner = TEST_KEY.parse().unwrap();
    let expected_addr = wallet.address();

    let backend = make_backend(wallet.clone());
    let bridge = SignerBackendBridge::new(backend.clone());

    assert_eq!(backend.sender_address(), expected_addr);
    assert_eq!(Signer::address(&bridge), expected_addr);
}
