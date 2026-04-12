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

//! SignerBackend trait and SignerMode enum.

use alloy::primitives::{Address, Signature, TxHash, B256};
use async_trait::async_trait;

use crate::types::{SignerError, TransactionIntent};

/// Which transaction-signing modes a backend supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerMode {
    /// Backend can sign a transaction and return the raw signature (Mode A).
    /// The caller is responsible for broadcasting.
    SignOnly,
    /// Backend signs AND broadcasts the transaction, returning the tx hash (Mode B).
    /// The caller does not need a provider for submission.
    SendOnly,
    /// Backend supports both Mode A and Mode B.
    Both,
}

/// Abstraction over transaction and message signing.
///
/// Implementations:
/// - [`crate::LocalSignerBackend`] – wraps an alloy `PrivateKeySigner`; zero-regression path.
/// - [`crate::HttpRemoteSignerBackend`] – delegates to fleet-node `POST /sign`.
///
/// All implementations must be `Send + Sync` so they can be shared across tokio tasks via
/// `Arc<dyn SignerBackend>`.
#[async_trait]
pub trait SignerBackend: Send + Sync + std::fmt::Debug {
    /// The Ethereum address that corresponds to the signing key.
    fn sender_address(&self) -> Address;

    /// Sign a pre-built transaction and return the signature (Mode A).
    ///
    /// The caller is responsible for broadcasting.  Returns
    /// [`SignerError::ModeNotSupported`] for `SendOnly` backends.
    async fn sign_transaction(
        &self,
        tx: &alloy::consensus::TypedTransaction,
    ) -> Result<Signature, SignerError>;

    /// Sign and broadcast a transaction described by `intent` (Mode B).
    ///
    /// The backend selects nonce, gas price, etc., signs, and broadcasts.
    /// Returns the transaction hash.  Returns [`SignerError::ModeNotSupported`]
    /// for `SignOnly` backends.
    async fn send_transaction(
        &self,
        intent: TransactionIntent,
    ) -> Result<TxHash, SignerError>;

    /// Sign an arbitrary off-chain message (EIP-191 personal_sign).
    ///
    /// Always available on every backend.
    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError>;

    /// Sign a raw 32-byte hash (EIP-191 / EIP-712).
    ///
    /// Used by [`crate::SignerBackendBridge`] to satisfy `alloy::signers::Signer`
    /// for callers that take `&impl Signer` (e.g. `Permit::sign`, `sign_request`,
    /// WebSocket `connect_async`).
    async fn sign_hash(&self, hash: B256) -> Result<Signature, SignerError>;

    /// Which signing modes this backend supports.
    fn supported_mode(&self) -> SignerMode;

    /// A short human-readable name for logging (e.g. `"local"`, `"http-remote"`).
    fn name(&self) -> &str;
}
