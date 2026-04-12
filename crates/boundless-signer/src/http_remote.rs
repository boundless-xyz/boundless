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

//! HttpRemoteSignerBackend: delegates signing to the fleet-node `/sign` endpoint.
//!
//! Fleet-node owns the WalletConnect session, key custody, and policy.  Bento
//! is a dumb HTTP consumer: it POSTs an intent and receives a tx hash.
//!
//! Feature-gated behind `http-remote` to avoid pulling in `reqwest` for
//! operators that only use the local backend.

use std::time::Duration;

use alloy::{
    consensus::TypedTransaction,
    primitives::{Address, Signature, TxHash},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{
    traits::{SignerBackend, SignerMode},
    types::{SignerError, SignerRole, TransactionIntent},
};

// ── Fleet-node JSON contract types (DEF-009, DEF-010) ────────────────────────

#[derive(Serialize)]
struct SignRequest<'a> {
    #[serde(rename = "type")]
    request_type: SignRequestType,
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    intent: Option<TransactionIntentDto<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum SignRequestType {
    Transaction,
    Message,
}

#[derive(Serialize)]
struct TransactionIntentDto<'a> {
    to: String,
    data: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    gas_limit: Option<u64>,
    chain_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<&'a str>,
}

#[derive(Deserialize)]
struct SignTransactionResponse {
    tx_hash: String,
}

#[derive(Deserialize)]
struct SignMessageResponse {
    signature: String,
}

#[derive(Deserialize)]
struct CapabilitiesResponse {
    roles: std::collections::HashMap<String, RoleCapability>,
}

#[derive(Deserialize)]
struct RoleCapability {
    address: String,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    #[serde(default)]
    message: String,
}

// ── Backend implementation ────────────────────────────────────────────────────

/// Signing backend that forwards to fleet-node `POST /sign`.
///
/// Construction performs a `GET /sign/capabilities` call to discover the
/// wallet address; fail-fast if the fleet-node is unreachable at startup.
pub struct HttpRemoteSignerBackend {
    base_url: String,
    role: SignerRole,
    client: reqwest::Client,
    wallet_address: Address,
}

impl HttpRemoteSignerBackend {
    /// Default HTTP request timeout (matches DEF-008).
    pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

    /// Create a new backend, discovering the wallet address from
    /// `GET {base_url}/sign/capabilities`.
    pub async fn new(base_url: &str, role: SignerRole) -> Result<Self, SignerError> {
        Self::new_with_timeout(base_url, role, Duration::from_secs(Self::DEFAULT_TIMEOUT_SECS))
            .await
    }

    /// Create with a custom HTTP timeout (useful in tests).
    pub async fn new_with_timeout(
        base_url: &str,
        role: SignerRole,
        timeout: Duration,
    ) -> Result<Self, SignerError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| SignerError::Transport { message: e.to_string() })?;

        let capabilities_url = format!("{base_url}/sign/capabilities");
        let resp = client
            .get(&capabilities_url)
            .send()
            .await
            .map_err(|e| map_reqwest_err(e))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SignerError::Unavailable {
                message: format!("GET /sign/capabilities returned {status}: {body}"),
            });
        }

        let caps: CapabilitiesResponse =
            resp.json().await.map_err(|e| SignerError::Transport { message: e.to_string() })?;

        let role_key = role.to_string();
        let addr_str = caps
            .roles
            .get(&role_key)
            .map(|c| c.address.as_str())
            .ok_or_else(|| SignerError::Unavailable {
                message: format!("role '{role_key}' not in /sign/capabilities response"),
            })?;

        let wallet_address: Address =
            addr_str.parse().map_err(|e: alloy::hex::FromHexError| SignerError::Transport {
                message: format!("invalid address from capabilities: {e}"),
            })?;

        Ok(Self { base_url: base_url.to_owned(), role, client, wallet_address })
    }

    /// Returns the base URL (useful in tests / logging).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

#[async_trait]
impl SignerBackend for HttpRemoteSignerBackend {
    fn sender_address(&self) -> Address {
        self.wallet_address
    }

