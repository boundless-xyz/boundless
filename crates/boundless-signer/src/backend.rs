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

//! [`ConcreteSignerBackend`]: closed-set enum wrapping all known backend types.
//!
//! Implements [`SignerBackend`] via exhaustive `match`, providing static
//! dispatch for the compiler and exhaustiveness checking when new backends
//! are added.

use alloy::primitives::{Address, Signature, TxHash, B256};
use async_trait::async_trait;

#[cfg(feature = "http-remote")]
use crate::http_remote::HttpRemoteSignerBackend;
use crate::{
    local::LocalSignerBackend,
    traits::{SignerBackend, SignerMode},
    types::{SignerError, TransactionIntent},
};

/// All known signing backends as a closed enum.
///
/// Use this type everywhere a backend is stored or passed: `Arc<ConcreteSignerBackend>`.
/// Adding a new backend requires updating every `match` arm here — the compiler
/// will catch any missed site, unlike `Box<dyn SignerBackend>`.
#[derive(Debug, Clone)]
pub enum ConcreteSignerBackend {
    /// In-process local key (zero-regression default path).
    Local(LocalSignerBackend),
    /// HTTP delegation to fleet-node `POST /sign`.
    #[cfg(feature = "http-remote")]
    HttpRemote(HttpRemoteSignerBackend),
}

#[async_trait]
impl SignerBackend for ConcreteSignerBackend {
    fn sender_address(&self) -> Address {
        match self {
            Self::Local(b) => b.sender_address(),
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.sender_address(),
        }
    }

    async fn sign_transaction(
        &self,
        tx: &alloy::consensus::TypedTransaction,
    ) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_transaction(tx).await,
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.sign_transaction(tx).await,
        }
    }

    async fn send_transaction(
        &self,
        intent: TransactionIntent,
    ) -> Result<TxHash, SignerError> {
        match self {
            Self::Local(b) => b.send_transaction(intent).await,
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.send_transaction(intent).await,
        }
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_message(message).await,
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.sign_message(message).await,
        }
    }

    async fn sign_hash(&self, hash: B256) -> Result<Signature, SignerError> {
        match self {
            Self::Local(b) => b.sign_hash(hash).await,
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.sign_hash(hash).await,
        }
    }

    fn supported_mode(&self) -> SignerMode {
        match self {
            Self::Local(b) => b.supported_mode(),
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.supported_mode(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Local(b) => b.name(),
            #[cfg(feature = "http-remote")]
            Self::HttpRemote(b) => b.name(),
        }
    }
}
