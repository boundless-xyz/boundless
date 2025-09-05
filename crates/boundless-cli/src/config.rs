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

/// Common configuration options for all commands
#[derive(Args, Debug, Clone)]
pub struct GlobalConfig {
    /// URL of the Ethereum RPC endpoint
    #[clap(short, long, env = "RPC_URL")]
    pub rpc_url: Option<Url>,

    /// Private key of the wallet (without 0x prefix)
    #[clap(long, env = "PRIVATE_KEY", global = true, hide_env_values = true)]
    pub private_key: Option<PrivateKeySigner>,

    /// Ethereum transaction timeout in seconds.
    #[clap(long, env = "TX_TIMEOUT", global = true, value_parser = |arg: &str| -> Result<Duration, ParseIntError> {Ok(Duration::from_secs(arg.parse()?))})]
    pub tx_timeout: Option<Duration>,

    /// Log level (error, warn, info, debug, trace)
    #[clap(long, env = "LOG_LEVEL", global = true, default_value = "info")]
    pub log_level: LevelFilter,

    /// Configuration for the Boundless deployment to use.
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,
}

impl GlobalConfig {
    // NOTE: It does not appear this is possible to specify the required dependencies with clap
    // natively. There is _some_ ability to use the #[group(requires = _)] attribute to do this,
    // but experimentation as of August 26, 2025 shows this is error prone and potentially buggy.

    /// Access [Self::rpc_url] or return an error that can be shown to the user.
    pub fn require_rpc_url(&self) -> Result<Url> {
        self.rpc_url
            .clone()
            .context("Blockchain RPC URL not provided; please set --rpc-url or the RPC_URL env var")
    }

    /// Access [Self::private_key] or return an error that can be shown to the user.
    pub fn require_private_key(&self) -> Result<PrivateKeySigner> {
        self.private_key.clone().context(
            "Private key not provided; please set --private-key or the PRIVATE_KEY env var",
        )
    }

    /// Create a parially initialzed [ClientBuilder] from the options in this struct.
    ///
    /// Requures [Self::rpc_url] to be set.
    pub fn client_builder(&self) -> Result<ClientBuilder> {
        Ok(Client::builder()
            .with_rpc_url(self.require_rpc_url()?)
            .with_deployment(self.deployment.clone())
            .with_timeout(self.tx_timeout))
    }

    /// Create a parially initialzed [ClientBuilder] from the options in this struct.
    ///
    /// Requures [Self::rpc_url] and [Self::private_key] to be set.
    pub fn client_builder_with_signer(
        &self,
    ) -> Result<ClientBuilder<NotProvided, PrivateKeySigner>> {
        Ok(self.client_builder()?.with_private_key(self.require_private_key()?))
    }

    /// Build a Boundless [Client] that can be used to query the Boundless smart contracts.
    ///
    /// The client built with this method is not able to sign transactions or requests.jUse
    /// [Self::build_client_with_signer] if signing is required.
    pub async fn build_client(
        &self,
    ) -> Result<
        Client<
            DynProvider,
            NotProvided,
            StandardRequestBuilder<DynProvider, NotProvided>,
            NotProvided,
        >,
    > {
        self.client_builder()?.build().await.context("Failed to build Boundless client")
    }

    /// Build a Boundless [Client] that can be used to query the Boundless smart contracts, and to
    /// sign requests and send transactions.
    pub async fn build_client_with_signer(
        &self,
    ) -> Result<
        Client<
            DynProvider,
            NotProvided,
            StandardRequestBuilder<DynProvider, NotProvided>,
            PrivateKeySigner,
        >,
    > {
        self.client_builder_with_signer()?.build().await.context("Failed to build Boundless client")
    }
}

const DEFAULT_BENTO_API_URL: &str = "http://localhost:8081";

/// Configuration options for commands that utilize proving.
#[derive(Args, Debug, Clone)]
pub struct ProverConfig {
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

impl ProverConfig {
    /// Sets environment variables BONSAI_API_URL and BONSAI_API_KEY environmen variables that are
    /// read by `default_prover()` when constructing the prover. Note that this is the only builtin
    /// way to do this.
    pub fn configure_proving_backend(&self) {
        if self.use_default_prover {
            tracing::info!(
                "Using default prover behavior (respects RISC0_PROVER, RISC0_DEV_MODE, etc.)"
            );
            return;
        }

        tracing::info!("Using Bento prover at {}", self.bento_api_url);
        std::env::set_var("BONSAI_API_URL", &self.bento_api_url);
        if let Some(ref api_key) = self.bento_api_key {
            std::env::set_var("BONSAI_API_KEY", api_key);
        } else {
            tracing::debug!("No API key provided. Setting BONSAI_API_KEY to empty string");
            std::env::set_var("BONSAI_API_KEY", "");
        }
    }

    /// Sets environment variables to configure the prover (see [configure_proving_backend]) and
    /// additionally runs a basic health check to make sure it can connect to Bento, if in use.
    ///
    /// This method is intended to give a slightly nicer error message if Bento is not running,
    /// expecially if they did not actually mean to use Bento.
    pub async fn configure_proving_backend_with_health_check(&self) -> anyhow::Result<()> {
        // No health check is implemented for default prover. If dev mode is set, then we are going
        // to use the dev mode prover anyway, so don't run the health check.
        if self.use_default_prover || self.skip_health_check || ProverOpts::default().dev_mode() {
            return Ok(());
        }

        // NOTE: If they are using the default, it is more likely they don't have Bento running.
        let using_default_url = self.bento_api_url == DEFAULT_BENTO_API_URL;

        // Send a request to the /health endpoint.
        let bento_url = Url::parse(&self.bento_api_url)
            .with_context(|| format!("Failed to parse Bento API URL: {}", self.bento_api_url))?;
        let health_check_url = bento_url.join("health")?;
        reqwest::get(health_check_url.clone())
            .await
            .with_context(|| match using_default_url {
                true => format!("Failed to send health check reqest to {health_check_url}; You can set --use-default-prover to use a local prover"),
                false => format!("Failed to send health check reqest to {health_check_url}"),
            })?
            .error_for_status()
            .context("Bento health check endpoint returned error status")?;

        Ok(())
    }
}
