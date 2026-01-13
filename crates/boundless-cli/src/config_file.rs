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

//! Configuration file management for the Boundless CLI.

use std::{collections::HashMap, fs, path::PathBuf};

use alloy::primitives::Address;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Get the configuration directory path (~/.boundless)
pub fn config_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME environment variable not set")?;
    Ok(PathBuf::from(home).join(".boundless"))
}

/// Ensure the configuration directory exists
pub fn ensure_config_dir() -> Result<PathBuf> {
    let dir = config_dir()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create config directory: {}", dir.display()))?;
    }
    Ok(dir)
}

/// Main configuration file (config.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    /// Requestor configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requestor: Option<RequestorConfig>,

    /// Prover configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prover: Option<ProverConfig>,

    /// Rewards configuration
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rewards: Option<RewardsConfig>,

    /// Custom market deployments
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub custom_markets: Vec<CustomMarketDeployment>,

    /// Custom rewards deployments
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub custom_rewards: Vec<CustomRewardsDeployment>,
}

/// Requestor configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorConfig {
    /// Network name
    pub network: String,
}

/// Prover configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProverConfig {
    /// Network name
    pub network: String,
}

/// Rewards configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardsConfig {
    /// Network name
    pub network: String,
}

/// Custom market deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomMarketDeployment {
    /// Deployment name
    pub name: String,
    /// EIP-155 chain ID
    pub chain_id: u64,
    /// BoundlessMarket contract address
    pub boundless_market_address: Address,
    /// RiscZeroVerifierRouter contract address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verifier_router_address: Option<Address>,
    /// RiscZeroSetVerifier contract address
    pub set_verifier_address: Address,
    /// Collateral token contract address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collateral_token_address: Option<Address>,
    /// Order stream URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_stream_url: Option<String>,
}

/// Custom rewards deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomRewardsDeployment {
    /// Deployment name
    pub name: String,
    /// EIP-155 chain ID
    pub chain_id: u64,
    /// ZKC token contract address
    pub zkc_address: Address,
    /// veZKC contract address
    pub vezkc_address: Address,
    /// Staking rewards contract address
    pub staking_rewards_address: Address,
    /// Mining accounting contract address
    pub mining_accounting_address: Address,
    /// Mining mint contract address
    pub mining_mint_address: Address,
}

/// Secrets file (secrets.toml)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Secrets {
    /// Network-specific requestor secrets (network name -> secrets)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub requestor_networks: HashMap<String, RequestorSecrets>,

    /// Network-specific prover secrets (network name -> secrets)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub prover_networks: HashMap<String, ProverSecrets>,

    /// Network-specific rewards secrets (network name -> secrets)
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub rewards_networks: HashMap<String, RewardsSecrets>,
}

/// Requestor secrets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestorSecrets {
    /// RPC URL for requestor network
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,

    /// Private key for transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,

    /// Public requestor address (if no private key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Prover secrets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProverSecrets {
    /// RPC URL for prover network
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,

    /// Private key for transactions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,

    /// Public prover address (if no private key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
}

/// Rewards secrets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewardsSecrets {
    /// RPC URL for rewards network
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_url: Option<String>,

    /// Private key for staking address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staking_private_key: Option<String>,

    /// Public staking address (if no private key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub staking_address: Option<String>,

    /// Private key for reward address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward_private_key: Option<String>,

    /// Public reward address (if no private key)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reward_address: Option<String>,

    /// Path to mining state file (optional - only needed if generating mining work)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mining_state_file: Option<String>,

    /// Beacon API URL for claiming mining rewards
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beacon_api_url: Option<String>,

    /// Deprecated: Private key for transactions (for backwards compatibility)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
}

impl Config {
    /// Load config from ~/.boundless/config.toml
    pub fn load() -> Result<Self> {
        let path = config_dir()?.join("config.toml");
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Save config to ~/.boundless/config.toml
    pub fn save(&self) -> Result<()> {
        let dir = ensure_config_dir()?;
        let path = dir.join("config.toml");

        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }

    /// Get the path to the config file
    pub fn path() -> Result<PathBuf> {
        Ok(config_dir()?.join("config.toml"))
    }
}

impl Secrets {
    /// Load secrets from ~/.boundless/secrets.toml
    pub fn load() -> Result<Self> {
        let path = config_dir()?.join("secrets.toml");
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read secrets file: {}", path.display()))?;
        toml::from_str(&contents)
            .with_context(|| format!("Failed to parse secrets file: {}", path.display()))
    }

    /// Save secrets to ~/.boundless/secrets.toml with secure permissions
    pub fn save(&self) -> Result<()> {
        let dir = ensure_config_dir()?;
        let path = dir.join("secrets.toml");

        let contents = toml::to_string_pretty(self).context("Failed to serialize secrets")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write secrets file: {}", path.display()))?;

        // Set file permissions to 600 (read/write for owner only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(&path, perms)?;
        }

        Ok(())
    }

    /// Get the path to the secrets file
    pub fn path() -> Result<PathBuf> {
        Ok(config_dir()?.join("secrets.toml"))
    }
}
