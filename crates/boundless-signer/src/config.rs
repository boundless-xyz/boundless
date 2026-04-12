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

//! `SignerConfig` TOML schema and `from_config()` factory.
//!
//! Priority chain (per D-004):
//!
//! 1. `[signer.<role>]` explicit config block
//! 2. `[signer]` shared fallback block
//! 3. Env var `SIGNER_<ROLE>_BACKEND` or `SIGNER_BACKEND`
//! 4. Key env var fallback: `PROVER_PRIVATE_KEY` → local(Prover),
//!    `REWARD_PRIVATE_KEY` → local(Rewards)

use alloy::{network::Ethereum, providers::Provider};
use serde::{Deserialize, Serialize};

use crate::{
    backend::GenericSigner,
    local::LocalSignerBackend,
    types::{SignerError, SignerRole},
};

/// Per-role HTTP settings.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HttpSignerConfig {
    /// Base URL of the fleet-node fleet-RPC (e.g. `http://localhost:3100`).
    pub url: String,
}

/// Settings for one signing role.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RoleSignerConfig {
    /// Which backend to use: `"local"` or `"http-remote"`.
    pub backend: Option<String>,
    /// Name of the environment variable that holds the private key hex string.
    /// Defaults to `PROVER_PRIVATE_KEY` (Prover) or `REWARD_PRIVATE_KEY` (Rewards).
    pub private_key_env: Option<String>,
    /// HTTP-remote settings (only relevant when `backend = "http-remote"`).
    pub http: Option<HttpSignerConfig>,
}

/// Top-level `[signer]` section of the broker / CLI TOML config.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignerConfig {
    /// Shared fallback settings (apply to all roles that don't have a
    /// role-specific override).
    pub backend: Option<String>,
    pub private_key_env: Option<String>,
    pub http: Option<HttpSignerConfig>,

    /// Prover-role specific overrides.
    pub prover: Option<RoleSignerConfig>,
    /// Rewards-role specific overrides.
    pub rewards: Option<RoleSignerConfig>,
}

impl SignerConfig {
    /// Resolve the effective `RoleSignerConfig` for `role`, merging role-specific
    /// settings over shared defaults.
    fn resolve_for_role(&self, role: SignerRole) -> RoleSignerConfig {
        let role_cfg = match role {
            SignerRole::Prover => self.prover.as_ref(),
            SignerRole::Rewards => self.rewards.as_ref(),
        };

        RoleSignerConfig {
            backend: role_cfg
                .and_then(|r| r.backend.clone())
                .or_else(|| self.backend.clone()),
            private_key_env: role_cfg
                .and_then(|r| r.private_key_env.clone())
                .or_else(|| self.private_key_env.clone()),
            http: role_cfg.and_then(|r| r.http.clone()).or_else(|| self.http.clone()),
        }
    }
}

