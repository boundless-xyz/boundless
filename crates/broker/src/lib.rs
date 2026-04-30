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

use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use alloy::{
    network::{AnyNetwork, Ethereum},
    primitives::{utils::parse_ether, Address, Bytes, FixedBytes, U256},
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
use alloy_chains::NamedChain;
use anyhow::{Context, Result};
use boundless_market::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::{boundless_market::BoundlessMarketService, ProofRequest},
    dynamic_gas_filler::{DynamicGasFiller, PriorityMode},
    nonce_layer::NonceProvider,
    order_stream_client::OrderStreamClient,
    selector::{is_blake3_groth16_selector, is_groth16_selector},
    storage::StorageDownloader,
    Deployment,
};
use chrono::{serde::ts_seconds, DateTime, Utc};
use clap::Parser;
pub use config::Config;
pub use config::ConfigLock;
use config::{ConfigWatcher, TelemetryMode};
use provers::ProverObj;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::sha::Digest;
pub use rpc_retry_policy::CustomRetryPolicy;
use rpcmetrics::RpcMetricsLayer;
use sequential_fallback::SequentialFallbackLayer;
use serde::{Deserialize, Serialize};
use storage::ConfigurableDownloader;
use tokio::{sync::broadcast, sync::mpsc, sync::RwLock};
use tokio_util::sync::CancellationToken;
use tower::{Layer, ServiceBuilder};
use url::Url;

const NEW_ORDER_CHANNEL_CAPACITY: usize = 1000;
const ORDER_STATE_CHANNEL_CAPACITY: usize = 1000;
const TELEMETRY_CHANNEL_CAPACITY: usize = 40960;
const PRICER_CHANNEL_CAPACITY: usize = 1000;
const COMPLETION_CHANNEL_CAPACITY: usize = 1000;
const COMMITMENT_CHANNEL_CAPACITY: usize = 1000;
const COMMITMENT_COMPLETION_CHANNEL_CAPACITY: usize = 1000;

pub(crate) mod aggregator;
pub(crate) mod block_history;
pub(crate) mod chain_monitor;
pub(crate) mod chain_monitor_v2;
pub(crate) mod channels;
pub mod config;
mod db;
pub use db::{broker_sqlite_url_for_chain, DbObj, SqliteDb};
pub(crate) mod errors;
pub mod futures_retry;
pub(crate) mod market_monitor;
pub(crate) mod offchain_market_monitor;
pub(crate) mod order_committer;
pub(crate) mod order_evaluator;
pub(crate) mod order_locker;
pub(crate) mod order_pricer;
mod price_oracle;
pub(crate) mod prioritization;
pub mod provers;
pub(crate) mod proving;
pub(crate) mod reaper;
pub(crate) mod requestor_monitor;
pub(crate) mod rpc_retry_policy;
pub mod rpcmetrics;
pub mod sequential_fallback;
pub(crate) mod service_runner;
pub(crate) mod storage;
pub(crate) mod submitter;
pub(crate) mod task;
pub(crate) mod telemetry;
pub mod utils;
pub mod version_check;

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

    /// Deprecated and ignored: ChainMonitorV2 (eth_getBlockReceipts) is now the default.
    /// Pass `--legacy-rpc` to opt back into the legacy ChainMonitorService + MarketMonitor pair.
    /// Kept for backwards compatibility with existing scripts and inventories.
    #[clap(long, default_value_t = true, hide = true)]
    pub experimental_rpc: bool,

    /// Use the legacy ChainMonitorService + MarketMonitor pair (eth_getLogs based).
    /// The default is ChainMonitorV2, which uses eth_getBlockReceipts in a single polling loop.
    #[clap(long, default_value_t = false)]
    pub legacy_rpc: bool,

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

/// Resolve deployment configuration for a given chain ID.
pub fn resolve_deployment(
    args_deployment: Option<&Deployment>,
    chain_id: u64,
) -> Result<Deployment> {
    if let Some(manual_deployment) = args_deployment {
        if let Some(expected_deployment) = Deployment::from_chain_id(chain_id) {
            validate_deployment_config(manual_deployment, &expected_deployment, chain_id);
        } else {
            tracing::info!(
                "Using manually configured deployment for chain ID {chain_id} (no default available)"
            );
        }
        Ok(manual_deployment.clone())
    } else {
        Deployment::from_chain_id(chain_id).with_context(|| {
            format!(
                "No default deployment found for chain ID {chain_id}. \
                 Please specify deployment configuration manually."
            )
        })
    }
}

/// Status of a persistent order as it moves through the lifecycle in the database.
/// Orders in initial, intermediate, or terminal non-failure states (e.g. New, Pricing, Done, Skipped)
/// are managed in-memory or removed from the database.
#[derive(Clone, Copy, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
pub enum OrderStatus {
    /// Order is ready to commence proving (either locked or filling without locking)
    PendingProving,
    /// Order is actively ready for proving
    Proving,
    /// Order is ready for aggregation
    PendingAgg,
    /// Order is in the process of Aggregation
    Aggregating,
    /// Unaggregated order is ready for submission
    SkipAggregation,
    /// Pending on chain finalization
    PendingSubmission,
    /// Order has been completed
    Done,
    /// Order failed
    Failed,
    /// Order was analyzed and marked as skipable
    Skipped,
}

pub use boundless_market::prover_utils::{
    Erc1271GasCache, FulfillmentType, MarketConfig, OrderPricingContext, OrderPricingError,
    OrderPricingOutcome, OrderRequest, PreflightCache, PreflightCacheKey, PreflightCacheValue,
    ProveLimitReason,
};
use service_runner::ServiceRunner;

/// On-chain order state change broadcast from MarketMonitor to all unified components
/// (OrderEvaluator, OrderPricer, OrderCommitter) and the ProvingService.
///
/// Each variant carries `chain_id` to ensure that events on one chain don't affect
/// pending orders for other chains that may share the same `request_id`.
#[derive(Debug, Clone)]
pub enum OrderStateChange {
    Locked { request_id: U256, prover: Address, chain_id: u64 },
    Fulfilled { request_id: U256, chain_id: u64 },
}

impl OrderStateChange {
    /// Returns the chain that produced this state change event.
    pub fn chain_id(&self) -> u64 {
        match self {
            OrderStateChange::Locked { chain_id, .. } => *chain_id,
            OrderStateChange::Fulfilled { chain_id, .. } => *chain_id,
        }
    }
}

/// Helper function to format an order ID consistently
fn format_order_id(
    request_id: &U256,
    signing_hash: &FixedBytes<32>,
    fulfillment_type: &FulfillmentType,
) -> String {
    format!("0x{request_id:x}-{signing_hash}-{fulfillment_type:?}")
}

