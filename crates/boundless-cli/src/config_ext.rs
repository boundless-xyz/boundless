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

//! Extension traits for config types to provide consistent validation and error handling

use crate::config::{ProverConfig, RequestorConfig, RewardsConfig};
use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Context, Result};

/// Common configuration validation trait
pub trait ConfigValidation {
    /// Validate that all required fields are present
    fn validate(&self) -> Result<()>;

    /// Get a helpful error message for missing configuration
    fn missing_config_help(&self, field: &str) -> String;
}

/// Extension trait for RewardsConfig
pub trait RewardsConfigExt {
    /// Load config from files and environment variables
    fn load_and_validate(&self) -> Result<RewardsConfig>;

    /// Get RPC URL or return helpful error
    fn require_rpc_url_with_help(&self) -> Result<String>;

    /// Get reward private key or return helpful error
    fn require_reward_key_with_help(&self) -> Result<PrivateKeySigner>;

    /// Get staking private key or return helpful error
    fn require_staking_key_with_help(&self) -> Result<PrivateKeySigner>;
}

impl RewardsConfigExt for RewardsConfig {
    fn load_and_validate(&self) -> Result<RewardsConfig> {
        let config = self.clone().load_from_files()?;
        config.validate()?;
        Ok(config)
    }

    fn require_rpc_url_with_help(&self) -> Result<String> {
        self.reward_rpc_url.clone().map(|u| u.to_string()).context(
            "No RPC URL configured for rewards.\n\n\
            To configure: run 'boundless rewards setup'\n\
            Or set REWARD_RPC_URL environment variable\n\
            Or use --reward-rpc-url flag",
        )
    }

    fn require_reward_key_with_help(&self) -> Result<PrivateKeySigner> {
        self.reward_private_key.clone().context(
            "No reward private key configured.\n\n\
            To configure: run 'boundless rewards setup'\n\
            Or set REWARD_PRIVATE_KEY environment variable\n\
            Or use --reward-private-key flag",
        )
    }

    fn require_staking_key_with_help(&self) -> Result<PrivateKeySigner> {
        self.staking_private_key.clone().or_else(|| self.reward_private_key.clone()).context(
            "No staking private key configured.\n\n\
            To configure: run 'boundless rewards setup'\n\
            Or set STAKING_PRIVATE_KEY environment variable\n\
            Or use --staking-private-key flag",
        )
    }
}

impl ConfigValidation for RewardsConfig {
    fn validate(&self) -> Result<()> {
        // Basic validation - at least RPC URL should be present for most operations
        if self.reward_rpc_url.is_none() {
            bail!(self.missing_config_help("reward_rpc_url"));
        }
        Ok(())
    }

    fn missing_config_help(&self, field: &str) -> String {
        format!(
            "Missing required configuration: {}\n\n\
            To configure rewards: run 'boundless rewards setup'\n\
            Or set environment variables (see 'boundless rewards --help')",
            field
        )
    }
}

/// Extension trait for ProverConfig
pub trait ProverConfigExt {
    /// Load config from files and environment variables
    fn load_and_validate(&self) -> Result<ProverConfig>;

    /// Get private key or return helpful error
    fn require_private_key_with_help(&self) -> Result<PrivateKeySigner>;
}

impl ProverConfigExt for ProverConfig {
    fn load_and_validate(&self) -> Result<ProverConfig> {
        let config = self.clone().load_from_files()?;
        config.validate()?;
        Ok(config)
    }

    fn require_private_key_with_help(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "No private key configured for prover.\n\n\
            To configure: run 'boundless prover setup'\n\
            Or set PROVER_PRIVATE_KEY environment variable\n\
            Or use --prover-private-key flag",
        )
    }
}

impl ConfigValidation for ProverConfig {
    fn validate(&self) -> Result<()> {
        if self.prover_rpc_url.is_none() {
            bail!(self.missing_config_help("prover_rpc_url"));
        }
        Ok(())
    }

    fn missing_config_help(&self, field: &str) -> String {
        format!(
            "Missing required configuration: {}\n\n\
            To configure prover: run 'boundless prover setup'\n\
            Or set environment variables (see 'boundless prover --help')",
            field
        )
    }
}

/// Extension trait for RequestorConfig
pub trait RequestorConfigExt {
    /// Load config from files and environment variables
    fn load_and_validate(&self) -> Result<RequestorConfig>;

    /// Get private key or return helpful error
    fn require_private_key_with_help(&self) -> Result<PrivateKeySigner>;
}

impl RequestorConfigExt for RequestorConfig {
    fn load_and_validate(&self) -> Result<RequestorConfig> {
        let config = self.clone().load_from_files()?;
        config.validate()?;
        Ok(config)
    }

