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

//! Common configuration options for commands in the Boundless CLI.

use std::{num::ParseIntError, time::Duration};

use alloy::signers::local::PrivateKeySigner;
use anyhow::{Context, Result};
use clap::Args;
use risc0_zkvm::ProverOpts;
use tracing::level_filters::LevelFilter;
use url::Url;

use boundless_market::{client::ClientBuilder, Client, Deployment, NotProvided};
use boundless_zkc;

/// Parse a private key string, adding "0x" prefix if not present
fn parse_private_key(key: &str) -> Result<PrivateKeySigner> {
    let key_with_prefix =
        if key.starts_with("0x") { key.to_string() } else { format!("0x{}", key) };
    key_with_prefix.parse().context("Failed to parse private key")
}

/// Common configuration options for all commands
#[derive(Args, Debug, Clone)]
pub struct GlobalConfig {
    /// Ethereum transaction timeout in seconds.
    #[clap(long, env = "TX_TIMEOUT", global = true, value_parser = |arg: &str| -> Result<Duration, ParseIntError> {Ok(Duration::from_secs(arg.parse()?))})]
    pub tx_timeout: Option<Duration>,

    /// Log level (error, warn, info, debug, trace)
    #[clap(long, env = "LOG_LEVEL", global = true, default_value = "warn")]
    pub log_level: LevelFilter,
}

/// Configuration for requestor module commands
#[derive(Args, Debug, Clone)]
pub struct RequestorConfig {
    /// RPC URL for the requestor network
    #[clap(long = "requestor-rpc-url", env = "REQUESTOR_RPC_URL")]
    pub requestor_rpc_url: Option<Url>,

    /// Private key for requestor transactions
    #[clap(long = "requestor-private-key", env = "REQUESTOR_PRIVATE_KEY", hide_env_values = true)]
    pub private_key: Option<PrivateKeySigner>,

    /// Configuration for the Boundless deployment to use.
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,
}

/// Configuration for rewards module commands
#[derive(Args, Debug, Clone)]
pub struct RewardsConfig {
    /// RPC URL for the rewards network
    #[clap(long = "reward-rpc-url", env = "REWARD_RPC_URL")]
    pub reward_rpc_url: Option<Url>,

    /// Private key for rewards transactions (deprecated, use staking/reward specific keys)
    #[clap(skip)]
    pub private_key: Option<PrivateKeySigner>,

    /// Private key for staking operations (loaded from config)
    #[clap(skip)]
    pub staking_private_key: Option<PrivateKeySigner>,

    /// Staking address (loaded from config or env)
    #[clap(skip)]
    pub staking_address: Option<alloy::primitives::Address>,

    /// Private key for reward claiming operations (loaded from config)
    #[clap(skip)]
    pub reward_private_key: Option<PrivateKeySigner>,

    /// Reward address (loaded from config or env)
    #[clap(skip)]
    pub reward_address: Option<alloy::primitives::Address>,

    /// Mining state file path (loaded from config or env)
    #[clap(skip)]
    pub mining_state_file: Option<String>,

    /// Beacon API URL for claiming mining rewards (loaded from config or env)
    #[clap(skip)]
    pub beacon_api_url: Option<Url>,

    /// Configuration for the ZKC deployment to use.
    #[clap(skip)]
    pub zkc_deployment: Option<boundless_zkc::deployments::Deployment>,
}