pub(crate) fn order_from_request(order_request: &OrderRequest, status: OrderStatus) -> Order {
    Order {
        boundless_market_address: order_request.boundless_market_address,
        chain_id: order_request.chain_id,
        fulfillment_type: order_request.fulfillment_type,
        request: order_request.request.clone(),
        status,
        client_sig: order_request.client_sig.clone(),
        updated_at: Utc::now(),
        image_id: order_request.image_id.clone(),
        input_id: order_request.input_id.clone(),
        total_cycles: order_request.total_cycles,
        journal_bytes: order_request.journal_bytes,
        target_timestamp: order_request.target_timestamp,
        expire_timestamp: order_request.expire_timestamp,
        proving_started_at: None,
        proof_id: None,
        compressed_proof_id: None,
        lock_price: None,
        error_msg: None,
        cached_id: OnceLock::new(),
    }
}

pub(crate) fn proving_order_from_request(order_request: &OrderRequest, lock_price: U256) -> Order {
    let mut order = order_from_request(order_request, OrderStatus::PendingProving);
    order.lock_price = Some(lock_price);
    order.proving_started_at = Some(Utc::now().timestamp().try_into().unwrap());
    order
}

/// An Order represents a proof request and a specific method of fulfillment.
///
/// Requests can be fulfilled in multiple ways, for example by locking then fulfilling them,
/// by waiting for an existing lock to expire then fulfilling for slashed stake, or by fulfilling
/// without locking at all.
///
/// For a given request, each type of fulfillment results in a separate Order being created, with different
/// FulfillmentType values.
///
/// Additionally, there may be multiple requests with the same request_id, but different ProofRequest
/// details. Those also result in separate Order objects being created.
///
/// See the id() method for more details on how Orders are identified.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Order {
    /// Address of the boundless market contract. Stored as it is required to compute the order id.
    boundless_market_address: Address,
    /// Chain ID of the boundless market contract. Stored as it is required to compute the order id.
    chain_id: u64,
    /// Fulfillment type
    fulfillment_type: FulfillmentType,
    /// Proof request object
    request: ProofRequest,
    /// status of the order
    status: OrderStatus,
    /// Last update time
    #[serde(with = "ts_seconds")]
    updated_at: DateTime<Utc>,
    /// Total cycles
    /// Populated after initial pricing in order picker
    total_cycles: Option<u64>,
    /// Journal size in bytes. Populated after preflight.
    #[serde(default)]
    journal_bytes: Option<usize>,
    /// Locking status target UNIX timestamp
    target_timestamp: Option<u64>,
    /// When proving was commenced at
    proving_started_at: Option<u64>,
    /// Prover image Id
    ///
    /// Populated after preflight
    image_id: Option<String>,
    /// Input Id
    ///
    ///  Populated after preflight
    input_id: Option<String>,
    /// Proof Id
    ///
    /// Populated after proof completion
    proof_id: Option<String>,
    /// Compressed proof Id
    ///
    /// Populated after proof completion. if the proof is compressed
    compressed_proof_id: Option<String>,
    /// UNIX timestamp the order expires at
    ///
    /// Populated during order picking
    expire_timestamp: Option<u64>,
    /// Client Signature
    client_sig: Bytes,
    /// Price the lockin was set at
    lock_price: Option<U256>,
    /// Failure message
    error_msg: Option<String>,
    #[serde(skip)]
    cached_id: OnceLock<String>,
}

impl Order {
    // An Order is identified by the request_id, the fulfillment type, and the hash of the proof request.
    // This structure supports multiple different ProofRequests with the same request_id, and different
    // fulfillment types.
    pub fn id(&self) -> String {
        self.cached_id
            .get_or_init(|| {
                let signing_hash = self
                    .request
                    .signing_hash(self.boundless_market_address, self.chain_id)
                    .unwrap();
                format_order_id(&self.request.id, &signing_hash, &self.fulfillment_type)
            })
            .clone()
    }

    pub fn is_groth16(&self) -> bool {
        is_groth16_selector(self.request.requirements.selector)
    }
    fn is_blake3_groth16(&self) -> bool {
        is_blake3_groth16_selector(self.request.requirements.selector)
    }
    pub fn compression_type(&self) -> CompressionType {
        if self.is_groth16() {
            CompressionType::Groth16
        } else if self.is_blake3_groth16() {
            CompressionType::Blake3Groth16
        } else {
            CompressionType::None
        }
    }
}

impl std::fmt::Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total_mcycles = if self.total_cycles.is_some() {
            format!(" ({} mcycles)", self.total_cycles.unwrap() / 1_000_000)
        } else {
            "".to_string()
        };
        write!(f, "{}{} [{}]", self.id(), total_mcycles, format_expiries(&self.request))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CompressionType {
    None,
    Groth16,
    Blake3Groth16,
}

#[derive(sqlx::Type, Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum BatchStatus {
    #[default]
    Aggregating,
    PendingCompression,
    Complete,
    PendingSubmission,
    Submitted,
    Failed,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct AggregationState {
    pub guest_state: risc0_aggregation::GuestState,
    /// All claim digests in this aggregation.
    /// This collection can be used to construct the aggregation Merkle tree and Merkle paths.
    pub claim_digests: Vec<Digest>,
    /// Proof ID for the STARK proof that compresses the root of the aggregation tree.
    pub proof_id: String,
    /// Proof ID for the Groth16 proof that compresses the root of the aggregation tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub groth16_proof_id: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Batch {
    pub status: BatchStatus,
    /// Orders from the market that are included in this batch.
    pub orders: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assessor_proof_id: Option<String>,
    /// Tuple of the current aggregation state, as committed by the set builder guest, and the
    /// proof ID for the receipt that attests to the correctness of this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregation_state: Option<AggregationState>,
    /// When the batch was initially created.
    pub start_time: DateTime<Utc>,
    /// The deadline for the batch, which is the earliest deadline for any order in the batch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
    /// The total fees for the batch, which is the sum of fees from all orders.
    pub fees: U256,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_msg: Option<String>,
}

pub struct Broker {
    args: CoreArgs,
    config_watcher: ConfigWatcher,
    downloader: ConfigurableDownloader,
}

impl Broker {
    pub async fn new(args: CoreArgs, config_watcher: ConfigWatcher) -> Result<Self> {
        let downloader = ConfigurableDownloader::new(config_watcher.config.clone())
            .await
            .context("Failed to initialize downloader")?;

        Ok(Self { args, config_watcher, downloader })
    }

