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

use crate::{
    errors::{impl_coded_debug, CodedError},
    is_dev_mode,
    task::{RetryRes, RetryTask, SupervisorErr},
};
use alloy::primitives::{address, Address};
use alloy::providers::Provider;
use alloy::sol;
use boundless_market::deployments::{BASE, BASE_SEPOLIA};
use std::time::Duration;
use thiserror::Error;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

// Minimal ABI binding for the VersionRegistry contract.
sol! {
    #[sol(rpc)]
    interface IVersionRegistry {
        function getVersionInfo() external view returns (uint64 minimumVersion, string notice);
    }
}

/// Hardcoded VersionRegistry contract addresses keyed by (chain_id, market_address).
/// Keying on market_address disambiguates multiple deployments on the same chain
/// (e.g. staging and prod testnet both on Base Sepolia, chain ID 84532).
/// NOT configurable to prevent bypass.
const VERSION_REGISTRIES: &[(u64, Address, Address)] = &[
    // (chain_id, boundless_market_address, version_registry_address)
    (
        BASE_SEPOLIA.market_chain_id.unwrap(),
        BASE_SEPOLIA.boundless_market_address,
        address!("0xe359782297d569f1c3cdc02a50217b2b142e27fd"),
    ),
    (
        BASE.market_chain_id.unwrap(),
        BASE.boundless_market_address,
        address!("0x5dd2a5fa8c60f9d547a41ad800ff9d122bd5a87f"),
    ),
];

fn lookup_registry(chain_id: u64, market_address: Address) -> Option<Address> {
    VERSION_REGISTRIES
        .iter()
        .find(|(id, market, _)| *id == chain_id && *market == market_address)
        .map(|(_, _, registry)| *registry)
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
        "{code} Broker version {broker_version} is below the minimum supported \
         version {min_version} for chain {chain_id}. Outdated versions may have \
         known performance or security vulnerabilities. \
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
    /// When true, run the version check even in RISC0_DEV_MODE. Auto-set when registry_address is
    /// explicitly provided (i.e., in tests). Production always passes None so this stays false.
    force_check: bool,
}

