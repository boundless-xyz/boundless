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

//! Minimum broker version enforcement via on-chain VersionRegistry contract.
//!
//! The broker checks the on-chain minimum version at startup and periodically
//! via a supervised background task. If the broker version is below the minimum,
//! the supervisor faults, triggering graceful shutdown. If the contract cannot
//! be read (RPC error, not deployed, etc.), it logs a warning and continues.

use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::sol;
use std::time::Duration;
use thiserror::Error;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::{
    errors::{impl_coded_debug, CodedError},
    is_dev_mode,
    task::{RetryRes, RetryTask, SupervisorErr},
};

// Minimal ABI binding for the VersionRegistry contract.
sol! {
    #[sol(rpc)]
    interface IVersionRegistry {
        function getVersionInfo() external view returns (uint64 minimumVersion, string notice);
    }
}

/// Hardcoded VersionRegistry contract addresses per chain ID.
/// NOT configurable to prevent bypass.
const VERSION_REGISTRIES: &[(u64, Address)] = &[
    // TODO: Entries will be added after deployment:
    // (1, address!("0x...")),        // ETH Mainnet
    // (8453, address!("0x...")),     // Base
    // (11155111, address!("0x...")), // Sepolia
    // (84532, address!("0x...")),    // Base Sepolia
];

fn lookup_registry(chain_id: u64) -> Option<Address> {
    VERSION_REGISTRIES.iter().find(|(id, _)| *id == chain_id).map(|(_, addr)| *addr)
}

/// Pack a semver triplet into a single u64 for comparison.
/// Encoding: (major << 32) | (minor << 16) | patch
pub const fn pack_version(major: u16, minor: u16, patch: u16) -> u64 {
    ((major as u64) << 32) | ((minor as u64) << 16) | (patch as u64)
}

const fn const_str_to_u16(s: &str) -> u16 {
    let bytes = s.as_bytes();
    let mut result: u16 = 0;
    let mut i = 0;
    while i < bytes.len() {
        result = result * 10 + (bytes[i] - b'0') as u16;
        i += 1;
    }
    result
}

/// Broker version packed at compile time from Cargo.toml.
pub const BROKER_VERSION: u64 = pack_version(
    const_str_to_u16(env!("CARGO_PKG_VERSION_MAJOR")),
    const_str_to_u16(env!("CARGO_PKG_VERSION_MINOR")),
    const_str_to_u16(env!("CARGO_PKG_VERSION_PATCH")),
);

/// Unpack a u64 version into semver components.
pub fn unpack_version(version: u64) -> (u16, u16, u16) {
    let major = (version >> 32) as u16;
    let minor = (version >> 16) as u16;
    let patch = version as u16;
    (major, minor, patch)
}

/// Format a packed version as a human-readable string (e.g. "1.2.3").
pub fn format_version(version: u64) -> String {
    let (major, minor, patch) = unpack_version(version);
    format!("{major}.{minor}.{patch}")
}

#[derive(Error)]
pub(crate) enum VersionCheckError {
    #[error(
        "{code} Broker version {broker_version} is below the on-chain minimum \
         version {min_version} on chain {chain_id}. \
         Please upgrade to at least version {min_version}. \
         See https://docs.boundless.network/ for instructions.", code = self.code()
    )]
    BelowMinimum { broker_version: String, min_version: String, chain_id: u64 },
}

impl_coded_debug!(VersionCheckError);
impl CodedError for VersionCheckError {
    fn code(&self) -> &str {
        match self {
            VersionCheckError::BelowMinimum { .. } => "[B-VER-001]",
        }
    }
}

pub(crate) struct VersionCheckTask<P> {
    /// RPC provider for reading the VersionRegistry contract.
    provider: P,
    /// Chain ID for logging and registry lookup.
    chain_id: u64,
    /// Packed broker version for comparison with on-chain minimum. Defaults to BROKER_VERSION.
    broker_version: u64,
    /// Resolved VersionRegistry address for this chain. None means no registry is configured and
    /// the version check will be skipped.
    registry_address: Option<Address>,
    /// Polling interval for periodic version checks. Default is 10 minutes. Configurable for testing.
    poll_interval: Duration,
}

impl<P: Provider + Clone + Send + Sync + 'static> VersionCheckTask<P> {
    /// Create a new `VersionCheckTask`.
    ///
    /// - `broker_version`: packed version to enforce. Pass `None` to use the compile-time
    ///   `BROKER_VERSION` (the default for production).
    /// - `registry_address`: address of the VersionRegistry contract. Pass `None` to look up the
    ///   address from the hardcoded chain table (the default for production).
    pub(crate) fn new(
        provider: P,
        chain_id: u64,
        broker_version: Option<u64>,
        registry_address: Option<Address>,
    ) -> Self {
        Self {
            provider,
            chain_id,
            broker_version: broker_version.unwrap_or(BROKER_VERSION),
            registry_address: registry_address.or_else(|| lookup_registry(chain_id)),
            poll_interval: Duration::from_secs(600),
        }
    }
}