    async fn fetch_and_upload_set_builder_image<P>(
        &self,
        prover: &ProverObj,
        provider: &Arc<P>,
        deployment: &Deployment,
        config: &ConfigLock,
    ) -> Result<Digest>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let set_verifier_contract = SetVerifierService::new(
            deployment.set_verifier_address,
            provider.clone(),
            Address::ZERO,
        );

        let (image_id, image_url_str) = set_verifier_contract
            .image_info()
            .await
            .context("Failed to get set builder image_info")?;
        let image_id = Digest::from_bytes(image_id.0);
        let (path, default_url) = {
            let config = config.lock_all().context("Failed to lock config")?;
            (
                config.prover.set_builder_guest_path.clone(),
                config.market.set_builder_default_image_url.clone(),
            )
        };

        self.fetch_and_upload_image(
            "set builder",
            prover,
            image_id,
            image_url_str,
            path,
            default_url,
        )
        .await
        .context("uploading set builder image")?;
        Ok(image_id)
    }

    async fn fetch_and_upload_assessor_image<P>(
        &self,
        prover: &ProverObj,
        provider: &Arc<P>,
        deployment: &Deployment,
        config: &ConfigLock,
    ) -> Result<Digest>
    where
        P: Provider<Ethereum> + Clone + 'static,
    {
        let boundless_market = BoundlessMarketService::new_for_broker(
            deployment.boundless_market_address,
            provider.clone(),
            Address::ZERO,
        );
        let (image_id, image_url_str) =
            boundless_market.image_info().await.context("Failed to get assessor image_info")?;
        let image_id = Digest::from_bytes(image_id.0);

        let (path, default_url) = {
            let config = config.lock_all().context("Failed to lock config")?;
            (
                config.prover.assessor_set_guest_path.clone(),
                config.market.assessor_default_image_url.clone(),
            )
        };

        self.fetch_and_upload_image("assessor", prover, image_id, image_url_str, path, default_url)
            .await
            .context("uploading assessor image")?;
        Ok(image_id)
    }

    async fn fetch_and_upload_image(
        &self,
        image_label: &'static str,
        prover: &ProverObj,
        image_id: Digest,
        contract_url: String,
        program_path: Option<PathBuf>,
        default_url: String,
    ) -> Result<()> {
        if prover.has_image(&image_id.to_string()).await? {
            tracing::debug!("{} image {} already uploaded, skipping pull", image_label, image_id);
            return Ok(());
        }

        tracing::debug!("Fetching {} image", image_label);
        let program_bytes = if let Some(path) = program_path {
            // Read from local file if provided
            tokio::fs::read(&path)
                .await
                .with_context(|| format!("Failed to read program file: {}", path.display()))?
        } else {
            // Try default URL first, fall back to contract URL if it fails or ID doesn't match
            match self.download_image(&default_url, "default").await {
                Ok(bytes) => {
                    let computed_id = risc0_zkvm::compute_image_id(&bytes)
                        .context("Failed to compute image ID")?;
                    if computed_id == image_id {
                        tracing::debug!(
                            "Successfully verified {} image from default URL",
                            image_label
                        );
                        bytes
                    } else {
                        tracing::warn!(
                            "{} image ID mismatch from default URL: expected {}, got {}, falling back to contract URL",
                            image_label,
                            image_id,
                            computed_id
                        );
                        self.download_image(&contract_url, "contract").await?
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to download {} image from default URL: {}, falling back to contract URL",
                        image_label,
                        e
                    );
                    self.download_image(&contract_url, "contract").await?
                }
            }
        };

        // Final verification - ensure we have the correct image
        let computed_id =
            risc0_zkvm::compute_image_id(&program_bytes).context("Failed to compute image ID")?;

        if computed_id != image_id {
            anyhow::bail!(
                "{} image ID mismatch: expected {}, got {}",
                image_label,
                image_id,
                computed_id
            );
        }

        tracing::debug!("Uploading {} image to bento", image_label);
        prover
            .upload_image(&image_id.to_string(), program_bytes)
            .await
            .with_context(|| format!("Failed to upload {} image to prover", image_label))?;
        Ok(())
    }

    async fn download_image(&self, url: &str, source_name: &str) -> Result<Vec<u8>> {
        tracing::trace!("Attempting to download image from {}: {}", source_name, url);

        let bytes = self
            .downloader
            .download(url)
            .await
            .with_context(|| format!("Failed to download image from {}", source_name))?;

        tracing::trace!("Successfully downloaded image from {}", source_name);
        Ok(bytes)
    }

    fn handle_join_result(
        res: std::result::Result<Result<()>, tokio::task::JoinError>,
    ) -> Result<bool> {
        match res {
            Err(join_err) if join_err.is_cancelled() => {
                tracing::info!("Tokio task exited with cancellation status: {join_err:?}");
                Ok(true)
            }
            Err(join_err) => {
                tracing::error!("Tokio task exited with error status: {join_err:?}");
                anyhow::bail!("Task exited with error status: {join_err:?}")
            }
            Ok(status) => match status {
                Err(err) => {
                    tracing::error!("Task exited with error status: {err:?}");
                    anyhow::bail!("Task exited with error status: {err:?}")
                }
                Ok(()) => {
                    tracing::info!("Task exited with ok status");
                    Ok(false)
                }
            },
        }
    }

    fn build_provers(&self, config: &ConfigLock) -> Result<(ProverObj, ProverObj)> {
        let prover: ProverObj;
        let aggregation_prover: ProverObj;
        if is_dev_mode() {
            tracing::warn!(
                "WARNING: Running the Broker in dev mode does not generate valid receipts. \
                 Receipts generated from this process are invalid and should never be used in production."
            );
            prover = Arc::new(provers::DefaultProver::new());
            aggregation_prover = Arc::clone(&prover);
        } else if let (Some(bonsai_api_key), Some(bonsai_api_url)) =
            (self.args.bonsai_api_key.as_ref(), self.args.bonsai_api_url.as_ref())
        {
            tracing::info!("Configured to run with Bonsai backend");
            prover = Arc::new(
                provers::Bonsai::new(config.clone(), bonsai_api_url.as_ref(), bonsai_api_key)
                    .context("Failed to construct Bonsai client")?,
            );
            aggregation_prover = Arc::clone(&prover);
        } else if let Some(bento_api_url) = self.args.bento_api_url.as_ref() {
            tracing::info!("Configured to run with Bento backend");
            prover = Arc::new(
                provers::Bonsai::new(config.clone(), bento_api_url.as_ref(), "v1:reserved:1000")
                    .context("Failed to initialize Bento client")?,
            );
            // Initialize aggregation/snark prover with a higher reserved key to prioritize
            aggregation_prover = Arc::new(
                provers::Bonsai::new(config.clone(), bento_api_url.as_ref(), "v1:reserved:2000")
                    .context("Failed to initialize Bento client")?,
            );
        } else {
            prover = Arc::new(provers::DefaultProver::new());
            aggregation_prover = Arc::clone(&prover);
        }
        Ok((prover, aggregation_prover))
    }

    pub async fn start_service<P>(&self, chains: Vec<ChainPipeline<P>>) -> Result<()>
    where
        P: Provider<Ethereum> + WalletProvider + Clone + 'static,
    {
        if self.args.listen_only {
            tracing::warn!(
                "LISTEN-ONLY MODE: Broker will monitor the market and evaluate orders but will NOT \
                 lock, prove, or submit. No on-chain transactions will be sent."

            );
        }

        let base_config = self.config_watcher.config.clone();
        let (prover, aggregation_prover) = self.build_provers(&base_config)?;

        let mut runner = ServiceRunner::new(base_config.clone());

        let chain_dbs: Vec<DbObj> = chains.iter().map(|c| c.db.clone()).collect();

        // All chain monitors send discovered orders here; the evaluator reads from evaluator_order_rx.
        let (evaluator_order_tx, evaluator_order_rx) =
            channels::shared_channel(NEW_ORDER_CHANNEL_CAPACITY);
        // Pricers send preflight completions here; the evaluator reads from pricing_completion_rx to free capacity.
        let (pricing_completion_tx, pricing_completion_rx) =
            channels::shared_channel(COMPLETION_CHANNEL_CAPACITY);
        // Shared broadcast for on-chain lock/fulfill events. Each chain's MarketMonitor sends into
        // this channel, and both global singletons (OrderEvaluator, OrderCommitter) and per-chain
        // components (OrderPricer, ProvingService) subscribe. Per-chain subscribers must filter by
        // chain_id since they receive events from all chains.
        let (order_state_tx, _) = tokio::sync::broadcast::channel(ORDER_STATE_CHANNEL_CAPACITY);
        let mut chain_dispatchers: std::collections::HashMap<u64, mpsc::Sender<Box<OrderRequest>>> =
            std::collections::HashMap::new();

        // All per-chain pricers send priced orders here; the order committer reads from priced_orders_rx.
        let (priced_orders_tx, priced_orders_rx) =
            channels::shared_channel(COMMITMENT_CHANNEL_CAPACITY);
        // Proving pipeline components send completion signals here; the order committer reads to free capacity.
        let (proving_completion_tx, proving_completion_rx) =
            channels::shared_channel(COMMITMENT_COMPLETION_CHANNEL_CAPACITY);
        let mut locker_dispatchers: std::collections::HashMap<
            u64,
            mpsc::Sender<Box<OrderRequest>>,
        > = std::collections::HashMap::new();

        let mut priority_requestors_map: std::collections::HashMap<
            u64,
            requestor_monitor::PriorityRequestors,
        > = std::collections::HashMap::new();

        for chain in &chains {
            // Evaluator dispatches capacity-gated orders to this chain's pricer via pricer_tx.
            let (pricer_tx, pricer_rx) = channels::shared_channel(PRICER_CHANNEL_CAPACITY);
            chain_dispatchers.insert(chain.chain_id, pricer_tx);

            // Order committer dispatches capacity-gated orders to this chain's locker.
            let (locker_tx, locker_rx) = channels::shared_channel(COMMITMENT_CHANNEL_CAPACITY);
            locker_dispatchers.insert(chain.chain_id, locker_tx);

            let priority_requestors =
                requestor_monitor::PriorityRequestors::new(chain.config.clone(), chain.chain_id);
            priority_requestors_map.insert(chain.chain_id, priority_requestors.clone());

            self.start_chain_pipeline(
                chain,
                prover.clone(),
                aggregation_prover.clone(),
                evaluator_order_tx.clone(),
                pricer_rx,
                pricing_completion_tx.clone(),
                order_state_tx.clone(),
                priced_orders_tx.clone(),
                locker_rx,
                proving_completion_tx.clone(),
                priority_requestors,
                &mut runner,
            )
            .await?;
        }
        // Drop our clones so the unified components see channel closure when all producers exit.
        drop(evaluator_order_tx);
        drop(pricing_completion_tx);
        drop(priced_orders_tx);
        drop(proving_completion_tx);

        let order_evaluator = Arc::new(order_evaluator::OrderEvaluator::new(
            base_config.clone(),
            evaluator_order_rx,
            chain_dispatchers,
            pricing_completion_rx,
            order_state_tx.clone(),
            priority_requestors_map.clone(),
        ));
        runner.spawn(order_evaluator, service_runner::Criticality::NonCritical, "order evaluator");

        let order_committer = Arc::new(order_committer::OrderCommitter::new(
            base_config.clone(),
            priced_orders_rx,
            locker_dispatchers,
            proving_completion_rx,
            order_state_tx,
            priority_requestors_map,
        ));
        runner.spawn(
            order_committer,
            service_runner::Criticality::NonCritical,
            "unified order committer",
        );

        // Decompose the runner to feed the shutdown loop and cancel-token machinery below.
        let service_runner::ServiceRunnerParts {
            mut non_critical_tasks,
            mut critical_tasks,
            non_critical_cancel_token,
            critical_cancel_token,
        } = runner.into_parts();

        // Monitor supervisor tasks and handle shutdown
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install SIGTERM handler");
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("Failed to install SIGINT handler");
        loop {
            tracing::info!("Waiting for supervisor tasks to complete...");
            tokio::select! {
                // Handle non-critical supervisor task results
                Some(res) = non_critical_tasks.join_next() => {
                    if Self::handle_join_result(res)? { continue; }
                }
                // Handle critical supervisor task results
                Some(res) = critical_tasks.join_next() => {
                    if Self::handle_join_result(res)? { continue; }
                }
                // Handle shutdown signals
                _ = tokio::signal::ctrl_c() => {
                    tracing::info!("Received CTRL+C, starting graceful shutdown...");
                    break;
                }
                _ = sigterm.recv() => {
                    tracing::info!("Received SIGTERM, starting graceful shutdown...");
                    break;
                }
                _ = sigint.recv() => {
                    tracing::info!("Received SIGINT, starting graceful shutdown...");
                    break;
                }
            }
        }

        // Phase 1: Cancel non-critical tasks immediately to stop taking new work
        tracing::info!("Cancelling non-critical tasks (order discovery, picking, monitoring)...");
        non_critical_cancel_token.cancel();

        tracing::info!("Waiting for non-critical tasks to exit...");
        let _ = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while non_critical_tasks.join_next().await.is_some() {}
        })
        .await
        .map_err(|_| {
            tracing::warn!(
                "Timed out waiting for non-critical tasks to exit; proceeding with critical shutdown"
            );
        });

        // Phase 2: Wait for committed orders to complete, then cancel critical tasks
        self.shutdown_and_cancel_critical_tasks(&chain_dbs, critical_cancel_token).await?;

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn start_chain_pipeline<P>(
        &self,
        chain: &ChainPipeline<P>,
        prover: ProverObj,
        aggregation_prover: ProverObj,
        evaluator_order_tx: mpsc::Sender<Box<OrderRequest>>,
        pricer_rx: channels::SharedReceiver<Box<OrderRequest>>,
        pricing_completion_tx: mpsc::Sender<order_evaluator::PreflightComplete>,
        order_state_tx: broadcast::Sender<OrderStateChange>,
        priced_orders_tx: mpsc::Sender<Box<OrderRequest>>,
        locker_rx: channels::SharedReceiver<Box<OrderRequest>>,
        proving_completion_tx: mpsc::Sender<order_committer::CommitmentComplete>,
        priority_requestors: requestor_monitor::PriorityRequestors,
        runner: &mut ServiceRunner,
    ) -> Result<()>
    where
        P: Provider<Ethereum> + WalletProvider + Clone + 'static,
    {
        let provider = chain.provider.clone();
        let config = chain.config.clone();
        let gas_priority_mode = chain.gas_priority_mode.clone();
        let gas_estimation_priority_mode = {
            let estimation_mode = config
                .lock_all()
                .map(|c| c.market.gas_estimation_priority_mode.clone())
                .unwrap_or_default();
            Arc::new(RwLock::new(estimation_mode))
        };
        let private_key = &chain.private_key;
        let chain_id = chain.chain_id;
        let deployment = &chain.deployment;
        let db = chain.db.clone();

        let chain_span = tracing::info_span!("chain", chain_id);

        {
            let task = Arc::new(version_check::VersionCheckTask::new(
                (*provider).clone(),
                chain_id,
                deployment.boundless_market_address,
                None,
                self.args.version_registry_address,
                self.args.force_version_check,
            ));
            runner.spawn_in_span(
                task,
                service_runner::Criticality::NonCritical,
                "version check",
                chain_span.clone(),
            );
        }

        let (lookback_blocks, events_poll_blocks, events_poll_ms) = {
            let config = config.lock_all().context("Failed to lock config")?;
            (
                config.market.lookback_blocks,
                config.market.events_poll_blocks,
                config.market.events_poll_ms,
            )
        };

        let order_stream_client = deployment
            .order_stream_url
            .clone()
            .map(|url| -> Result<OrderStreamClient> {
                let url = Url::parse(&url).context("Failed to parse order stream URL")?;
                Ok(OrderStreamClient::new(url, deployment.boundless_market_address, chain_id))
            })
            .transpose()?;

        let signer = private_key.clone();
        let telemetry_mode =
            config.lock_all().map(|c| c.market.telemetry_mode.clone()).unwrap_or_default();

        let _telemetry_handle = match telemetry_mode {
            TelemetryMode::Enabled => {
                if let Some(ref client) = order_stream_client {
                    telemetry::init_for_chain(chain_id, || {
                        let (tx, rx) =
                            mpsc::channel::<telemetry::TelemetryEvent>(TELEMETRY_CHANNEL_CAPACITY);
                        let handle = telemetry::TelemetryHandle::new(tx);
                        let service = telemetry::TelemetryService::new(
                            rx,
                            handle.clone(),
                            client.clone(),
                            signer.clone(),
                            config.clone(),
                            db.clone(),
                        );
                        // Telemetry rides on the critical cancel token so it keeps emitting
                        // broker heartbeats during the Phase 2 drain (in-flight committed
                        // orders). Otherwise monitoring sees the broker as offline within
                        // seconds of SIGTERM, while it is in fact still actively proving.
                        let cancel_token = runner.critical_cancel_token();
                        runner.spawn_future_in_span(
                            service_runner::Criticality::Critical,
                            "telemetry service",
                            chain_span.clone(),
                            async move {
                                telemetry::run_telemetry_service(service, cancel_token).await
                            },
                        );
                        handle
                    })
                } else {
                    tracing::warn!(
                        chain_id,
                        "TelemetryMode::Enabled requires order-stream; falling back to LogsOnly"
                    );

                    telemetry::init_for_chain(chain_id, || {
                        let (tx, rx) =
                            mpsc::channel::<telemetry::TelemetryEvent>(TELEMETRY_CHANNEL_CAPACITY);
                        let handle = telemetry::TelemetryHandle::new(tx);
                        let service = telemetry::TelemetryService::new_debug(
                            rx,
                            handle.clone(),
                            signer.clone(),
                            config.clone(),
                            db.clone(),
                        );
                        // See note above: telemetry on the critical token to survive
                        // the Phase 2 drain.
                        let cancel_token = runner.critical_cancel_token();
                        runner.spawn_future_in_span(
                            service_runner::Criticality::Critical,
                            "telemetry service",
                            chain_span.clone(),
                            async move {
                                telemetry::run_telemetry_service(service, cancel_token).await
                            },
                        );
                        handle
                    })
                }
            }
            TelemetryMode::LogsOnly => telemetry::init_for_chain(chain_id, || {
                let (tx, rx) =
                    mpsc::channel::<telemetry::TelemetryEvent>(TELEMETRY_CHANNEL_CAPACITY);
                let handle = telemetry::TelemetryHandle::new(tx);
                let service = telemetry::TelemetryService::new_debug(
                    rx,
                    handle.clone(),
                    signer.clone(),
                    config.clone(),
                    db.clone(),
                );
                // See note above: telemetry on the critical token to survive
                // the Phase 2 drain.
                let cancel_token = runner.critical_cancel_token();
                runner.spawn_future_in_span(
                    service_runner::Criticality::Critical,
                    "telemetry service",
                    chain_span.clone(),
                    async move { telemetry::run_telemetry_service(service, cancel_token).await },
                );
                handle
            }),
        };

        let prover_addr = provider.default_signer_address();

        // Set up chain + market monitoring. Default is ChainMonitorV2, a single polling loop
        // built on eth_getBlockReceipts. Pass `--legacy-rpc` to fall back to the older
        // ChainMonitorService + MarketMonitor pair (eth_getLogs).
        let (chain_monitor, block_times) = if !self.args.legacy_rpc {
            let monitor = Arc::new(
                chain_monitor_v2::ChainMonitorV2::new(
                    db.clone(),
                    provider.clone(),
                    Arc::new(chain.any_provider.clone()),
                    deployment.boundless_market_address,
                    private_key.address(),
                    lookback_blocks,
                    chain_id,
                    gas_priority_mode.clone(),
                    evaluator_order_tx.clone(),
                    order_state_tx.clone(),
                )
                .await
                .context("Failed to initialize ChainMonitorV2")?,
            );

            runner.spawn_in_span(
                monitor.clone(),
                service_runner::Criticality::Critical,
                "ChainMonitorV2",
                chain_span.clone(),
            );

            let block_times = monitor.block_time();
            (monitor as chain_monitor::ChainMonitorObj, block_times)
        } else {
            let chain_monitor_service = Arc::new(
                chain_monitor::ChainMonitorService::new(
                    provider.clone(),
                    gas_priority_mode.clone(),
                )
                .await
                .context("Failed to initialize chain monitor")?,
            );

            runner.spawn_in_span(
                chain_monitor_service.clone(),
                service_runner::Criticality::Critical,
                "chain monitor",
                chain_span.clone(),
            );

            let market_monitor = Arc::new(market_monitor::MarketMonitor::new(
                lookback_blocks,
                events_poll_blocks,
                events_poll_ms,
                deployment.boundless_market_address,
                provider.clone(),
                db.clone(),
                chain_monitor_service.clone(),
                private_key.address(),
                evaluator_order_tx.clone(),
                order_state_tx.clone(),
            ));

            let block_times =
                market_monitor.get_block_time().await.context("Failed to sample block times")?;

            runner.spawn_in_span(
                market_monitor,
                service_runner::Criticality::NonCritical,
                "market monitor",
                chain_span.clone(),
            );

            (chain_monitor_service as chain_monitor::ChainMonitorObj, block_times)
        };

        tracing::debug!(chain_id, "Estimated block time: {block_times}");

        if let Some(client) = order_stream_client {
            let offchain_market_monitor =
                Arc::new(offchain_market_monitor::OffchainMarketMonitor::new(
                    client,
                    private_key.clone(),
                    evaluator_order_tx.clone(),
                ));
            runner.spawn_in_span(
                offchain_market_monitor,
                service_runner::Criticality::NonCritical,
                "offchain market monitor",
                chain_span.clone(),
            );
        }

        let market = Arc::new(BoundlessMarketService::new_for_broker(
            deployment.boundless_market_address,
            DynProvider::new(provider.clone()),
            prover_addr,
        ));
        let collateral_token_decimals = market
            .collateral_token_decimals()
            .await
            .context("Failed to get stake token decimals. Possible RPC error.")?;

        let named_chain = NamedChain::try_from(chain_id)?;
        let price_oracle = Arc::new(
            config
                .lock_all()
                .unwrap()
                .price_oracle
                .build(named_chain, provider.clone())
                .context("Failed to build price oracle")?,
        );
        runner.spawn_in_span(
            price_oracle.clone(),
            service_runner::Criticality::NonCritical,
            "price oracle",
            chain_span.clone(),
        );

        let allow_requestors = requestor_monitor::AllowRequestors::new(config.clone(), chain_id);

        // Shared ERC-1271 gas cache between OrderPricer and OrderCommitter so that estimates
        // computed during preflight are reused when the committer locks the same order.
        let erc1271_gas_cache: Erc1271GasCache = Arc::new(
            moka::future::Cache::builder()
                .eviction_policy(moka::policy::EvictionPolicy::lru())
                .max_capacity(256)
                .time_to_live(std::time::Duration::from_secs(60 * 60))
                .build(),
        );

        let order_pricer = Arc::new(order_pricer::OrderPricer::new(
            db.clone(),
            config.clone(),
            prover.clone(),
            deployment.boundless_market_address,
            provider.clone(),
            chain_monitor.clone(),
            pricer_rx,
            priced_orders_tx,
            collateral_token_decimals,
            order_state_tx.clone(),
            priority_requestors.clone(),
            allow_requestors.clone(),
            self.downloader.clone(),
            price_oracle.clone(),
            erc1271_gas_cache.clone(),
            self.args.listen_only,
            chain_id,
            pricing_completion_tx,
        ));
        runner.spawn_in_span(
            order_pricer,
            service_runner::Criticality::NonCritical,
            "order pricer",
            chain_span.clone(),
        );

        let order_locker = Arc::new(order_locker::OrderLocker::new(
            db.clone(),
            provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_times,
            prover_addr,
            deployment.boundless_market_address,
            locker_rx,
            collateral_token_decimals,
            order_locker::RpcRetryConfig {
                retry_count: self.args.rpc_retry_max.into(),
                retry_sleep_ms: self.args.rpc_retry_backoff,
            },
            gas_priority_mode.clone(),
            gas_estimation_priority_mode,
            erc1271_gas_cache,
            self.args.listen_only,
            chain_id,
            proving_completion_tx.clone(),
        )?);
        runner.spawn_in_span(
            order_locker,
            service_runner::Criticality::NonCritical,
            "order locker",
            chain_span.clone(),
        );

        if !self.args.listen_only {
            let proving_service = Arc::new(proving::ProvingService::new(
                db.clone(),
                prover.clone(),
                aggregation_prover.clone(),
                config.clone(),
                order_state_tx.clone(),
                priority_requestors.clone(),
                market.clone(),
                self.downloader.clone(),
                chain_id,
                proving_completion_tx.clone(),
            ));

            runner.spawn_in_span(
                proving_service,
                service_runner::Criticality::Critical,
                "proving service",
                chain_span.clone(),
            );

            let set_builder_img_id = self
                .fetch_and_upload_set_builder_image(&prover, &provider, deployment, &config)
                .await?;
            let assessor_img_id = self
                .fetch_and_upload_assessor_image(&prover, &provider, deployment, &config)
                .await?;

            let aggregator = Arc::new(
                aggregator::AggregatorService::new(
                    db.clone(),
                    chain_id,
                    set_builder_img_id,
                    assessor_img_id,
                    deployment.boundless_market_address,
                    prover_addr,
                    config.clone(),
                    aggregation_prover.clone(),
                    proving_completion_tx.clone(),
                )
                .await
                .context("Failed to initialize aggregator service")?,
            );

            runner.spawn_in_span(
                aggregator,
                service_runner::Criticality::CriticalWithFastRetry,
                "aggregator service",
                chain_span.clone(),
            );

            // Start the ReaperTask to check for expired committed orders.
            // Using critical cancel token to ensure no stuck expired jobs on shutdown.
            let reaper = Arc::new(reaper::ReaperTask::new(
                db.clone(),
                config.clone(),
                prover.clone(),
                chain_id,
                proving_completion_tx.clone(),
            ));
            runner.spawn_in_span(
                reaper,
                service_runner::Criticality::Critical,
                "reaper service",
                chain_span.clone(),
            );

            let submitter = Arc::new(submitter::Submitter::new(
                db.clone(),
                config.clone(),
                aggregation_prover.clone(),
                provider.clone(),
                deployment.set_verifier_address,
                deployment.boundless_market_address,
                set_builder_img_id,
                chain_id,
                proving_completion_tx.clone(),
            )?);
            runner.spawn_in_span(
                submitter,
                service_runner::Criticality::CriticalWithFastRetry,
                "submitter service",
                chain_span.clone(),
            );
        }

        // Start the RequestorMonitor to periodically fetch priority and allow lists
        let requestor_monitor = Arc::new(requestor_monitor::RequestorMonitor::new(
            priority_requestors,
            allow_requestors,
        ));
        runner.spawn_in_span(
            requestor_monitor,
            service_runner::Criticality::NonCritical,
            "requestor list monitor",
            chain_span.clone(),
        );

        Ok(())
    }

    async fn shutdown_and_cancel_critical_tasks(
        &self,
        chain_dbs: &[DbObj],
        critical_cancel_token: CancellationToken,
    ) -> Result<(), anyhow::Error> {
        // 2 hour max to shutdown time, to avoid indefinite shutdown time.
        const SHUTDOWN_GRACE_PERIOD_SECS: u32 = 2 * 60 * 60;
        const SLEEP_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

        let start_time = std::time::Instant::now();
        let grace_period = std::time::Duration::from_secs(SHUTDOWN_GRACE_PERIOD_SECS as u64);
        let mut last_log = "".to_string();
        while start_time.elapsed() < grace_period {
            let mut in_progress_orders = Vec::new();
            for db in chain_dbs {
                in_progress_orders.extend(db.get_committed_orders().await?);
            }
            if in_progress_orders.is_empty() {
                break;
            }

            let new_log = format!(
                "Waiting for {} in-progress orders to complete...\n{}",
                in_progress_orders.len(),
                in_progress_orders
                    .iter()
                    .map(|order| { format!("[{:?}] {}", order.status, order) })
                    .collect::<Vec<_>>()
                    .join("\n")
            );

            if new_log != last_log {
                tracing::info!("{}", new_log);
                last_log = new_log;
            }

            tokio::time::sleep(SLEEP_DURATION).await;
        }

        // Cancel critical tasks after committed work completes (or timeout)
        tracing::info!("Cancelling critical tasks...");
        critical_cancel_token.cancel();

        if start_time.elapsed() >= grace_period {
            let mut in_progress_orders = Vec::new();
            for db in chain_dbs {
                in_progress_orders.extend(db.get_committed_orders().await?);
            }
            tracing::info!(
                "Shutdown timed out after {} seconds. Exiting with {} in-progress orders: {}",
                SHUTDOWN_GRACE_PERIOD_SECS,
                in_progress_orders.len(),
                in_progress_orders
                    .iter()
                    .map(|order| format!("[{:?}] {}", order.status, order))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        } else {
            tracing::info!("Shutdown complete");
        }
        Ok(())
    }
}

