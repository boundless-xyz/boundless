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

//! CLI arguments and per-chain pipeline construction.
//!
//! [`CoreArgs`] is the clap-derived top-level CLI shape; [`ChainArgs`] holds
//! per-chain values that the binary parses dynamically (one set per chain ID).
//! [`build_chain_provider`] constructs the provider stack a chain pipeline
//! needs (typed provider + AnyNetwork provider + gas priority mode).
//! [`ChainPipeline`] is the bag of per-chain resources the binary passes into
//! `Broker::start_service`.

use std::{path::PathBuf, sync::Arc, time::Duration};

use alloy::{
    network::{AnyNetwork, Ethereum},
    primitives::{utils::parse_ether, Address, U256},
    providers::{
        fillers::ChainIdFiller, network::EthereumWallet, DynProvider, Provider, ProviderBuilder,
        WalletProvider,
    },
    rpc::client::RpcClient,
    signers::local::PrivateKeySigner,
    transports::{
        http::{reqwest, Http},
        layers::RetryBackoffLayer,
    },
};
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    dynamic_gas_filler::{DynamicGasFiller, PriorityMode},
    nonce_layer::NonceProvider,
    Deployment,
};
use clap::Parser;
use tokio::sync::RwLock;
use tower::{Layer, ServiceBuilder};
use url::Url;

use crate::{
    config::ConfigLock, db::DbObj, rpc_retry_policy::CustomRetryPolicy,
    rpcmetrics::RpcMetricsLayer, sequential_fallback::SequentialFallbackLayer,
};
#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
/// Clap-derived CLI args shared across single-chain and multi-chain modes.
/// Per-chain flags (--rpc-url-{chain_id}, etc.) are parsed dynamically in bin/broker.rs.
pub struct CoreArgs {
    /// Base SQLite URL for the broker. Each chain pipeline opens its own file or named in-memory DB
    /// derived from this value and the chain ID.
    #[clap(short = 's', long, env, default_value = "sqlite::memory:")]
    pub db_url: String,

    /// local prover API (Bento)
    ///
    /// Setting this value toggles using Bento for proving and disables Bonsai
    #[clap(long, env, default_value = "http://localhost:8081", conflicts_with_all = ["bonsai_api_url", "bonsai_api_key"]
    )]
    pub bento_api_url: Option<Url>,

    /// Bonsai API URL
    ///
    /// Toggling this disables Bento proving and uses Bonsai as a backend
    #[clap(long, env, conflicts_with = "bento_api_url")]
    pub bonsai_api_url: Option<Url>,

    /// Bonsai API Key
    ///
    /// Required if using BONSAI_API_URL
    #[clap(long, env, conflicts_with = "bento_api_url")]
    pub bonsai_api_key: Option<String>,

    /// Config file path
    #[clap(short, long, default_value = "broker.toml")]
    pub config_file: PathBuf,

    /// Pre deposit amount
    ///
    /// Amount of stake tokens to pre-deposit into the contract for staking eg: 100
    #[clap(short, long)]
    pub deposit_amount: Option<U256>,

    /// RPC HTTP retry rate limit max retry
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 10)]
    pub rpc_retry_max: u32,

    /// RPC HTTP retry backoff (in ms)
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 1000)]
    pub rpc_retry_backoff: u64,

    /// RPC HTTP retry compute-unit per second
    ///
    /// From the `RetryBackoffLayer` of Alloy
    #[clap(long, default_value_t = 100)]
    pub rpc_retry_cu: u64,

    /// RPC HTTP request timeout in seconds.
    ///
    /// Individual HTTP requests to the RPC endpoint will be aborted after this duration,
    /// allowing the retry layer to retry the request. Set to 0 to disable.
    #[clap(long, default_value_t = 15)]
    pub rpc_request_timeout: u64,

    /// Log JSON
    #[clap(long, env, default_value_t = false)]
    pub log_json: bool,

    /// Listen-only mode: monitor the market and evaluate orders without locking, proving, or submitting.
    ///
    /// Useful for testing market monitoring and order evaluation without on-chain stake or collateral.
    /// No transactions will be sent. Balance and collateral checks are skipped.
    #[clap(long, default_value_t = false)]
    pub listen_only: bool,

    /// VersionRegistry contract address override. Not a CLI flag — set programmatically in tests
    /// to exercise the version check against a locally deployed registry.
    #[clap(skip)]
    pub version_registry_address: Option<Address>,

    /// Force the version check even when RISC0_DEV_MODE is enabled.
    /// Useful for testing version enforcement locally.
    #[clap(long, env, default_value_t = false)]
    pub force_version_check: bool,

    /// [Deprecated: use --market-address-{chain_id} etc.] Single-chain deployment configuration.
    #[clap(flatten, next_help_heading = "Boundless Deployment (Deprecated)")]
    pub deployment: Option<Deployment>,

    /// [Deprecated: use --rpc-url-{chain_id}] Single-chain RPC URL.
    #[clap(long, env = "PROVER_RPC_URL")]
    pub rpc_url: Option<String>,

    /// [Deprecated: use --rpc-url-{chain_id}] Additional single-chain RPC URLs for failover.
    #[clap(long, env = "PROVER_RPC_URLS", value_delimiter = ',')]
    pub rpc_urls: Vec<String>,

    /// [Deprecated: use --private-key-{chain_id}] Single-chain wallet key.
    #[clap(long, env = "PROVER_PRIVATE_KEY", hide_env_values = true)]
    pub private_key: Option<PrivateKeySigner>,
}

