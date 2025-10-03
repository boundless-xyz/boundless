// Copyright 2025 RISC Zero, Inc.
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

use alloy::{providers::DynProvider, signers::local::PrivateKeySigner};
use anyhow::{Context, Result};
use clap::Args;
use risc0_zkvm::ProverOpts;
use tracing::level_filters::LevelFilter;
use url::Url;

use boundless_market::{
    client::ClientBuilder, request_builder::StandardRequestBuilder, Client, Deployment, NotProvided,
};
use boundless_zkc;

/// Common configuration options for all commands
#[derive(Args, Debug, Clone)]
pub struct GlobalConfig {
    /// Ethereum transaction timeout in seconds.
    #[clap(long, env = "TX_TIMEOUT", global = true, value_parser = |arg: &str| -> Result<Duration, ParseIntError> {Ok(Duration::from_secs(arg.parse()?))})]
    pub tx_timeout: Option<Duration>,

    /// Log level (error, warn, info, debug, trace)
    #[clap(long, env = "LOG_LEVEL", global = true, default_value = "info")]
    pub log_level: LevelFilter,
}

/// Configuration for requestor module commands
#[derive(Args, Debug, Clone)]
pub struct RequestorConfig {
    /// RPC URL for the requestor network
    #[clap(long = "requestor-rpc-url", env = "REQUESTOR_RPC_URL")]
    #[clap(help = "Also supports RPC_URL env var")]
    pub rpc_url: Option<Url>,

    /// Private key for requestor transactions
    #[clap(long = "requestor-private-key", env = "REQUESTOR_PRIVATE_KEY", hide_env_values = true)]
    #[clap(help = "Also supports PRIVATE_KEY env var")]
    pub private_key: Option<PrivateKeySigner>,

    /// Configuration for the Boundless deployment to use.
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,
}

/// Configuration for rewards module commands
#[derive(Args, Debug, Clone)]
pub struct RewardsConfig {
    /// RPC URL for the rewards network
    #[clap(long = "rewards-rpc-url", env = "REWARDS_RPC_URL")]
    #[clap(help = "Also supports RPC_URL env var")]
    pub rpc_url: Option<Url>,

    /// Private key for rewards transactions
    #[clap(long = "rewards-private-key", env = "REWARDS_PRIVATE_KEY", hide_env_values = true)]
    #[clap(help = "Also supports PRIVATE_KEY env var")]
    pub private_key: Option<PrivateKeySigner>,

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