fn validate_deployment_config(manual: &Deployment, expected: &Deployment, chain_id: u64) {
    let mut warnings = Vec::new();

    if manual.boundless_market_address != expected.boundless_market_address {
        warnings.push(format!(
            "boundless_market_address mismatch: configured={}, expected={}",
            manual.boundless_market_address, expected.boundless_market_address
        ));
    }

    if manual.set_verifier_address != expected.set_verifier_address {
        warnings.push(format!(
            "set_verifier_address mismatch: configured={}, expected={}",
            manual.set_verifier_address, expected.set_verifier_address
        ));
    }

    if let (Some(manual_addr), Some(expected_addr)) =
        (manual.verifier_router_address, expected.verifier_router_address)
    {
        if manual_addr != expected_addr {
            warnings.push(format!(
                "verifier_router_address mismatch: configured={manual_addr}, expected={expected_addr}"
            ));
        }
    }

    if let (Some(manual_addr), Some(expected_addr)) =
        (manual.collateral_token_address, expected.collateral_token_address)
    {
        if manual_addr != expected_addr {
            warnings.push(format!(
                "collateral_token_address mismatch: configured={manual_addr}, expected={expected_addr}"
            ));
        }
    }

    if manual.order_stream_url != expected.order_stream_url {
        warnings.push(format!(
            "order_stream_url mismatch: configured={:?}, expected={:?}",
            manual.order_stream_url, expected.order_stream_url
        ));
    }

    if let (Some(chain_id), Some(expected_chain_id)) =
        (manual.market_chain_id, expected.market_chain_id)
    {
        if chain_id != expected_chain_id {
            warnings.push(format!(
                "market_chain_id mismatch: configured={chain_id}, expected={expected_chain_id}"
            ));
        }
    }

    if warnings.is_empty() {
        tracing::info!(
            "Manual deployment configuration matches expected defaults for chain ID {chain_id}"
        );
    } else {
        tracing::warn!(
            "Manual deployment configuration differs from expected defaults for chain ID {chain_id}: {}",
            warnings.join(", ")
        );
        tracing::warn!(
            "This may indicate a configuration error. Please verify your deployment addresses are correct."
        );
    }
}