async fn check_version<P: Provider>(
    provider: &P,
    registry_address: Address,
    chain_id: u64,
    broker_version: u64,
) -> Result<(), SupervisorErr<VersionCheckError>> {
    let registry = IVersionRegistry::new(registry_address, provider);

    let (min_version, notice) = match registry.getVersionInfo().call().await {
        Ok(v) => (v.minimumVersion, v.notice),
        Err(e) => {
            tracing::warn!(chain_id, error = %e, "Failed to read VersionRegistry. Continuing.");
            return Ok(());
        }
    };

    if !notice.is_empty() {
        tracing::warn!(chain_id, notice, "VersionRegistry notice");
    }

    // Returning SupervisorErr::Fault triggers graceful shutdown of the broker.
    if broker_version < min_version {
        return Err(SupervisorErr::Fault(VersionCheckError::BelowMinimum {
            broker_version: format_version(broker_version),
            min_version: format_version(min_version),
            chain_id,
        }));
    }

    tracing::info!(
        chain_id,
        broker_version = format_version(broker_version),
        minimum_version = format_version(min_version),
        "Version check passed"
    );
    Ok(())
}

impl<P: Provider + Clone + Send + Sync + 'static> RetryTask for VersionCheckTask<P> {
    type Error = VersionCheckError;

    fn spawn(&self, cancel_token: CancellationToken) -> RetryRes<Self::Error> {
        let provider = self.provider.clone();
        let chain_id = self.chain_id;
        let broker_version = self.broker_version;
        let registry_address = self.registry_address;
        let poll_interval = self.poll_interval;

        Box::pin(async move {
            // Skipping the version check in dev mode is safe because RISC0_DEV_MODE
            // disables real proof generation entirely — the broker produces fake receipts
            // that are rejected by on-chain verifiers. A prover cannot use this flag to
            // bypass only the version check while remaining operational in production.
            if is_dev_mode() {
                tracing::info!("Skipping version check (RISC0_DEV_MODE enabled)");
                cancel_token.cancelled().await;
                return Ok(());
            }

            let registry_addr = match registry_address {
                Some(addr) => addr,
                None => {
                    tracing::info!(
                        chain_id,
                        "VersionRegistry address not configured for chain. Skipping version check."
                    );
                    cancel_token.cancelled().await;
                    return Ok(());
                }
            };

            // Because first tick completes immediately, we effectively do a version check on startup.
            let mut interval = tokio::time::interval(poll_interval);
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        check_version(&provider, registry_addr, chain_id, broker_version).await?;
                    }
                    _ = cancel_token.cancelled() => break,
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        providers::{ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
    };
    use std::sync::Arc;

    use crate::{config::ConfigLock, task::Supervisor};

    // --- Unit tests ---

    #[test]
    fn test_pack_version() {
        let expected = (2u64 << 32) | (3u64 << 16) | 1u64;
        assert_eq!(pack_version(2, 3, 1), expected);
        assert_eq!(pack_version(0, 0, 0), 0);
        assert_eq!(pack_version(1, 0, 0), 1u64 << 32);
    }

    #[test]
    fn test_unpack_version() {
        assert_eq!(unpack_version(0), (0, 0, 0));
        let packed = pack_version(2, 3, 1);
        assert_eq!(unpack_version(packed), (2, 3, 1));
    }

    #[test]
    fn test_pack_unpack_roundtrip() {
        let cases = [(0, 0, 0), (1, 0, 0), (0, 1, 0), (0, 0, 1), (2, 3, 1), (65535, 65535, 65535)];
        for (major, minor, patch) in cases {
            let packed = pack_version(major, minor, patch);
            assert_eq!(unpack_version(packed), (major, minor, patch));
        }
    }

    #[test]
    fn test_version_ordering() {
        assert!(pack_version(2, 0, 0) > pack_version(1, 65535, 65535));
        assert!(pack_version(1, 1, 0) > pack_version(1, 0, 65535));
        assert!(pack_version(0, 0, 1) > pack_version(0, 0, 0));
        assert_eq!(pack_version(1, 2, 3), pack_version(1, 2, 3));
    }

    #[test]
    fn test_format_version() {
        assert_eq!(format_version(pack_version(2, 3, 1)), "2.3.1");
        assert_eq!(format_version(pack_version(0, 0, 0)), "0.0.0");
        assert_eq!(format_version(pack_version(1, 0, 0)), "1.0.0");
    }

    #[test]
    fn test_lookup_registry_unknown_chain() {
        assert!(lookup_registry(999999).is_none());
        assert!(lookup_registry(0).is_none());
    }
}