        if self.rpc_url.is_none() {
            // Try REQUESTOR_RPC_URL, then RPC_URL, then config file
            if let Ok(rpc_url) = std::env::var("REQUESTOR_RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Ok(rpc_url) = std::env::var("RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref requestor_secrets) = secrets.requestor {
                    if let Some(ref rpc_url) = requestor_secrets.rpc_url {
                        self.rpc_url = Some(Url::parse(rpc_url)?);
                    }
                } else if let Some(ref market_secrets) = secrets.market {
                    // Backward compatibility
                    if let Some(ref rpc_url) = market_secrets.rpc_url {
                        self.rpc_url = Some(Url::parse(rpc_url)?);
                    }
                }
            }
        }

        if self.private_key.is_none() {
            // Try REQUESTOR_PRIVATE_KEY, then PRIVATE_KEY, then config file
            if let Ok(pk) = std::env::var("REQUESTOR_PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Ok(pk) = std::env::var("PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref requestor_secrets) = secrets.requestor {
                    if let Some(ref pk) = requestor_secrets.private_key {
                        self.private_key = Some(pk.parse()?);
                    }
                } else if let Some(ref market_secrets) = secrets.market {
                    // Backward compatibility
                    if let Some(ref pk) = market_secrets.private_key {
                        self.private_key = Some(pk.parse()?);
                    }
                }
            }
        }

        if self.deployment.is_none() {
            if let Some(ref config) = config {
                let network = config
                    .requestor
                    .as_ref()
                    .map(|r| &r.network)
                    .or_else(|| config.market.as_ref().map(|m| &m.network));

                if let Some(network) = network {
                    self.deployment = match network.as_str() {
                        "base-mainnet" => Some(boundless_market::deployments::BASE),
                        "base-sepolia" => Some(boundless_market::deployments::BASE_SEPOLIA),
                        "eth-sepolia" => Some(boundless_market::deployments::SEPOLIA),
                        custom => config.custom_markets.iter().find(|m| m.name == custom).map(|m| {
                            let mut builder = boundless_market::Deployment::builder();
                            builder
                                .chain_id(m.chain_id)
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
                        }),
                    };
                }
            }
        }

        Ok(self)
    }

    /// Access [Self::rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.rpc_url
            .clone()
            .context("Blockchain RPC URL not provided.\n\nTo configure: run 'boundless setup requestor'\nOr set --requestor-rpc-url flag or REQUESTOR_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Private key not provided.\n\nTo configure: run 'boundless setup requestor'\nOr set --requestor-private-key flag or REQUESTOR_PRIVATE_KEY env var",
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

        if self.rpc_url.is_none() {
            // Try REWARDS_RPC_URL, then RPC_URL, then config file
            if let Ok(rpc_url) = std::env::var("REWARDS_RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Ok(rpc_url) = std::env::var("RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref rewards_secrets) = secrets.rewards {
                    if let Some(ref rpc_url) = rewards_secrets.rpc_url {
                        self.rpc_url = Some(Url::parse(rpc_url)?);
                    }
                }
            }
        }

        if self.private_key.is_none() {
            // Try REWARDS_PRIVATE_KEY, then PRIVATE_KEY, then config file
            if let Ok(pk) = std::env::var("REWARDS_PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Ok(pk) = std::env::var("PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref rewards_secrets) = secrets.rewards {
                    if let Some(ref pk) = rewards_secrets.private_key {
                        self.private_key = Some(pk.parse()?);
                    }
                }
            }
        }

        if self.zkc_deployment.is_none() {
            if let Some(ref config) = config {
                if let Some(ref rewards) = config.rewards {
                    self.zkc_deployment = match rewards.network.as_str() {
                        "mainnet" => boundless_zkc::deployments::Deployment::from_chain_id(1u64),
                        "sepolia" => boundless_zkc::deployments::Deployment::from_chain_id(11155111u64),
                        custom => config.custom_rewards.iter().find(|r| r.name == custom).map(|r| {
                            boundless_zkc::deployments::Deployment::builder()
                                .zkc_address(r.zkc_address)
                                .vezkc_address(r.vezkc_address)
                                .staking_rewards_address(r.staking_rewards_address)
                                .build()
                                .expect("Failed to build custom ZKC deployment")
                        }),
                    };
                }
            }
        }

        Ok(self)
    }

    /// Access [Self::rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.rpc_url
            .clone()
            .context("Rewards RPC URL not provided.\n\nTo configure: run 'boundless setup rewards'\nOr set --rewards-rpc-url flag or REWARDS_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Rewards private key not provided.\n\nTo configure: run 'boundless setup rewards'\nOr set --rewards-private-key flag or REWARDS_PRIVATE_KEY env var",
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

/// Configuration options for commands that utilize proving.
#[derive(Args, Debug, Clone)]
pub struct ProverConfig {
    /// RPC URL for the prover network
    #[clap(long = "prover-rpc-url", env = "PROVER_RPC_URL")]
    #[clap(help = "Also supports RPC_URL env var")]
    pub rpc_url: Option<Url>,

    /// Private key for prover transactions
    #[clap(long = "prover-private-key", env = "PROVER_PRIVATE_KEY", hide_env_values = true)]
    #[clap(help = "Also supports PRIVATE_KEY env var")]
    pub private_key: Option<PrivateKeySigner>,

    /// Configuration for the Boundless deployment to use.
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,

    // NOTE: BONSAI_x environment variables are used to avoid breaking workflows when this changed
    // from "bonsai" to "bento". There is not a clap-native way of providing env var aliases.
    /// Bento API URL
    ///
    /// URL at which your Bento cluster is running.
    #[clap(
        long,
        env = "BONSAI_API_URL",
        visible_alias = "bonsai-api-url",
        default_value = DEFAULT_BENTO_API_URL
    )]
    pub bento_api_url: String,

    /// Bento API Key
    ///
    /// Not necessary if using Bento without authentication, which is the default.
    #[clap(long, env = "BONSAI_API_KEY", visible_alias = "bonsai-api-key", hide_env_values = true)]
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