/// A very small utility function to get the current unix timestamp in seconds.
// TODO(#379): Avoid drift relative to the chain's timestamps.
pub(crate) fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

// Utility function to format the expiries of a request in a human readable format
fn format_expiries(request: &ProofRequest) -> String {
    let now: i64 = now_timestamp().try_into().unwrap();
    let lock_expires_at: i64 = request.lock_expires_at().try_into().unwrap();
    let lock_expires_delta = lock_expires_at - now;
    let lock_expires_delta_str = if lock_expires_delta > 0 {
        format!("{lock_expires_delta} seconds from now")
    } else {
        format!("{} seconds ago", lock_expires_delta.abs())
    };
    let expires_at: i64 = request.expires_at().try_into().unwrap();
    let expires_at_delta = expires_at - now;
    let expires_at_delta_str = if expires_at_delta > 0 {
        format!("{expires_at_delta} seconds from now")
    } else {
        format!("{} seconds ago", expires_at_delta.abs())
    };
    format!(
        "lock expires at {lock_expires_at} ({lock_expires_delta_str}), expires at {expires_at} ({expires_at_delta_str})"
    )
}

/// Returns `true` if the dev mode environment variable is enabled.
pub(crate) fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(feature = "test-utils")]
pub mod test_utils {
    use std::sync::Arc;