    fn require_private_key_with_help(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "No private key configured for requestor.\n\n\
            To configure: run 'boundless requestor setup'\n\
            Or set REQUESTOR_PRIVATE_KEY environment variable\n\
            Or use --requestor-private-key flag",
        )
    }
}

impl ConfigValidation for RequestorConfig {
    fn validate(&self) -> Result<()> {
        if self.requestor_rpc_url.is_none() {
            bail!(self.missing_config_help("requestor_rpc_url"));
        }
        Ok(())
    }

    fn missing_config_help(&self, field: &str) -> String {
        format!(
            "Missing required configuration: {}\n\n\
            To configure requestor: run 'boundless requestor setup'\n\
            Or set environment variables (see 'boundless requestor --help')",
            field
        )
    }
}

/// Helper to validate common address input patterns
pub fn validate_address_input(
    address: Option<Address>,
    private_key: Option<&PrivateKeySigner>,
    context: &str,
) -> Result<Address> {
    address.or_else(|| private_key.map(|pk| pk.address())).with_context(|| {
        format!(
            "No address specified for {}.\n\n\
            To configure a default address: run 'boundless setup'\n\
            Or provide an address as an argument",
            context
        )
    })
}

/// Helper to validate prover address input (supports read-only mode)
pub fn validate_prover_address_input(
    address_arg: Option<Address>,
    config_address: Option<Address>,
    private_key: Option<&PrivateKeySigner>,
    context: &str,
) -> Result<Address> {
    // Priority: CLI arg > config address > private key derived address
    address_arg.or(config_address).or_else(|| private_key.map(|pk| pk.address())).with_context(
        || {
            format!(
                "No address specified for {}.\n\n\
                To configure a default address: run 'boundless prover setup'\n\
                Or provide an address as an argument",
                context
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::address;

    #[test]
    fn test_rewards_config_validation() {
        use url::Url;

        let config = RewardsConfig {
            reward_rpc_url: None,
            private_key: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            mining_state_file: None,
            beacon_api_url: None,
            zkc_deployment: None,
        };
        assert!(config.validate().is_err());

        let config_with_rpc = RewardsConfig {
            reward_rpc_url: Some(Url::parse("http://localhost:8545").unwrap()),
            private_key: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            mining_state_file: None,
            beacon_api_url: None,
            zkc_deployment: None,
        };
        assert!(config_with_rpc.validate().is_ok());
    }

    #[test]
    fn test_missing_config_help() {
        let config = RewardsConfig {
            reward_rpc_url: None,
            private_key: None,
            staking_private_key: None,
            staking_address: None,
            reward_private_key: None,
            reward_address: None,
            mining_state_file: None,
            beacon_api_url: None,
            zkc_deployment: None,
        };
        let help = config.missing_config_help("test_field");
        assert!(help.contains("test_field"));
        assert!(help.contains("boundless rewards setup"));
    }

    #[test]
    fn test_validate_address_input() {
        let addr = address!("0x0000000000000000000000000000000000000001");
        let result = validate_address_input(Some(addr), None, "test");
        assert_eq!(result.unwrap(), addr);

        let signer = "0x0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<PrivateKeySigner>()
            .unwrap();
        let result = validate_address_input(None, Some(&signer), "test");
        assert_eq!(result.unwrap(), signer.address());

        let result = validate_address_input(None, None, "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_prover_address_input() {
        // Test priority: CLI arg > config address > private key derived
        let cli_addr = address!("0x0000000000000000000000000000000000000001");
        let config_addr = address!("0x0000000000000000000000000000000000000002");
        let signer = "0x0000000000000000000000000000000000000000000000000000000000000001"
            .parse::<PrivateKeySigner>()
            .unwrap();

        // CLI arg takes precedence
        let result =
            validate_prover_address_input(Some(cli_addr), Some(config_addr), Some(&signer), "test");
        assert_eq!(result.unwrap(), cli_addr);

        // Config address used when no CLI arg
        let result = validate_prover_address_input(None, Some(config_addr), Some(&signer), "test");
        assert_eq!(result.unwrap(), config_addr);

        // Private key derived when no CLI arg or config address
        let result = validate_prover_address_input(None, None, Some(&signer), "test");
        assert_eq!(result.unwrap(), signer.address());

        // Read-only mode: config address but no private key
        let result = validate_prover_address_input(None, Some(config_addr), None, "test");
        assert_eq!(result.unwrap(), config_addr);

        // Error when nothing is provided
        let result = validate_prover_address_input(None, None, None, "test");
        assert!(result.is_err());
    }
}
