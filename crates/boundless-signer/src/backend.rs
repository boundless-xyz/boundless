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

//! [`GenericSigner`]: closed-set enum wrapping all known backend types.
//!
//! Implements both [`SignerBackend`] (our custom async trait) and
//! `alloy::signers::Signer` + `TxSigner<Signature>` directly, so it can be
//! used everywhere alloy expects `&impl Signer` or passed to
//! `EthereumWallet::from(...)` without any adapter type.
//!
//! Adding a new backend variant requires updating every `match` arm; the
//! compiler will catch any missed call site.

use alloy::{
    consensus::SignableTransaction,
    network::TxSigner,
    primitives::{Address, ChainId, Signature, TxHash, B256},
    signers::{self, Signer},
};
use async_trait::async_trait;

#[cfg(feature = "http-remote")]
use crate::http_remote::HttpRemoteSignerBackend;
use crate::{
    local::LocalSignerBackend,
    traits::{SignerBackend, SignerMode},
    types::{SignerError, TransactionIntent},
};

/// All known signing backends as a closed, clonable enum.
///
/// Store and share as `Arc<GenericSigner>`.  Pass a clone (`(*arc).clone()`)
/// wherever an owned `impl Signer` / `impl TxSigner` is consumed (e.g.
/// `EthereumWallet::from(...)`, `client_builder.with_signer(...)`).  Borrow
/// as `&*arc` wherever `&impl Signer` is expected.
#[derive(Debug, Clone)]
pub enum GenericSigner {
    /// In-process local key — zero-regression default path.
    Local(LocalSignerBackend),
    /// HTTP delegation to fleet-node `POST /sign`.
    #[cfg(feature = "http-remote")]
    Remote(HttpRemoteSignerBackend),
}

impl GenericSigner {
    /// Construct a local backend and wrap it in `Arc<GenericSigner>`.
    ///
    /// Equivalent to `Arc::new(GenericSigner::Local(LocalSignerBackend::from_signer(wallet, provider)))`.
    pub fn arc_local<P>(wallet: alloy::signers::local::PrivateKeySigner, provider: P) -> std::sync::Arc<Self>
    where
        P: alloy::providers::Provider<alloy::network::Ethereum> + Clone + 'static,
    {
        std::sync::Arc::new(Self::Local(LocalSignerBackend::from_signer(wallet, provider)))
    }

    /// Construct a local backend (not wrapped in `Arc`).
    pub fn local<P>(wallet: alloy::signers::local::PrivateKeySigner, provider: P) -> Self
    where
        P: alloy::providers::Provider<alloy::network::Ethereum> + Clone + 'static,
    {
        Self::Local(LocalSignerBackend::from_signer(wallet, provider))
    }

    /// Construct an HTTP-remote backend and wrap it in `Arc<GenericSigner>`.
    ///
    /// Equivalent to `Arc::new(GenericSigner::Remote(HttpRemoteSignerBackend::new(url, role).await?))`.
    #[cfg(feature = "http-remote")]
    pub async fn arc_remote(
        url: &str,
        role: crate::types::SignerRole,
    ) -> Result<std::sync::Arc<Self>, crate::types::SignerError> {
        Ok(std::sync::Arc::new(Self::Remote(
            HttpRemoteSignerBackend::new(url, role).await?,
        )))
    }
}

// ── SignerBackend (our custom trait) ──────────────────────────────────────────

#[async_trait]
impl SignerBackend for GenericSigner {
    fn sender_address(&self) -> Address {
        match self {
            Self::Local(b) => b.sender_address(),
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.sender_address(),
        }
    }

    async fn sign_transaction(
        &self,
        tx: &alloy::consensus::TypedTransaction,
    ) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_transaction(tx).await,
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.sign_transaction(tx).await,
        }
    }

    async fn send_transaction(&self, intent: TransactionIntent) -> Result<TxHash, SignerError> {
        match self {
            Self::Local(b) => b.send_transaction(intent).await,
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.send_transaction(intent).await,
        }
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_message(message).await,
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.sign_message(message).await,
        }
    }

    async fn sign_hash(&self, hash: B256) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_hash(hash).await,
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.sign_hash(hash).await,
        }
    }

    fn supported_mode(&self) -> SignerMode {
        match self {
            Self::Local(b) => b.supported_mode(),
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.supported_mode(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Local(b) => b.name(),
            #[cfg(feature = "http-remote")]
            Self::Remote(b) => b.name(),
        }
    }
}

// ── alloy::signers::Signer ────────────────────────────────────────────────────
//
// Implementing this directly on GenericSigner means Arc<GenericSigner> also
// satisfies `Signer` (via alloy's blanket `impl<S: Signer + ?Sized> Signer for Arc<S>`),
// eliminating any need for an adapter/bridge type.

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl Signer for GenericSigner {
    async fn sign_hash(&self, hash: &B256) -> signers::Result<Signature> {
        // Delegates to SignerBackend::sign_hash (B256 by-value variant).
        SignerBackend::sign_hash(self, *hash)
            .await
            .map_err(|e| signers::Error::other(Box::new(e)))
    }

    fn address(&self) -> Address {
        self.sender_address()
    }

    /// Chain ID is encoded in every transaction type and in `signature_hash()`;
    /// we don't need to cache it here.
    fn chain_id(&self) -> Option<ChainId> {
        None
    }

    fn set_chain_id(&mut self, _chain_id: Option<ChainId>) {
        // No-op: chain_id is carried by the transaction, not the signer.
    }
}

// ── alloy::network::TxSigner<Signature> ──────────────────────────────────────
//
// Required so `EthereumWallet::from(signer)` and `client_builder.with_signer(signer)`
// accept a `GenericSigner` directly.

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl TxSigner<Signature> for GenericSigner {
    fn address(&self) -> Address {
        self.sender_address()
    }

    async fn sign_transaction(
        &self,
        tx: &mut dyn SignableTransaction<Signature>,
    ) -> signers::Result<Signature> {
        let hash = tx.signature_hash();
        SignerBackend::sign_hash(self, hash)
            .await
            .map_err(|e| signers::Error::other(Box::new(e)))
    }
}