    use alloy::{
        network::{AnyNetwork, Ethereum},
        providers::{
            fillers::ChainIdFiller, DynProvider, Provider, ProviderBuilder, WalletProvider,
        },
    };
    use anyhow::Result;
    use boundless_market::price_oracle::config::PriceValue;
    use boundless_market::price_oracle::Amount;
    use boundless_test_utils::{
        guests::{ASSESSOR_GUEST_PATH, SET_BUILDER_PATH},
        market::TestCtx,
    };
    use tempfile::NamedTempFile;
    use url::Url;

    use crate::{
        broker_sqlite_url_for_chain,
        config::{Config, ConfigWatcher},
        resolve_deployment, Broker, ChainPipeline, CoreArgs, DbObj, SqliteDb,
    };

    pub struct BrokerBuilder {
        args: CoreArgs,
        config_file: NamedTempFile,
        db_dir: tempfile::TempDir,
        rpc_url: Url,
    }

    impl BrokerBuilder {
        pub async fn new_test<P>(ctx: &TestCtx<P>, rpc_url: Url) -> Self {
            let config_file: NamedTempFile = NamedTempFile::new().unwrap();
            let mut config = Config::default();
            config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
            config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
            config.market.min_mcycle_price = Amount::parse("0.0 ETH", None).unwrap();
            config.batcher.min_batch_size = 1;
            config.market.min_deadline = 30;
            config.price_oracle.eth_usd = PriceValue::Static(2500.0);
            config.price_oracle.zkc_usd = PriceValue::Static(1.0);
            config.write(config_file.path()).await.unwrap();

            let db_dir = tempfile::tempdir().unwrap();
            let db_url = format!("sqlite://{}", db_dir.path().join("broker.sqlite").display());

            let args = CoreArgs {
                db_url,
                config_file: config_file.path().to_path_buf(),
                deployment: Some(ctx.deployment.clone()),
                rpc_url: Some(rpc_url.to_string()),
                rpc_urls: Vec::new(),
                private_key: Some(ctx.prover_signer.clone()),
                bento_api_url: None,
                bonsai_api_key: None,
                bonsai_api_url: None,
                deposit_amount: None,
                rpc_retry_max: 0,
                rpc_retry_backoff: 200,
                rpc_retry_cu: 1000,
                rpc_request_timeout: 30,
                log_json: false,
                listen_only: false,
                experimental_rpc: true,
                legacy_rpc: false,
                version_registry_address: Some(ctx.version_registry_address),
                force_version_check: false,
            };
            Self { args, config_file, db_dir, rpc_url }
        }