impl ProverConfig {
    /// Load configuration from prover config files
    pub fn load_from_files(mut self) -> Result<Self> {
        use crate::config_file::{Config, Secrets};

        let config = Config::load().ok();
        let secrets = Secrets::load().ok();

        if self.rpc_url.is_none() {
            // Try PROVER_RPC_URL, then RPC_URL, then config file
            if let Ok(rpc_url) = std::env::var("PROVER_RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Ok(rpc_url) = std::env::var("RPC_URL") {
                self.rpc_url = Some(Url::parse(&rpc_url)?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref prover_secrets) = secrets.prover {
                    if let Some(ref rpc_url) = prover_secrets.rpc_url {
                        self.rpc_url = Some(Url::parse(rpc_url)?);
                    }
                } else if let Some(ref market_secrets) = secrets.market {
                    if let Some(ref rpc_url) = market_secrets.rpc_url {
                        self.rpc_url = Some(Url::parse(rpc_url)?);
                    }
                }
            }
        }

        if self.private_key.is_none() {
            // Try PROVER_PRIVATE_KEY, then PRIVATE_KEY, then config file
            if let Ok(pk) = std::env::var("PROVER_PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Ok(pk) = std::env::var("PRIVATE_KEY") {
                self.private_key = Some(pk.parse()?);
            } else if let Some(ref secrets) = secrets {
                if let Some(ref prover_secrets) = secrets.prover {
                    if let Some(ref pk) = prover_secrets.private_key {
                        self.private_key = Some(pk.parse()?);
                    }
                } else if let Some(ref market_secrets) = secrets.market {
                    if let Some(ref pk) = market_secrets.private_key {
                        self.private_key = Some(pk.parse()?);
                    }
                }
            }
        }

        if self.deployment.is_none() {
            if let Some(ref config) = config {
                let network = config
                    .prover
                    .as_ref()
                    .map(|p| &p.network)
                    .or_else(|| config.market.as_ref().map(|m| &m.network));

                if let Some(network) = network {
                    self.deployment = match network.as_str() {
                        "base-mainnet" => Some(boundless_market::deployments::BASE),
                        "base-sepolia" => Some(boundless_market::deployments::BASE_SEPOLIA),
                        "eth-sepolia" => Some(boundless_market::deployments::SEPOLIA),
                        custom => config.custom_markets.iter().find(|m| m.name == custom).map(|m| {
                            let mut builder = boundless_market::Deployment::builder();
                            builder
                                .chain_id(m.chain_id)
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
                        }),
                    };
                }
            }
        }

        Ok(self)
    }

    /// Access [Self::rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.rpc_url
            .clone()
            .context("Prover RPC URL not provided.\n\nTo configure: run 'boundless setup prover'\nOr set --prover-rpc-url flag or PROVER_RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Prover private key not provided.\n\nTo configure: run 'boundless setup prover'\nOr set --prover-private-key flag or PROVER_PRIVATE_KEY env var",
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
        if self.use_default_prover {
            tracing::info!(
                "Using default prover behavior (respects RISC0_PROVER, RISC0_DEV_MODE, etc.)"
            );
            return;
        }

        println!("Using Bento prover at {}", self.bento_api_url);
        std::env::set_var("BONSAI_API_URL", &self.bento_api_url);
        if let Some(ref api_key) = self.bento_api_key {
            std::env::set_var("BONSAI_API_KEY", api_key);
        } else {
            tracing::debug!("No API key provided. Setting BONSAI_API_KEY to empty string");
            std::env::set_var("BONSAI_API_KEY", "");
        }
    }

    /// Sets environment variables to configure the prover and runs a health check.
    pub async fn configure_proving_backend_with_health_check(&self) -> anyhow::Result<()> {
        if self.use_default_prover || self.skip_health_check || ProverOpts::default().dev_mode() {
            return Ok(());
        }

        let using_default_url = self.bento_api_url == DEFAULT_BENTO_API_URL;

        let bento_url = Url::parse(&self.bento_api_url)
            .with_context(|| format!("Failed to parse Bento API URL: {}", self.bento_api_url))?;
        let health_check_url = bento_url.join("health")?;
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