/// Parsed per-chain configuration resolved from environment variables and CLI args.
#[derive(Debug, Clone)]
pub struct ChainArgs {
    pub chain_id: u64,
    pub rpc_urls: Vec<Url>,
    pub private_key: PrivateKeySigner,
    pub config_override_path: Option<PathBuf>,
    pub deployment: Option<Deployment>,
}

/// Build a provider stack for a single chain from its RPC URLs and private key.
///
/// Returns (typed provider, AnyNetwork provider, gas priority mode).
pub fn build_chain_provider(
    rpc_urls: &[Url],
    private_key: &PrivateKeySigner,
    args: &CoreArgs,
    config: &ConfigLock,
) -> Result<(
    impl Provider<Ethereum> + WalletProvider + Clone + 'static,
    DynProvider<AnyNetwork>,
    Arc<RwLock<PriorityMode>>,
)> {
    let wallet = EthereumWallet::from(private_key.clone());

    let retry_layer = RetryBackoffLayer::new_with_policy(
        args.rpc_retry_max,
        args.rpc_retry_backoff,
        args.rpc_retry_cu,
        CustomRetryPolicy,
    );

    let mut http_client_builder = reqwest::Client::builder();
    if args.rpc_request_timeout > 0 {
        http_client_builder =
            http_client_builder.timeout(Duration::from_secs(args.rpc_request_timeout));
    }
    let http_client = http_client_builder.build().expect("Failed to build HTTP client");

    let client = if rpc_urls.len() > 1 {
        let transports: Vec<_> = rpc_urls
            .iter()
            .map(|url| {
                RpcMetricsLayer::new().layer(Http::with_client(http_client.clone(), url.clone()))
            })
            .collect();

        tracing::info!(
            "Configuring chain with sequential fallback RPC support: {} URLs: {:?}",
            rpc_urls.len(),
            rpc_urls
        );

        let transport = ServiceBuilder::new()
            .layer(retry_layer)
            .layer(SequentialFallbackLayer)
            .service(transports);

        RpcClient::builder().transport(transport, false)
    } else {
        let single_url = &rpc_urls[0];
        tracing::info!("Configuring chain with single RPC URL: {}", single_url);
        let transport = ServiceBuilder::new().layer(retry_layer).service(
            RpcMetricsLayer::new().layer(Http::with_client(http_client, single_url.clone())),
        );
        RpcClient::builder().transport(transport, false)
    };

    let balance_alerts_config = {
        let config = config.lock_all().context("Failed to read config")?;
        BalanceAlertConfig {
            watch_address: wallet.default_signer().address(),
            warn_threshold: config
                .market
                .balance_warn_threshold
                .as_ref()
                .map(|s| parse_ether(s))
                .transpose()?,
            error_threshold: config
                .market
                .balance_error_threshold
                .as_ref()
                .map(|s| parse_ether(s))
                .transpose()?,
        }
    };
    let balance_alerts_layer = BalanceAlertLayer::new(balance_alerts_config);

    let priority_mode = {
        let config = config.lock_all().context("Failed to read config")?;
        config.market.gas_priority_mode.clone()
    };
    let dynamic_gas_filler =
        DynamicGasFiller::new(20, priority_mode, wallet.default_signer().address());

    let gas_priority_mode = dynamic_gas_filler.priority_mode.clone();

    let base_provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .filler(ChainIdFiller::default())
        .filler(dynamic_gas_filler)
        .layer(balance_alerts_layer)
        .connect_client(client.clone());

    let provider = NonceProvider::new(base_provider, wallet);

    let any_provider = DynProvider::new(
        ProviderBuilder::new()
            .network::<AnyNetwork>()
            .filler(ChainIdFiller::default())
            .connect_client(client),
    );

    Ok((provider, any_provider, gas_priority_mode))
}

/// Per-chain resources constructed by the binary and passed into `start_service`.
pub struct ChainPipeline<P> {
    pub provider: Arc<P>,
    pub any_provider: DynProvider<AnyNetwork>,
    pub config: ConfigLock,
    pub gas_priority_mode: Arc<RwLock<PriorityMode>>,
    pub private_key: PrivateKeySigner,
    pub chain_id: u64,
    pub deployment: Deployment,
    pub db: DbObj,
}