        pub fn with_db_url(mut self, db_url: String) -> Self {
            self.args.db_url = db_url;
            self
        }

        pub async fn build<P>(
            self,
            ctx: &TestCtx<P>,
        ) -> Result<(
            Broker,
            ChainPipeline<impl Provider<Ethereum> + WalletProvider + Clone + 'static>,
            NamedTempFile,
            tempfile::TempDir,
        )>
        where
            P: Provider<Ethereum> + WalletProvider + Clone + 'static,
        {
            let config_watcher = ConfigWatcher::new(self.config_file.path()).await?;
            let config = config_watcher.config.clone();
            let (provider, any_provider, gas_priority_mode) = crate::build_chain_provider(
                &[self.rpc_url],
                &ctx.prover_signer,
                &self.args,
                &config,
            )?;
            let provider = Arc::new(provider);
            let chain_id = provider.get_chain_id().await?;
            let deployment = resolve_deployment(self.args.deployment.as_ref(), chain_id)?;

            let db_url = broker_sqlite_url_for_chain(&self.args.db_url, chain_id)
                .map_err(|e| anyhow::anyhow!("invalid broker database URL: {e}"))?;
            let db: DbObj = Arc::new(
                SqliteDb::new(&db_url)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to open per-chain sqlite DB: {e}"))?,
            );

            let chain = ChainPipeline {
                provider,
                any_provider,
                config,
                gas_priority_mode,
                private_key: ctx.prover_signer.clone(),
                chain_id,
                deployment,
                db,
            };

            let broker = Broker::new(self.args, config_watcher).await?;
            Ok((broker, chain, self.config_file, self.db_dir))
        }
    }

