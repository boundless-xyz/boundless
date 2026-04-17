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

//! Core types: TransactionIntent, SignerRole, SignerError.

use alloy::primitives::{Address, Bytes, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Unsigned transaction parameters forwarded to a SignerBackend.
///
/// Deliberately omits nonce, gas-price and signature fields – the backend
/// fills or overrides those as appropriate for its signing environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionIntent {
    /// Recipient address (contract being called).
    pub to: Address,
    /// Encoded call-data.
    pub data: Bytes,
    /// ETH value to transfer (usually 0 for market calls).
    pub value: U256,
    /// Advisory gas limit hint; the backend may re-estimate.
    pub gas_limit: Option<u64>,
    /// Chain ID – mandatory for routing and safety checks.
    pub chain_id: u64,
}

/// Which protocol role the signing key represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SignerRole {
    /// Market / lock / fulfill wallet on Base L2 (chain 8453).
    Prover,
    /// Rewards / staking wallet on Ethereum L1 (chain 1).
    Rewards,
}

impl std::fmt::Display for SignerRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerRole::Prover => write!(f, "prover"),
            SignerRole::Rewards => write!(f, "rewards"),
        }
    }
}

/// Errors returned by a [`crate::SignerBackend`].
#[derive(Debug, Error)]
pub enum SignerError {
    /// The backend does not support the requested signing mode.
    #[error("signing mode not supported by backend")]
    ModeNotSupported,

    /// The remote signer (e.g. fleet-node / user wallet) refused the request.
    #[error("signing rejected: {reason}")]
    Rejected { reason: String },

    /// The signing request timed out.
    #[error("signing timed out after {elapsed_secs}s")]
    Timeout { elapsed_secs: u64 },

    /// The signer is temporarily unavailable (e.g. fleet-node offline).
    #[error("signer unavailable: {message}")]
    Unavailable { message: String },

    /// Transaction was signed but the broadcast failed.
    #[error("broadcast failed: {message}")]
    BroadcastFailed { message: String },

    /// Low-level transport or HTTP error.
    #[error("transport error: {message}")]
    Transport { message: String },
}