impl RequestorConfig {
    /// Load configuration from requestor config files
    pub fn load_from_files(mut self) -> Result<Self> {
        use crate::config_file::{Config, Secrets};

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        // Get network name from config
        let network = config.as_ref().and_then(|c| c.requestor.as_ref()).map(|r| &r.network);

        if self.requestor_rpc_url.is_none() {
            if let Ok(rpc_url) = std::env::var("REQUESTOR_RPC_URL") {
                self.requestor_rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let (Some(ref secrets), Some(network)) = (&secrets, network) {
                if let Some(requestor_secrets) = secrets.requestor_networks.get(network) {
                    if let Some(ref rpc_url) = requestor_secrets.rpc_url {
                        self.requestor_rpc_url = Some(Url::parse(rpc_url)?);
                    }
                }
            }
        }

        if self.private_key.is_none() {
            if let Ok(pk) = std::env::var("REQUESTOR_PRIVATE_KEY") {
                self.private_key = Some(parse_private_key(&pk)?);
            } else if let (Some(ref secrets), Some(network)) = (&secrets, network) {
                if let Some(requestor_secrets) = secrets.requestor_networks.get(network) {
                    if let Some(ref pk) = requestor_secrets.private_key {
                        self.private_key = Some(parse_private_key(pk)?);
                    }
                }
            }
        }

        if self.deployment.is_none() {
            if let Some(ref config) = config {
                let network = config.requestor.as_ref().map(|r| &r.network);

                if let Some(network) = network {
                    self.deployment = match network.as_str() {
                        "base-mainnet" => Some(boundless_market::deployments::BASE),
                        "base-sepolia" => Some(boundless_market::deployments::BASE_SEPOLIA),
                        "eth-sepolia" => Some(boundless_market::deployments::SEPOLIA),
                        custom => {
                            config.custom_markets.iter().find(|m| m.name == custom).map(|m| {
                                let mut builder = boundless_market::Deployment::builder();
                                builder
                                    .market_chain_id(m.chain_id)
                                    .boundless_market_address(m.boundless_market_address)
                                    .set_verifier_address(m.set_verifier_address);

                                if let Some(addr) = m.verifier_router_address {
                                    builder.verifier_router_address(addr);
                                }
                                if let Some(addr) = m.collateral_token_address {
                                    builder.collateral_token_address(addr);
                                }
                                if let Some(url) = m.order_stream_url.as_ref() {
                                    builder.order_stream_url(std::borrow::Cow::Owned(url.clone()));
                                }

                                builder.build().expect("Failed to build custom deployment")
                            })
                        }
                    };
                }
            }
        }

        Ok(self)
    }

    /// Access [Self::requestor_rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.requestor_rpc_url
            .clone()
            .context("Requestor RPC URL not provided.\n\nTo configure: run 'boundless requestor setup'\nOr set REQUESTOR_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Requestor private key not provided.\n\nTo configure: run 'boundless requestor setup'\nOr set REQUESTOR_PRIVATE_KEY env var",
        )
    }

    /// Create a partially initialized [ClientBuilder] from the options in this struct.
    pub fn client_builder(&self, tx_timeout: Option<Duration>) -> Result<ClientBuilder> {
        Ok(Client::builder()
            .with_rpc_url(self.require_rpc_url()?)
            .with_deployment(self.deployment.clone())
            .with_timeout(tx_timeout))
    }

    /// Create a partially initialized [ClientBuilder] with signer from the options in this struct.
    pub fn client_builder_with_signer(
        &self,
        tx_timeout: Option<Duration>,
    ) -> Result<ClientBuilder<NotProvided, PrivateKeySigner>> {
        Ok(self.client_builder(tx_timeout)?.with_private_key(self.require_private_key()?))
    }
}

impl RewardsConfig {
    /// Load configuration from rewards config files
    pub fn load_from_files(mut self) -> Result<Self> {
        use crate::config_file::{Config, Secrets};

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        // Get network name from config
        let network = config.as_ref().and_then(|c| c.rewards.as_ref()).map(|r| &r.network);

        // Get config values for override detection
        let config_rpc_url = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.rpc_url.as_ref());