    pub fn make_any_provider(rpc_url: Url) -> DynProvider<AnyNetwork> {
        DynProvider::new(
            ProviderBuilder::new()
                .network::<AnyNetwork>()
                .filler(ChainIdFiller::default())
                .connect_http(rpc_url),
        )
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    /// Ensures existing DB rows (serialized without journal_bytes) deserialize correctly.
    #[test]
    fn journal_bytes_backwards_compat() {
        use boundless_market::contracts::{
            Offer, Predicate, RequestId, RequestInput, Requirements,
        };
        use risc0_zkvm::sha::Digest;

        let request = ProofRequest::new(
            RequestId::new(Address::ZERO, 0),
            Requirements::new(Predicate::prefix_match(Digest::ZERO, Bytes::default())),
            "",
            RequestInput::inline(Bytes::new()),
            Offer::default(),
        );

        // Create an Order and serialize it to JSON.
        let order = Order {
            boundless_market_address: Address::ZERO,
            chain_id: 1,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            request,
            status: OrderStatus::PendingProving,
            updated_at: Utc::now(),
            total_cycles: Some(1000),
            journal_bytes: Some(42),
            target_timestamp: None,
            proving_started_at: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
            cached_id: OnceLock::new(),
        };
        let json = serde_json::to_string(&order).unwrap();

        // Remove journal_bytes from the JSON to simulate an old DB row.
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value.as_object_mut().unwrap().remove("journal_bytes");
        let json_without_field = serde_json::to_string(&value).unwrap();

        // Deserialize and verify journal_bytes defaults to None.
        let deserialized: Order = serde_json::from_str(&json_without_field).unwrap();
        assert!(deserialized.journal_bytes.is_none());

        // Verify that an explicit null also round-trips to None.
        let mut value_null: serde_json::Value = serde_json::from_str(&json).unwrap();
        value_null.as_object_mut().unwrap().insert("journal_bytes".into(), serde_json::Value::Null);
        let json_with_null = serde_json::to_string(&value_null).unwrap();
        let deserialized_null: Order = serde_json::from_str(&json_with_null).unwrap();
        assert!(deserialized_null.journal_bytes.is_none());
    }
}

#[cfg(test)]
pub mod tests;
