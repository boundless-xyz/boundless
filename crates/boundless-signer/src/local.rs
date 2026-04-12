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

//! LocalSignerBackend: wraps an alloy PrivateKeySigner and a Provider.
//!
//! This is the current "always worked" path.  Byte-identical behaviour to the
//! existing direct-PrivateKeySigner usage is the hard contract.

use std::sync::Arc;

use alloy::{
    consensus::{SignableTransaction, TypedTransaction},
    network::{Ethereum, TransactionBuilder},
    primitives::{Address, Signature, TxHash},
    providers::{DynProvider, Provider},
    rpc::types::TransactionRequest,
    signers::{local::PrivateKeySigner, Signer},
};
use async_trait::async_trait;

use crate::{
    traits::{SignerBackend, SignerMode},
    types::{SignerError, TransactionIntent},
};

/// Signing backend backed by an in-process private key.
///
/// `send_transaction` delegates to the provider's full filler pipeline
/// (nonce, gas, chain-id) so the behaviour is byte-identical to the
/// existing `EthereumWallet`-in-provider path.
#[derive(Clone)]
pub struct LocalSignerBackend {
    wallet: PrivateKeySigner,
    address: Address,
    /// A provider that already has the EthereumWallet filler configured with
    /// `wallet` above.  Used for nonce management, gas estimation, tx
    /// submission, and waiting for receipts.
    provider: Arc<DynProvider<Ethereum>>,
}

impl LocalSignerBackend {
    /// Create a new backend from a hex private key string and a wallet-enabled
    /// provider.
    ///
    /// `provider` must have an `EthereumWallet` configured for `wallet`'s
    /// address (i.e. created via `ProviderBuilder::new().wallet(wallet).…`).
    /// The provider's existing nonce, gas, and chain-id fillers are reused to
    /// preserve byte-identical transaction behaviour.
    pub fn new<P>(private_key: &str, provider: P) -> Result<Self, SignerError>
    where
        P: Provider<Ethereum> + Send + Sync + 'static,
    {
        let wallet: PrivateKeySigner = private_key
            .parse()
            .map_err(|e: alloy::signers::local::LocalSignerError| SignerError::Transport {
                message: format!("invalid private key: {e}"),
            })?;
        let address = wallet.address();
        let dyn_provider = DynProvider::new(provider);
        Ok(Self { wallet, address, provider: Arc::new(dyn_provider) })
    }

    /// Create from an already-parsed `PrivateKeySigner`.
    pub fn from_signer<P>(wallet: PrivateKeySigner, provider: P) -> Self
    where
        P: Provider<Ethereum> + Send + Sync + 'static,
    {
        let address = wallet.address();
        let dyn_provider = DynProvider::new(provider);
        Self { wallet, address, provider: Arc::new(dyn_provider) }
    }

    /// Return the underlying `PrivateKeySigner`.
    ///
    /// Useful when callers need an `impl alloy::signers::Signer` directly
    /// (e.g. `deposit_collateral_with_permit`).
    pub fn as_signer(&self) -> &PrivateKeySigner {
        &self.wallet
    }
}

#[async_trait]
impl SignerBackend for LocalSignerBackend {
    fn sender_address(&self) -> Address {
        self.address
    }

    async fn sign_transaction(
        &self,
        tx: &TypedTransaction,
    ) -> Result<Signature, SignerError> {
        let hash = tx.signature_hash();
        self.wallet
            .sign_hash(&hash)
            .await
            .map_err(|e| SignerError::Transport { message: e.to_string() })
    }

    async fn send_transaction(&self, intent: TransactionIntent) -> Result<TxHash, SignerError> {
        let mut tx = TransactionRequest::default()
            .from(self.address)
            .with_to(intent.to)
            .with_input(intent.data.clone())
            .with_value(intent.value)
            .with_chain_id(intent.chain_id);

        if let Some(gas) = intent.gas_limit {
            tx = tx.with_gas_limit(gas);
        }

        let pending = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|e| SignerError::BroadcastFailed { message: e.to_string() })?;

        let tx_hash = pending
            .watch()
            .await
            .map_err(|e| SignerError::BroadcastFailed { message: e.to_string() })?;

        Ok(tx_hash)
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        self.wallet
            .sign_message(message)
            .await
            .map_err(|e| SignerError::Transport { message: e.to_string() })
    }

    fn supported_mode(&self) -> SignerMode {
        SignerMode::Both
    }

    fn name(&self) -> &str {
        "local"
    }
}