        let config_private_key = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.private_key.as_ref());

        let config_staking_private_key = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.staking_private_key.as_ref());

        let config_staking_address = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.staking_address.as_ref());

        let config_reward_private_key = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.reward_private_key.as_ref());

        let config_reward_address = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.reward_address.as_ref());

        let config_mining_state_file = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.mining_state_file.as_ref());

        let config_beacon_api_url = secrets
            .as_ref()
            .and_then(|s| network.and_then(|n| s.rewards_networks.get(n)))
            .and_then(|r| r.beacon_api_url.as_ref());

        // Load RPC URL (env var takes precedence)
        if self.reward_rpc_url.is_none() {
            if let Ok(rpc_url) = std::env::var("REWARD_RPC_URL") {
                if config_rpc_url.is_some() {
                    println!(
                        "⚠ Using REWARD_RPC_URL from environment (overriding configured value)"
                    );
                }
                self.reward_rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Some(rpc_url) = config_rpc_url {
                self.reward_rpc_url = Some(Url::parse(rpc_url)?);
            }
        }

        // Load deprecated private_key field from config only (for backward compatibility)
        if self.private_key.is_none() {
            if let Some(pk) = config_private_key {
                self.private_key = Some(parse_private_key(pk)?);
            }
        }

        // Load staking private key (env var takes precedence)
        if self.staking_private_key.is_none() {
            if let Ok(pk) = std::env::var("STAKING_PRIVATE_KEY") {
                if config_staking_private_key.is_some() {
                    println!("⚠ Using STAKING_PRIVATE_KEY from environment (overriding configured value)");
                }
                self.staking_private_key = Some(parse_private_key(&pk)?);
            } else if let Some(pk) = config_staking_private_key {
                self.staking_private_key = Some(parse_private_key(pk)?);
            }
        }

        // Load staking address (env var takes precedence)
        if self.staking_address.is_none() {
            if let Ok(addr) = std::env::var("STAKING_ADDRESS") {
                if config_staking_address.is_some() {
                    println!(
                        "⚠ Using STAKING_ADDRESS from environment (overriding configured value)"
                    );
                }
                if !addr.is_empty() {
                    self.staking_address = Some(addr.parse()?);
                }
            } else if let Some(addr) = config_staking_address {
                if !addr.is_empty() {
                    self.staking_address = Some(addr.parse()?);
                }
            }
        }

        // Load reward private key (env var takes precedence)
        if self.reward_private_key.is_none() {
            if let Ok(pk) = std::env::var("REWARD_PRIVATE_KEY") {
                if config_reward_private_key.is_some() {
                    println!(
                        "⚠ Using REWARD_PRIVATE_KEY from environment (overriding configured value)"
                    );
                }
                self.reward_private_key = Some(parse_private_key(&pk)?);
            } else if let Some(pk) = config_reward_private_key {
                self.reward_private_key = Some(parse_private_key(pk)?);
            }
        }

        // Load reward address (env var takes precedence)
        if self.reward_address.is_none() {
            if let Ok(addr) = std::env::var("REWARD_ADDRESS") {
                if config_reward_address.is_some() {
                    println!(
                        "⚠ Using REWARD_ADDRESS from environment (overriding configured value)"
                    );
                }
                if !addr.is_empty() {
                    self.reward_address = Some(addr.parse()?);
                }
            } else if let Some(addr) = config_reward_address {
                if !addr.is_empty() {
                    self.reward_address = Some(addr.parse()?);
                }
            }
        }

        // Load mining state file (env var takes precedence)
        if self.mining_state_file.is_none() {
            if let Ok(state_file) = std::env::var("MINING_STATE_FILE") {
                if config_mining_state_file.is_some() {
                    println!(
                        "⚠ Using MINING_STATE_FILE from environment (overriding configured value)"
                    );
                }
                self.mining_state_file = Some(state_file);
            } else if let Some(state_file) = config_mining_state_file {
                self.mining_state_file = Some(state_file.clone());
            }
        }

        // Load Beacon API URL (env var takes precedence)
        if self.beacon_api_url.is_none() {
            if let Ok(url) = std::env::var("BEACON_API_URL") {
                if config_beacon_api_url.is_some() {
                    println!(
                        "⚠ Using BEACON_API_URL from environment (overriding configured value)"
                    );
                }
                self.beacon_api_url = Some(Url::parse(&url)?);
            } else if let Some(url) = config_beacon_api_url {
                self.beacon_api_url = Some(Url::parse(url)?);
            }
        }

        if self.zkc_deployment.is_none() {
            if let Some(ref config) = config {
                if let Some(ref rewards) = config.rewards {
                    self.zkc_deployment = match rewards.network.as_str() {
                        "eth-mainnet" => {
                            boundless_zkc::deployments::Deployment::from_chain_id(1u64)
                        }
                        "eth-sepolia" => {
                            boundless_zkc::deployments::Deployment::from_chain_id(11155111u64)
                        }
                        custom => {
                            config.custom_rewards.iter().find(|r| r.name == custom).map(|r| {
                                boundless_zkc::deployments::Deployment::builder()
                                    .zkc_address(r.zkc_address)
                                    .vezkc_address(r.vezkc_address)
                                    .staking_rewards_address(r.staking_rewards_address)
                                    .build()
                                    .expect("Failed to build custom ZKC deployment")
                            })
                        }
                    };
                }
            }
        }

        Ok(self)
    }

    /// Access [Self::reward_rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.reward_rpc_url
            .clone()
            .context("Reward RPC URL not provided.\n\nTo configure: run 'boundless rewards setup'\nOr set REWARD_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    /// This is deprecated - use require_staking_private_key or require_reward_private_key instead.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Private key not provided.\n\nTo configure: run 'boundless rewards setup'\nOr set STAKING_PRIVATE_KEY or REWARD_PRIVATE_KEY env var",
        )
    }

    /// Access staking private key (tries staking_private_key → reward_private_key → private_key).
    pub fn require_staking_private_key(&self) -> Result<PrivateKeySigner> {
        self.staking_private_key
            .clone()
            .context(
                "Staking private key not provided.\n\nTo configure: run 'boundless rewards setup'\nOr set STAKING_PRIVATE_KEY or REWARD_PRIVATE_KEY env var",
            )
    }

    /// Access reward private key (tries reward_private_key → staking_private_key → private_key).
    pub fn require_reward_private_key(&self) -> Result<PrivateKeySigner> {
        self.reward_private_key
            .clone()
            .context(
                "Reward private key not provided.\n\nTo configure: run 'boundless rewards setup'\nOr set REWARD_PRIVATE_KEY or STAKING_PRIVATE_KEY env var",
            )
    }

    /// Get the ZKC contract address from deployment or environment variable.
    pub fn zkc_address(&self) -> Result<alloy::primitives::Address> {
        if let Ok(addr_str) = std::env::var("ZKC_ADDRESS") {
            return addr_str.parse().context("Failed to parse ZKC_ADDRESS environment variable");
        }
        self.zkc_deployment.as_ref().map(|d| d.zkc_address).context(
            "ZKC address not provided; please set --zkc-address or the ZKC_ADDRESS env var",
        )
    }

    /// Get the veZKC (staking) contract address from deployment or environment variable.
    pub fn vezkc_address(&self) -> Result<alloy::primitives::Address> {
        if let Ok(addr_str) = std::env::var("VEZKC_ADDRESS") {
            return addr_str.parse().context("Failed to parse VEZKC_ADDRESS environment variable");
        }
        self.zkc_deployment.as_ref().map(|d| d.vezkc_address).context(
            "veZKC address not provided; please set --vezkc-address or the VEZKC_ADDRESS env var",
        )
    }

    /// Get the staking rewards contract address from deployment or environment variable.
    pub fn staking_rewards_address(&self) -> Result<alloy::primitives::Address> {
        if let Ok(addr_str) = std::env::var("STAKING_REWARDS_ADDRESS") {
            return addr_str
                .parse()
                .context("Failed to parse STAKING_REWARDS_ADDRESS environment variable");
        }
        self.zkc_deployment
            .as_ref()
            .map(|d| d.staking_rewards_address)
            .context("Staking rewards address not provided; please set --staking-rewards-address or the STAKING_REWARDS_ADDRESS env var")
    }
}

