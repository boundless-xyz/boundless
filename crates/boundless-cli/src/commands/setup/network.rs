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

//! Network normalization and validation helpers.

use anyhow::{Context, Result};

/// Pre-built requestor networks that cannot be modified
pub const PREBUILT_REQUESTOR_NETWORKS: &[&str] = &["base-mainnet", "base-sepolia", "eth-sepolia"];

/// Pre-built prover networks that cannot be modified
pub const PREBUILT_PROVER_NETWORKS: &[&str] = &["base-mainnet", "base-sepolia", "eth-sepolia"];

/// Pre-built rewards networks that cannot be modified
pub const PREBUILT_REWARDS_NETWORKS: &[&str] = &["eth-mainnet", "eth-sepolia"];

/// Module type for network operations
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleType {
    /// Requestor module
    Requestor,
    /// Prover module
    Prover,
    /// Rewards module
    Rewards,
}

/// Normalize a network name for market modules (requestor/prover)
pub fn normalize_market_network(network: &str) -> &str {
    match network {
        "Base Mainnet" => "base-mainnet",
        "Base Sepolia" => "base-sepolia",
        "Ethereum Sepolia" | "Eth Sepolia" => "eth-sepolia",
        custom => custom,
    }
}

/// Normalize a network name for rewards module
pub fn normalize_rewards_network(network: &str) -> &str {
    match network {
        "Eth Mainnet" | "Ethereum Mainnet" => "eth-mainnet",
        "Eth Testnet (Sepolia)" | "Eth Sepolia" | "Ethereum Sepolia" => "eth-sepolia",
        custom => custom,
    }
}

/// Check if a network is a pre-built network for the given module type
pub fn is_prebuilt_network(module: ModuleType, network: &str) -> bool {
    match module {
        ModuleType::Requestor => PREBUILT_REQUESTOR_NETWORKS.contains(&network),
        ModuleType::Prover => PREBUILT_PROVER_NETWORKS.contains(&network),
        ModuleType::Rewards => PREBUILT_REWARDS_NETWORKS.contains(&network),
    }
}

/// Get the list of pre-built networks for a module type
pub fn get_prebuilt_networks(module: ModuleType) -> &'static [&'static str] {
    match module {
        ModuleType::Requestor => PREBUILT_REQUESTOR_NETWORKS,
        ModuleType::Prover => PREBUILT_PROVER_NETWORKS,
        ModuleType::Rewards => PREBUILT_REWARDS_NETWORKS,
    }
}

/// Query the chain ID from an RPC URL
pub async fn query_chain_id(rpc_url: &str) -> Result<u64> {
    use alloy::providers::{Provider, ProviderBuilder};

    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .context("Failed to connect to RPC provider")?;

    let chain_id = provider.get_chain_id().await.context("Failed to query chain ID")?;

    Ok(chain_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_market_network() {
        assert_eq!(normalize_market_network("Base Mainnet"), "base-mainnet");
        assert_eq!(normalize_market_network("base-mainnet"), "base-mainnet");
        assert_eq!(normalize_market_network("Base Sepolia"), "base-sepolia");
        assert_eq!(normalize_market_network("base-sepolia"), "base-sepolia");
        assert_eq!(normalize_market_network("Ethereum Sepolia"), "eth-sepolia");
        assert_eq!(normalize_market_network("Eth Sepolia"), "eth-sepolia");
        assert_eq!(normalize_market_network("eth-sepolia"), "eth-sepolia");
    }

    #[test]
    fn test_normalize_market_network_custom() {
        assert_eq!(normalize_market_network("custom-1234"), "custom-1234");
        assert_eq!(normalize_market_network("my-network"), "my-network");
    }

    #[test]
    fn test_is_prebuilt_network_requestor() {
        assert!(is_prebuilt_network(ModuleType::Requestor, "base-mainnet"));
        assert!(is_prebuilt_network(ModuleType::Requestor, "base-sepolia"));
        assert!(is_prebuilt_network(ModuleType::Requestor, "eth-sepolia"));
        assert!(!is_prebuilt_network(ModuleType::Requestor, "custom-1234"));
        assert!(!is_prebuilt_network(ModuleType::Requestor, "eth-mainnet"));
    }
}