impl<P: Provider + Clone + Send + Sync + 'static> VersionCheckTask<P> {
    /// Create a new `VersionCheckTask`.
    ///
    /// - `broker_version`: packed version to enforce. Pass `None` to use the compile-time
    ///   `BROKER_VERSION` (the default for production).
    /// - `market_address`: address of the BoundlessMarket contract for this deployment. Used
    ///   to resolve the correct VersionRegistry when multiple deployments share a chain ID
    ///   (e.g. staging and prod on Base Sepolia).
    /// - `registry_address`: address of the VersionRegistry contract. Pass `None` to look up the
    ///   address from the hardcoded table (the default for production).
    /// - `force_check`: run the version check even in `RISC0_DEV_MODE`. Auto-set when
    ///   `registry_address` is provided; also exposed as `--force-version-check` CLI flag.
    pub(crate) fn new(
        provider: P,
        chain_id: u64,
        market_address: Address,
        broker_version: Option<u64>,
        registry_address: Option<Address>,
        force_check: bool,
    ) -> Self {
        let force_check = force_check || registry_address.is_some();
        Self {
            provider,
            chain_id,
            broker_version: broker_version.unwrap_or(BROKER_VERSION),
            registry_address: registry_address
                .or_else(|| lookup_registry(chain_id, market_address)),
            poll_interval: Duration::from_secs(600),
            force_check,
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
        let force_check = self.force_check;

        Box::pin(async move {
            // Skipping the version check in dev mode is safe because RISC0_DEV_MODE
            // disables real proof generation entirely — the broker produces fake receipts
            // that are rejected by on-chain verifiers. A prover cannot use this flag to
            // bypass only the version check while remaining operational in production.
            // force_check is set when registry_address is explicitly provided (i.e., in tests)
            // so that e2e tests can exercise the check even under RISC0_DEV_MODE.
            if is_dev_mode() && !force_check {
                tracing::info!(
                    "Skipping version check (RISC0_DEV_MODE enabled). Broker version: {}",
                    format_version(broker_version)
                );
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
    use crate::{config::ConfigLock, task::Supervisor};
    use alloy::{
        network::EthereumWallet,
        node_bindings::{Anvil, AnvilInstance},
        providers::{ProviderBuilder, WalletProvider},
        signers::local::PrivateKeySigner,
    };
    use std::sync::Arc;
    use tracing_test::traced_test;

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
        assert!(lookup_registry(999999, Address::ZERO).is_none());
        assert!(lookup_registry(0, Address::ZERO).is_none());
    }

    // --- E2e tests ---

    // Minimal mock: no proxy, no initialization, no ownership checks.
    // Implements minimumBrokerVersion, setMinimumBrokerVersion, notice, setNotice, getVersionInfo.
    sol! {
        #[sol(rpc, bytecode = "0x6080604052348015600e575f80fd5b506107c18061001c5f395ff3fe608060405234801561000f575f80fd5b5060043610610055575f3560e01c8063164e6374146100595780633cf572a71461007857806366491911146100945780639c94e6c6146100b0578063de0f4bd4146100ce575b5f80fd5b6100616100ec565b60405161006f929190610314565b60405180910390f35b610092600480360381019061008d91906103ab565b610197565b005b6100ae60048036038101906100a99190610420565b6101ad565b005b6100b86101d7565b6040516100c5919061044b565b60405180910390f35b6100d6610267565b6040516100e3919061046b565b60405180910390f35b5f60605f8054906101000a900467ffffffffffffffff166001808054610111906104b1565b80601f016020809104026020016040519081016040528092919081815260200182805461013d906104b1565b80156101885780601f1061015f57610100808354040283529160200191610188565b820191905f5260205f20905b81548152906001019060200180831161016b57829003601f168201915b50505050509050915091509091565b8181600191826101a89291906106be565b505050565b805f806101000a81548167ffffffffffffffff021916908367ffffffffffffffff16021790555050565b6060600180546101e6906104b1565b80601f0160208091040260200160405190810160405280929190818152602001828054610212906104b1565b801561025d5780601f106102345761010080835404028352916020019161025d565b820191905f5260205f20905b81548152906001019060200180831161024057829003601f168201915b5050505050905090565b5f805f9054906101000a900467ffffffffffffffff16905090565b5f67ffffffffffffffff82169050919050565b61029e81610282565b82525050565b5f81519050919050565b5f82825260208201905092915050565b8281835e5f83830152505050565b5f601f19601f8301169050919050565b5f6102e6826102a4565b6102f081856102ae565b93506103008185602086016102be565b610309816102cc565b840191505092915050565b5f6040820190506103275f830185610295565b818103602083015261033981846102dc565b90509392505050565b5f80fd5b5f80fd5b5f80fd5b5f80fd5b5f80fd5b5f8083601f84011261036b5761036a61034a565b5b8235905067ffffffffffffffff8111156103885761038761034e565b5b6020830191508360018202830111156103a4576103a3610352565b5b9250929050565b5f80602083850312156103c1576103c0610342565b5b5f83013567ffffffffffffffff8111156103de576103dd610346565b5b6103ea85828601610356565b92509250509250929050565b6103ff81610282565b8114610409575f80fd5b50565b5f8135905061041a816103f6565b92915050565b5f6020828403121561043557610434610342565b5b5f6104428482850161040c565b91505092915050565b5f6020820190508181035f83015261046381846102dc565b905092915050565b5f60208201905061047e5f830184610295565b92915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52602260045260245ffd5b5f60028204905060018216806104c857607f821691505b6020821081036104db576104da610484565b5b50919050565b5f82905092915050565b7f4e487b71000000000000000000000000000000000000000000000000000000005f52604160045260245ffd5b5f819050815f5260205f209050919050565b5f6020601f8301049050919050565b5f82821b905092915050565b5f600883026105747fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff82610539565b61057e8683610539565b95508019841693508086168417925050509392505050565b5f819050919050565b5f819050919050565b5f6105c26105bd6105b884610596565b61059f565b610596565b9050919050565b5f819050919050565b6105db836105a8565b6105ef6105e7826105c9565b848454610545565b825550505050565b5f90565b6106036105f7565b61060e8184846105d2565b505050565b5b81811015610631576106265f826105fb565b600181019050610614565b5050565b601f8211156106765761064781610518565b6106508461052a565b8101602085101561065f578190505b61067361066b8561052a565b830182610613565b50505b505050565b5f82821c905092915050565b5f6106965f198460080261067b565b1980831691505092915050565b5f6106ae8383610687565b9150826002028217905092915050565b6106c883836104e1565b67ffffffffffffffff8111156106e1576106e06104eb565b5b6106eb82546104b1565b6106f6828285610635565b5f601f831160018114610723575f8415610711578287013590505b61071b85826106a3565b865550610782565b601f19841661073186610518565b5f5b8281101561075857848901358255600182019150602085019450602081019050610733565b868310156107755784890135610771601f891682610687565b8355505b6001600288020188555050505b5050505050505056fea26469706673582212201f956482673a956133b45eb85239af9fbe93ed5e43821e18ba0ad8014d8184f164736f6c634300081a0033")]
        contract MockVersionRegistry {
            constructor() {}
            function setMinimumBrokerVersion(uint64 version) external;
            function minimumBrokerVersion() external view returns (uint64);
            function setNotice(string calldata n) external;
            function notice() external view returns (string memory);
            function getVersionInfo() external view returns (uint64 minimumVersion, string memory noticeOut);
        }
    }

    async fn make_provider(
        anvil: &AnvilInstance,
    ) -> impl Provider + WalletProvider + Clone + 'static {
        let signer: PrivateKeySigner = anvil.keys()[0].clone().into();
        ProviderBuilder::new()
            .wallet(EthereumWallet::from(signer))
            .connect_http(anvil.endpoint().parse().unwrap())
    }

    async fn deploy_mock<P: Provider + WalletProvider + Clone>(
        provider: &P,
    ) -> anyhow::Result<Address> {
        let contract = MockVersionRegistry::deploy(provider).await?;
        Ok(*contract.address())
    }

    #[tokio::test]
    async fn task_faults_when_below_minimum() {
        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let registry_addr = deploy_mock(&provider).await.unwrap();

        // Set minimum to 99.0.0
        let registry = MockVersionRegistry::new(registry_addr, &provider);
        registry
            .setMinimumBrokerVersion(pack_version(99, 0, 0))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let chain_id = provider.get_chain_id().await.unwrap();
        let broker_version = pack_version(1, 0, 0);

        let task = Arc::new(VersionCheckTask {
            provider: provider.clone(),
            chain_id,
            broker_version,
            registry_address: Some(registry_addr),
            poll_interval: Duration::from_secs(600),
            force_check: true,
        });

        let config = ConfigLock::default();
        let cancel = CancellationToken::new();
        let result = Supervisor::new(task, config, cancel).spawn().await;

        assert!(result.is_err(), "Supervisor should fail when broker is below minimum version");
    }

    #[tokio::test]
    #[traced_test(debug)]
    async fn successful_version_check_task() {
        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let registry_addr = deploy_mock(&provider).await.unwrap();

        // Set minimum to 1.0.0 — broker version matches exactly
        let registry = MockVersionRegistry::new(registry_addr, &provider);
        registry
            .setMinimumBrokerVersion(pack_version(1, 0, 0))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let chain_id = provider.get_chain_id().await.unwrap();
        let broker_version = pack_version(1, 0, 0);

        let task = Arc::new(VersionCheckTask {
            provider: provider.clone(),
            chain_id,
            broker_version,
            registry_address: Some(registry_addr),
            poll_interval: Duration::from_secs(600),
            force_check: true,
        });

        // Cancel after a short delay — the first tick fires immediately,
        // so check_version runs before cancellation.
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            cancel_clone.cancel();
        });

        let config = ConfigLock::default();
        let result = Supervisor::new(task, config, cancel).spawn().await;
        assert!(result.is_ok(), "Version check should pass when broker meets minimum");
        assert!(logs_contain("Version check passed"));
    }

    #[tokio::test]
    #[traced_test(debug)]
    async fn version_check_logs_notice() {
        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let registry_addr = deploy_mock(&provider).await.unwrap();

        let registry = MockVersionRegistry::new(registry_addr, &provider);
        // Set version well below broker's, with a notice message
        registry
            .setMinimumBrokerVersion(pack_version(0, 0, 1))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();
        registry
            .setNotice("Upgrade to v1.4.0 available".to_string())
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let chain_id = provider.get_chain_id().await.unwrap();
        let broker_version = pack_version(1, 0, 0);

        let result = check_version(&provider, registry_addr, chain_id, broker_version).await;

        assert!(result.is_ok(), "Version check should pass when broker is above minimum");
        assert!(logs_contain("VersionRegistry notice"), "Notice should be logged");
    }

    #[tokio::test]
    #[traced_test(debug)]
    async fn rpc_failure_logs_warning_without_panic() {
        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let chain_id = provider.get_chain_id().await.unwrap();

        // Address with no contract deployed — RPC call will fail
        let bogus_addr: Address = "0x0000000000000000000000000000000000000001".parse().unwrap();
        let broker_version = pack_version(1, 0, 0);

        let result = check_version(&provider, bogus_addr, chain_id, broker_version).await;

        assert!(result.is_ok(), "RPC failure should not propagate as an error");
        assert!(
            logs_contain("Failed to read VersionRegistry"),
            "Should log warning on RPC failure"
        );
    }

    #[tokio::test]
    #[traced_test(debug)]
    async fn version_check_passes_then_faults_after_update() {
        // TODO: remove guard entirely since we now have force

        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let registry_addr = deploy_mock(&provider).await.unwrap();

        let registry = MockVersionRegistry::new(registry_addr, &provider);
        // Start with minimum 1.0.0 — broker is at 1.0.0, passes
        registry
            .setMinimumBrokerVersion(pack_version(1, 0, 0))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let chain_id = provider.get_chain_id().await.unwrap();
        let broker_version = pack_version(1, 0, 0);

        // Phase 1: direct check proves the initial state passes
        let result = check_version(&provider, registry_addr, chain_id, broker_version).await;
        assert!(result.is_ok());
        assert!(logs_contain("Version check passed"), "First check should pass");

        // Bump minimum above broker version
        registry
            .setMinimumBrokerVersion(pack_version(99, 0, 0))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        // Phase 2: supervisor detects the raised minimum on its first (immediate) tick.
        // tokio interval's first tick always fires immediately, so no timer advancement needed.
        let task = Arc::new(VersionCheckTask {
            provider: provider.clone(),
            chain_id,
            broker_version,
            registry_address: Some(registry_addr),
            poll_interval: Duration::from_secs(600),
            force_check: true,
        });
        let cancel = CancellationToken::new();
        let config = ConfigLock::default();
        let result = Supervisor::new(task, config, cancel).spawn().await;
        assert!(
            result.is_err(),
            "Supervisor should fault after minimum is raised above broker version"
        );
    }

    #[tokio::test]
    #[traced_test(debug)]
    async fn notice_set_then_cleared() {
        let anvil = Anvil::new().spawn();
        let provider = make_provider(&anvil).await;
        let registry_addr = deploy_mock(&provider).await.unwrap();

        let registry = MockVersionRegistry::new(registry_addr, &provider);
        registry
            .setMinimumBrokerVersion(pack_version(0, 0, 1))
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();
        registry
            .setNotice("Upgrade to v2.0.0 by March 28".to_string())
            .send()
            .await
            .unwrap()
            .watch()
            .await
            .unwrap();

        let chain_id = provider.get_chain_id().await.unwrap();
        let broker_version = pack_version(1, 0, 0);

        // First check — notice should be logged
        let result = check_version(&provider, registry_addr, chain_id, broker_version).await;
        assert!(result.is_ok());
        assert!(logs_contain("VersionRegistry notice"), "Notice should be logged when set");
        assert!(logs_contain("Upgrade to v2.0.0 by March 28"), "Notice text should appear in logs");

        // Clear the notice
        registry.setNotice(String::new()).send().await.unwrap().watch().await.unwrap();

        // Second check — notice is empty, should NOT be logged again
        let result = check_version(&provider, registry_addr, chain_id, broker_version).await;
        assert!(result.is_ok());

        // Notice must appear exactly once — the second call must not log it
        logs_assert(|lines: &[&str]| {
            let count = lines.iter().filter(|l| l.contains("VersionRegistry notice")).count();
            match count {
                1 => Ok(()),
                n => Err(format!(
                    "Expected 'VersionRegistry notice' exactly once, but found {n} occurrences"
                )),
            }
        });

        // Both calls must succeed — "Version check passed" appears twice
        logs_assert(|lines: &[&str]| {
            let count = lines.iter().filter(|l| l.contains("Version check passed")).count();
            match count {
                2 => Ok(()),
                n => Err(format!(
                    "Expected 'Version check passed' exactly twice, but found {n} occurrences"
                )),
            }
        });
    }
}