    async fn sign_transaction(&self, _tx: &TypedTransaction) -> Result<Signature, SignerError> {
        Err(SignerError::ModeNotSupported)
    }

    async fn send_transaction(&self, intent: TransactionIntent) -> Result<TxHash, SignerError> {
        let role_str = self.role.to_string();
        let to_str = intent.to.to_string();
        let data_str = format!("0x{}", hex::encode(&intent.data));
        let value_str = format!("0x{:x}", intent.value);

        let request_body = SignRequest {
            request_type: SignRequestType::Transaction,
            role: &role_str,
            intent: Some(TransactionIntentDto {
                to: to_str,
                data: data_str,
                value: value_str,
                gas_limit: intent.gas_limit,
                chain_id: intent.chain_id,
                from: None,
            }),
            message: None,
        };

        let url = format!("{}/sign", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(map_reqwest_err)?;

        let status = resp.status();
        match status.as_u16() {
            200 => {
                let body: SignTransactionResponse = resp
                    .json()
                    .await
                    .map_err(|e| SignerError::Transport { message: e.to_string() })?;
                let hash: TxHash =
                    body.tx_hash.trim_start_matches("0x").parse().map_err(|e: alloy::hex::FromHexError| {
                        SignerError::Transport { message: format!("invalid tx hash: {e}") }
                    })?;
                Ok(hash)
            }
            403 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::Rejected { reason: err })
            }
            408 => {
                Err(SignerError::Timeout { elapsed_secs: 0 })
            }
            502 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::BroadcastFailed { message: err })
            }
            503 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::Unavailable { message: err })
            }
            other => Err(SignerError::Transport {
                message: format!("unexpected status {other} from fleet-node"),
            }),
        }
    }

    async fn sign_message(&self, message: &[u8]) -> Result<Signature, SignerError> {
        let role_str = self.role.to_string();
        let message_hex = format!("0x{}", hex::encode(message));

        let request_body = SignRequest {
            request_type: SignRequestType::Message,
            role: &role_str,
            intent: None,
            message: Some(message_hex),
        };

        let url = format!("{}/sign", self.base_url);
        let resp = self
            .client
            .post(&url)
            .json(&request_body)
            .send()
            .await
            .map_err(map_reqwest_err)?;

        let status = resp.status();
        match status.as_u16() {
            200 => {
                let body: SignMessageResponse = resp
                    .json()
                    .await
                    .map_err(|e| SignerError::Transport { message: e.to_string() })?;
                let sig_bytes = alloy::hex::decode(
                    body.signature.trim_start_matches("0x"),
                )
                .map_err(|e| SignerError::Transport { message: format!("invalid signature hex: {e}") })?;
                let sig = Signature::try_from(sig_bytes.as_ref())
                    .map_err(|e| SignerError::Transport { message: format!("invalid signature bytes: {e}") })?;
                Ok(sig)
            }
            403 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::Rejected { reason: err })
            }
            408 => Err(SignerError::Timeout { elapsed_secs: 0 }),
            502 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::BroadcastFailed { message: err })
            }
            503 => {
                let err = parse_error_body(resp).await;
                Err(SignerError::Unavailable { message: err })
            }
            other => Err(SignerError::Transport {
                message: format!("unexpected status {other} from fleet-node"),
            }),
        }
    }

    fn supported_mode(&self) -> SignerMode {
        SignerMode::SendOnly
    }

    fn name(&self) -> &str {
        "http-remote"
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn map_reqwest_err(e: reqwest::Error) -> SignerError {
    if e.is_connect() || e.is_timeout() {
        SignerError::Transport { message: e.to_string() }
    } else {
        SignerError::Transport { message: e.to_string() }
    }
}

async fn parse_error_body(resp: reqwest::Response) -> String {
    resp.json::<ErrorResponse>()
        .await
        .map(|e| {
            if e.message.is_empty() {
                e.error
            } else {
                format!("{}: {}", e.error, e.message)
            }
        })
        .unwrap_or_else(|_| "unknown error".to_owned())
}