/// Configuration for the proving backend (Bento cluster or local prover)
#[derive(Args, Debug, Clone)]
pub struct ProvingBackendConfig {
    /// Bento API URL
    ///
    /// URL at which your Bento cluster is running.
    #[clap(
        long,
        env = "BENTO_API_URL",
        visible_alias = "bonsai-api-url",
        default_value = DEFAULT_BENTO_API_URL
    )]
    pub bento_api_url: String,

    /// Bento API Key
    ///
    /// Not necessary if using Bento without authentication, which is the default.
    #[clap(long, env = "BENTO_API_KEY", visible_alias = "bonsai-api-key", hide_env_values = true)]
    pub bento_api_key: Option<String>,

    /// Use the default prover instead of defaulting to Bento.
    ///
    /// When enabled, the prover selection follows the default zkVM behavior
    /// based on environment variables like RISC0_PROVER, RISC0_DEV_MODE, etc.
    #[clap(long, conflicts_with = "bento_api_url")]
    pub use_default_prover: bool,

    /// Most commands run a health check on the prover by default. Set this flag to skip it.
    #[clap(long, env = "BENTO_SKIP_HEALTH_CHECK")]
    pub skip_health_check: bool,
}

const DEFAULT_BENTO_API_URL: &str = "http://localhost:8081";

