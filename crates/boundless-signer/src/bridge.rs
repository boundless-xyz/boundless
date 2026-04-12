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

//! [`SignerBackendBridge`]: adapts any [`SignerBackend`] to `alloy::signers::Signer`.
//!
//! Needed wherever alloy takes `&impl Signer`:
//! - `Permit::sign()` (EIP-2612 collateral / staking permits)
//! - `ProofRequest::sign_request()` (EIP-712 order signing)
//! - `order_stream::Client::connect_async()` (WebSocket EIP-191 auth)

use std::sync::Arc;

use alloy::{
    consensus::SignableTransaction,
    network::TxSigner,
    primitives::{Address, ChainId, Signature, B256},
    signers::{self, Signer},
};
use async_trait::async_trait;

use crate::{backend::ConcreteSignerBackend, traits::SignerBackend};

/// Wraps an `Arc<ConcreteSignerBackend>` and implements `alloy::signers::Signer`.
///
/// # Usage
///
/// ```rust,ignore
/// let bridge = SignerBackendBridge::new(backend.clone());
/// permit.sign(&bridge, domain_separator).await?;
/// ```
#[derive(Clone)]
pub struct SignerBackendBridge {
    inner: Arc<ConcreteSignerBackend>,
    chain_id: Option<ChainId>,
}

impl SignerBackendBridge {
    /// Create a bridge with no chain ID set.
    pub fn new(backend: Arc<ConcreteSignerBackend>) -> Self {
        Self { inner: backend, chain_id: None }
    }

    /// Create a bridge with a specific chain ID pre-set.
    pub fn new_with_chain(backend: Arc<ConcreteSignerBackend>, chain_id: u64) -> Self {
        Self { inner: backend, chain_id: Some(chain_id) }
    }
}

#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl Signer for SignerBackendBridge {
    async fn sign_hash(&self, hash: &B256) -> signers::Result<Signature> {
        self.inner
            .sign_hash(*hash)
            .await
            .map_err(|e| signers::Error::other(Box::new(e)))
    }

    fn address(&self) -> Address {
        self.inner.sender_address()
    }

    fn chain_id(&self) -> Option<ChainId> {
        self.chain_id
    }

    fn set_chain_id(&mut self, chain_id: Option<ChainId>) {
        self.chain_id = chain_id;
    }
}

/// Implement `TxSigner<Signature>` so that `SignerBackendBridge` can be wrapped
/// in an `EthereumWallet` and used as the provider's wallet filler.
///
/// The chain_id guard is intentionally omitted: for EIP-1559 and EIP-2930
/// transactions the chain_id is already encoded in the transaction itself and
/// reflected in `signature_hash()`.  Legacy EIP-155 replay protection is
/// likewise handled inside `signature_hash()`.
#[cfg_attr(target_family = "wasm", async_trait(?Send))]
#[cfg_attr(not(target_family = "wasm"), async_trait)]
impl TxSigner<Signature> for SignerBackendBridge {
    fn address(&self) -> Address {
        self.inner.sender_address()
    }

    async fn sign_transaction(
        &self,
        tx: &mut dyn SignableTransaction<Signature>,
    ) -> signers::Result<Signature> {
        let hash = tx.signature_hash();
        self.inner
            .sign_hash(hash)
            .await
            .map_err(|e| signers::Error::other(Box::new(e)))
    }
}