/// Construct a [`GenericSigner`] for `role` using the configuration
/// priority chain.
///
/// `provider` is only used for the `local` backend (for transaction
/// submission).  For `http-remote`, the fleet-node handles submission and no
/// local provider is needed.
pub async fn from_config<P>(
    config: &SignerConfig,
    role: SignerRole,
    provider: P,
) -> Result<GenericSigner, SignerError>
where
    P: Provider<Ethereum> + Send + Sync + 'static,
{
    let resolved = config.resolve_for_role(role);

    // --- 1. Explicit backend name from config or env var ---
    let backend_name = resolved
        .backend
        .clone()
        .or_else(|| {
            // SIGNER_PROVER_BACKEND / SIGNER_REWARDS_BACKEND
            let role_key = format!("SIGNER_{}_BACKEND", role.to_string().to_uppercase());
            std::env::var(&role_key).ok()
        })
        .or_else(|| std::env::var("SIGNER_BACKEND").ok());

    if let Some(ref name) = backend_name {
        match name.as_str() {
            "local" => return build_local(resolved, role, provider).await,
            "http-remote" => return build_http_remote(resolved, role).await,
            other => {
                return Err(SignerError::Transport {
                    message: format!("unknown signer backend '{other}' (expected 'local' or 'http-remote')"),
                });
            }
        }
    }

    // --- 4. Key env var fallback ---
    let key_env = match role {
        SignerRole::Prover => {
            resolved.private_key_env.as_deref().unwrap_or("PROVER_PRIVATE_KEY")
        }
        SignerRole::Rewards => {
            resolved.private_key_env.as_deref().unwrap_or("REWARD_PRIVATE_KEY")
        }
    };

    if let Ok(key_hex) = std::env::var(key_env) {
        if !key_hex.is_empty() {
            tracing::debug!(
                "signer backend for role {role}: using local key from {key_env}"
            );
            return Ok(GenericSigner::Local(LocalSignerBackend::new(&key_hex, provider)?));
        }
    }

    // Also check PROVER_PRIVATE_KEY / PRIVATE_KEY legacy aliases for Prover
    if role == SignerRole::Prover {
        if let Ok(key_hex) = std::env::var("PRIVATE_KEY") {
            if !key_hex.is_empty() {
                tracing::debug!("signer backend for role {role}: using local key from PRIVATE_KEY (legacy)");
                return Ok(GenericSigner::Local(LocalSignerBackend::new(&key_hex, provider)?));
            }
        }
    }

    Err(SignerError::Unavailable {
        message: format!(
            "no signer configured for role '{role}': set {key_env} or add [signer.{role}] to config",
        ),
    })
}

// ── Private helpers ───────────────────────────────────────────────────────────

async fn build_local<P>(
    cfg: RoleSignerConfig,
    role: SignerRole,
    provider: P,
) -> Result<GenericSigner, SignerError>
where
    P: Provider<Ethereum> + Send + Sync + 'static,
{
    let key_env = match role {
        SignerRole::Prover => {
            cfg.private_key_env.as_deref().unwrap_or("PROVER_PRIVATE_KEY").to_owned()
        }
        SignerRole::Rewards => {
            cfg.private_key_env.as_deref().unwrap_or("REWARD_PRIVATE_KEY").to_owned()
        }
    };

    let key_hex = std::env::var(&key_env).map_err(|_| SignerError::Unavailable {
        message: format!("local signer for role '{role}' requires {key_env} to be set"),
    })?;

    Ok(GenericSigner::Local(LocalSignerBackend::new(&key_hex, provider)?))
}

#[cfg(feature = "http-remote")]
async fn build_http_remote(
    cfg: RoleSignerConfig,
    role: SignerRole,
) -> Result<GenericSigner, SignerError> {
    use crate::http_remote::HttpRemoteSignerBackend;

    // URL resolution priority:
    //   1. [signer.<role>.http] url  (TOML)
    //   2. SIGNER_<ROLE>_HTTP_URL   (e.g. SIGNER_PROVER_HTTP_URL)
    //   3. SIGNER_HTTP_URL          (shared fallback)
    let url = cfg
        .http
        .as_ref()
        .map(|h| h.url.clone())
        .filter(|u| !u.is_empty())
        .or_else(|| {
            let role_key = format!("SIGNER_{}_HTTP_URL", role.to_string().to_uppercase());
            std::env::var(&role_key).ok().filter(|u| !u.is_empty())
        })
        .or_else(|| std::env::var("SIGNER_HTTP_URL").ok().filter(|u| !u.is_empty()))
        .ok_or_else(|| SignerError::Unavailable {
            message: format!(
                "http-remote backend for role '{role}' requires a URL: \
                 set [signer.{role}.http] url in config, \
                 SIGNER_{role_upper}_HTTP_URL, or SIGNER_HTTP_URL",
                role_upper = role.to_string().to_uppercase(),
            ),
        })?;

    Ok(GenericSigner::Remote(HttpRemoteSignerBackend::new(&url, role).await?))
}

#[cfg(not(feature = "http-remote"))]
async fn build_http_remote(
    _cfg: RoleSignerConfig,
    role: SignerRole,
) -> Result<GenericSigner, SignerError> {
    Err(SignerError::Transport {
        message: format!(
            "http-remote backend for role '{role}' requires the 'http-remote' crate feature",
        ),
    })
}