impl ProvingBackendConfig {
    /// Sets environment variables BONSAI_API_URL and BONSAI_API_KEY that are
    /// read by `default_prover()` when constructing the prover.
    pub fn configure_proving_backend(&self) {
        if self.use_default_prover {
            println!("Using default prover behavior (respects RISC0_PROVER, RISC0_DEV_MODE, etc.)");
            return;
        }

        std::env::set_var("BONSAI_API_URL", &self.bento_api_url);
        if let Some(ref api_key) = self.bento_api_key {
            std::env::set_var("BONSAI_API_KEY", api_key);
        } else {
            std::env::set_var("BONSAI_API_KEY", "v1:reserved:50");
        }
    }

    /// Sets environment variables to configure the prover and runs a health check.
    pub async fn configure_proving_backend_with_health_check(&self) -> Result<()> {
        self.configure_proving_backend();

        if self.use_default_prover || self.skip_health_check || ProverOpts::default().dev_mode() {
            return Ok(());
        }

        let using_default_url = self.bento_api_url == DEFAULT_BENTO_API_URL;

        let bento_url = Url::parse(&self.bento_api_url)
            .with_context(|| format!("Failed to parse Bento API URL: {}", self.bento_api_url))?;
        let health_check_url = bento_url.join("health")?;
        println!("  Using Bento prover at {}.", health_check_url);
        reqwest::get(health_check_url.clone())
            .await
            .with_context(|| match using_default_url {
                true => format!("Failed to send health check request to {health_check_url}; You can set --use-default-prover to use a local prover"),
                false => format!("Failed to send health check request to {health_check_url}"),
            })?
            .error_for_status()
            .context("Bento health check endpoint returned error status")?;
        Ok(())
    }
}

/// Configuration options for commands that utilize proving.
#[derive(Args, Debug, Clone)]
pub struct ProverConfig {
    /// RPC URL for the prover network
    #[clap(long = "prover-rpc-url", env = "PROVER_RPC_URL")]
    pub prover_rpc_url: Option<Url>,

    /// Private key for prover transactions
    #[clap(long = "prover-private-key", env = "PROVER_PRIVATE_KEY", hide_env_values = true)]
    pub private_key: Option<PrivateKeySigner>,

    /// Prover address (for read-only mode when no private key is configured)
    #[clap(skip)]
    pub prover_address: Option<alloy::primitives::Address>,

    /// Configuration for the Boundless deployment to use.
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,

    /// Proving backend configuration
    #[clap(flatten, next_help_heading = "Proving Backend")]
    pub proving_backend: ProvingBackendConfig,
}

impl ProverConfig {
    /// Load configuration from prover config files
    pub fn load_from_files(mut self) -> Result<Self> {
        use crate::config_file::{Config, Secrets};

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        // Get network name from config
        let network = config.as_ref().and_then(|c| c.prover.as_ref()).map(|p| &p.network);

        if self.prover_rpc_url.is_none() {
            if let Ok(rpc_url) = std::env::var("PROVER_RPC_URL") {
                self.prover_rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let (Some(ref secrets), Some(network)) = (&secrets, network) {
                if let Some(prover_secrets) = secrets.prover_networks.get(network) {
                    if let Some(ref rpc_url) = prover_secrets.rpc_url {
                        self.prover_rpc_url = Some(Url::parse(rpc_url)?);
                    }
                }
            }
        }

        if self.private_key.is_none() {
            if let Ok(pk) = std::env::var("PROVER_PRIVATE_KEY") {
                self.private_key = Some(parse_private_key(&pk)?);
            } else if let (Some(ref secrets), Some(network)) = (&secrets, network) {
                if let Some(prover_secrets) = secrets.prover_networks.get(network) {
                    if let Some(ref pk) = prover_secrets.private_key {
                        self.private_key = Some(parse_private_key(pk)?);
                    }
                }
            }
        }

        // Load prover address (for read-only mode)
        if self.prover_address.is_none() {
            if let Ok(addr) = std::env::var("PROVER_ADDRESS") {
                self.prover_address = Some(addr.parse()?);
            } else if let (Some(ref secrets), Some(network)) = (&secrets, network) {
                if let Some(prover_secrets) = secrets.prover_networks.get(network) {
                    if let Some(ref addr) = prover_secrets.address {
                        self.prover_address = Some(addr.parse()?);
                    }
                }
            }
        }

        if self.deployment.is_none() {
            if let Some(ref config) = config {
                let network = config.prover.as_ref().map(|p| &p.network);

                if let Some(network) = network {
                    self.deployment = match network.as_str() {
                        "base-mainnet" => Some(boundless_market::deployments::BASE),
                        "base-sepolia" => Some(boundless_market::deployments::BASE_SEPOLIA),
                        "eth-sepolia" => Some(boundless_market::deployments::SEPOLIA),
                        custom => {
                            config.custom_markets.iter().find(|m| m.name == custom).map(|m| {
                                let mut builder = boundless_market::Deployment::builder();
                                builder
                                    .market_chain_id(m.chain_id)
                                    .boundless_market_address(m.boundless_market_address)
                                    .set_verifier_address(m.set_verifier_address);

                                if let Some(addr) = m.verifier_router_address {
                                    builder.verifier_router_address(addr);
                                }
                                if let Some(addr) = m.collateral_token_address {
                                    builder.collateral_token_address(addr);
                                }
                                if let Some(url) = m.order_stream_url.as_ref() {
                                    builder.order_stream_url(std::borrow::Cow::Owned(url.clone()));
                                }

                                builder.build().expect("Failed to build custom deployment")
                            })
                        }
                    };
                }
            }
        }

        // Backward compatibility: check for BONSAI_* env vars if BENTO_* not set
        if self.proving_backend.bento_api_url == DEFAULT_BENTO_API_URL {
            if let Ok(bonsai_url) = std::env::var("BONSAI_API_URL") {
                self.proving_backend.bento_api_url = bonsai_url;
            }
        }

        if self.proving_backend.bento_api_key.is_none() {
            if let Ok(bonsai_key) = std::env::var("BONSAI_API_KEY") {
                self.proving_backend.bento_api_key = Some(bonsai_key);
            }
        }

        Ok(self)
    }

    /// Access [Self::prover_rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.prover_rpc_url
            .clone()
            .context("Prover RPC URL not provided.\n\nTo configure: run 'boundless prover setup'\nOr set PROVER_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Prover private key not provided.\n\nTo configure: run 'boundless prover setup'\nOr set PROVER_PRIVATE_KEY env var",
        )
    }

    /// Create a partially initialized [ClientBuilder] from the options in this struct.
    pub fn client_builder(&self, tx_timeout: Option<Duration>) -> Result<ClientBuilder> {
        Ok(Client::builder()
            .with_rpc_url(self.require_rpc_url()?)
            .with_deployment(self.deployment.clone())
            .with_timeout(tx_timeout))
    }

    /// Create a partially initialized [ClientBuilder] with signer from the options in this struct.
    pub fn client_builder_with_signer(
        &self,
        tx_timeout: Option<Duration>,
    ) -> Result<ClientBuilder<NotProvided, PrivateKeySigner>> {
        Ok(self.client_builder(tx_timeout)?.with_private_key(self.require_private_key()?))
    }

    /// Sets environment variables BONSAI_API_URL and BONSAI_API_KEY that are
    /// read by `default_prover()` when constructing the prover.
    pub fn configure_proving_backend(&self) {
        self.proving_backend.configure_proving_backend()
    }

    /// Sets environment variables to configure the prover and runs a health check.
    pub async fn configure_proving_backend_with_health_check(&self) -> Result<()> {
        self.proving_backend.configure_proving_backend_with_health_check().await
    }
}
