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
    time::SystemTime,
};

use crate::storage::create_uri_handler;
use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, FixedBytes, U256},
    providers::{DynProvider, Provider, WalletProvider},
    signers::local::PrivateKeySigner,
};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{boundless_market::BoundlessMarketService, ProofRequest},
    dynamic_gas_filler::PriorityMode,
    order_stream_client::OrderStreamClient,
    override_gateway,
    selector::{is_blake3_groth16_selector, is_groth16_selector},
    Deployment,
};
use chrono::{serde::ts_seconds, DateTime, Utc};
use clap::Parser;
pub use config::Config;
use config::ConfigWatcher;
use db::{DbObj, SqliteDb};
use provers::ProverObj;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::sha::Digest;
pub use rpc_retry_policy::CustomRetryPolicy;
use serde::{Deserialize, Serialize};
use task::{RetryPolicy, Supervisor};
use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use url::Url;

const NEW_ORDER_CHANNEL_CAPACITY: usize = 1000;
const PRICING_CHANNEL_CAPACITY: usize = 1000;
const ORDER_STATE_CHANNEL_CAPACITY: usize = 1000;

pub(crate) mod aggregator;
pub(crate) mod chain_monitor;
pub mod config;
pub(crate) mod db;
pub(crate) mod errors;
pub mod futures_retry;
pub(crate) mod market_monitor;
pub(crate) mod offchain_market_monitor;
pub(crate) mod order_monitor;
pub(crate) mod order_picker;
pub(crate) mod prioritization;
pub mod provers;
pub(crate) mod proving;
pub(crate) mod reaper;
pub(crate) mod requestor_monitor;
pub(crate) mod rpc_retry_policy;
pub(crate) mod storage;
pub(crate) mod submitter;
pub(crate) mod task;
pub mod utils;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// sqlite database connection url
    #[clap(short = 's', long, env, default_value = "sqlite::memory:")]
    pub db_url: String,

    /// RPC URL (prefer PROVER_RPC_URL as environment variable; falls back to RPC_URL if unset)
    ///
    /// Note we use a String here as the type instead of Url, as the env variable may be
    /// set to an empty string, especially if using our Docker compose setup, which causes parsing errors with Url.
    #[clap(long, env = "PROVER_RPC_URL")]
    pub rpc_url: Option<String>,

    /// Additional RPC URLs for automatic failover.
    /// Can be specified multiple times or as a comma-separated list.
    /// If provided along with rpc_url, they will be merged into a single list.
    /// If 2+ URLs are provided total, a fallback provider will be used.
    #[clap(long, env = "PROVER_RPC_URLS", value_delimiter = ',')]
    pub rpc_urls: Vec<String>,

    /// Wallet key
    ///
    /// Can be set via PROVER_PRIVATE_KEY (preferred) or PRIVATE_KEY (backward compatibility) env vars
    #[clap(long, env = "PROVER_PRIVATE_KEY", hide_env_values = true)]
    pub private_key: Option<PrivateKeySigner>,

    /// Boundless deployment configuration (contract addresses, etc.)
    #[clap(flatten, next_help_heading = "Boundless Deployment")]
    pub deployment: Option<Deployment>,

    /// local prover API (Bento)
    ///
    /// Setting this value toggles using Bento for proving and disables Bonsai
    #[clap(long, env, default_value = "http://localhost:8081", conflicts_with_all = ["bonsai_api_url", "bonsai_api_key"])]
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

    /// Log JSON
    #[clap(long, env, default_value_t = false)]
    pub log_json: bool,
}

/// Status of a persistent order as it moves through the lifecycle in the database.
/// Orders in initial, intermediate, or terminal non-failure states (e.g. New, Pricing, Done, Skipped)
/// are managed in-memory or removed from the database.
#[derive(Clone, Copy, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
enum OrderStatus {
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

#[derive(Clone, Copy, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
enum FulfillmentType {
    LockAndFulfill,
    FulfillAfterLockExpire,
    // Currently not supported
    FulfillWithoutLocking,
}

/// Message sent from MarketMonitor to OrderPicker about order state changes
#[derive(Debug, Clone)]
pub enum OrderStateChange {
    /// Order has been locked by a prover
    Locked { request_id: U256, prover: Address },
    /// Order has been fulfilled
    Fulfilled { request_id: U256 },
}

/// Helper function to format an order ID consistently
fn format_order_id(
    request_id: &U256,
    signing_hash: &FixedBytes<32>,
    fulfillment_type: &FulfillmentType,
) -> String {
    format!("0x{request_id:x}-{signing_hash}-{fulfillment_type:?}")
}

/// Order request from the network.
///
/// This will turn into an [`Order`] once it is locked or skipped.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct OrderRequest {
    request: ProofRequest,
    client_sig: Bytes,
    fulfillment_type: FulfillmentType,
    boundless_market_address: Address,
    chain_id: u64,
    image_id: Option<String>,
    input_id: Option<String>,
    total_cycles: Option<u64>,
    target_timestamp: Option<u64>,
    expire_timestamp: Option<u64>,
    #[serde(skip)]
    cached_id: OnceLock<String>,
}

impl OrderRequest {
    pub fn new(
        request: ProofRequest,
        client_sig: Bytes,
        fulfillment_type: FulfillmentType,
        boundless_market_address: Address,
        chain_id: u64,
    ) -> Self {
        Self {
            request,
            client_sig,
            fulfillment_type,
            boundless_market_address,
            chain_id,
            image_id: None,
            input_id: None,
            total_cycles: None,
            target_timestamp: None,
            expire_timestamp: None,
            cached_id: OnceLock::new(),
        }
    }

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

    fn to_order(&self, status: OrderStatus) -> Order {
        Order {
            boundless_market_address: self.boundless_market_address,
            chain_id: self.chain_id,
            fulfillment_type: self.fulfillment_type,
            request: self.request.clone(),
            status,
            client_sig: self.client_sig.clone(),
            updated_at: Utc::now(),
            image_id: self.image_id.clone(),
            input_id: self.input_id.clone(),
            total_cycles: self.total_cycles,
            target_timestamp: self.target_timestamp,
            expire_timestamp: self.expire_timestamp,
            proving_started_at: None,
            proof_id: None,
            compressed_proof_id: None,
            lock_price: None,
            error_msg: None,
            cached_id: OnceLock::new(),
        }
    }

    fn to_skipped_order(&self) -> Order {
        self.to_order(OrderStatus::Skipped)
    }

    fn to_proving_order(&self, lock_price: U256) -> Order {
        let mut order = self.to_order(OrderStatus::PendingProving);
        order.lock_price = Some(lock_price);
        order.proving_started_at = Some(Utc::now().timestamp().try_into().unwrap());
        order
    }

    /// Returns the relevant expiration timestamp for this order based on its fulfillment type.
    /// - For LockAndFulfill orders: returns lock expiration
    /// - For FulfillAfterLockExpire/FulfillWithoutLocking orders: returns order expiration
    pub fn expiry(&self) -> u64 {
        match self.fulfillment_type {
            FulfillmentType::LockAndFulfill => self.request.lock_expires_at(),
            FulfillmentType::FulfillAfterLockExpire => self.request.expires_at(),
            FulfillmentType::FulfillWithoutLocking => self.request.expires_at(),
        }
    }
}

impl std::fmt::Display for OrderRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let total_mcycles = if self.total_cycles.is_some() {
            format!(" ({} mcycles)", self.total_cycles.unwrap() / 1_000_000)
        } else {
            "".to_string()
        };
        write!(f, "{}{} [{}]", self.id(), total_mcycles, format_expiries(&self.request))
    }
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
struct Order {
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
enum CompressionType {
    None,
    Groth16,
    Blake3Groth16,
}

#[derive(sqlx::Type, Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
enum BatchStatus {
    #[default]
    Aggregating,
    PendingCompression,
    Complete,
    PendingSubmission,
    Submitted,
    Failed,
}

#[derive(Serialize, Deserialize, Clone)]
struct AggregationState {
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
struct Batch {
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

pub struct Broker<P> {
    args: Args,
    provider: Arc<P>,
    db: DbObj,
    config_watcher: ConfigWatcher,
    priority_requestors: requestor_monitor::PriorityRequestors,
    allow_requestors: requestor_monitor::AllowRequestors,
    gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
}

impl<P> Broker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    pub async fn new(
        mut args: Args,
        provider: P,
        config_watcher: ConfigWatcher,
        gas_priority_mode: Arc<tokio::sync::RwLock<PriorityMode>>,
    ) -> Result<Self> {
        let db: DbObj =
            Arc::new(SqliteDb::new(&args.db_url).await.context("Failed to connect to sqlite DB")?);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        // Resolve deployment configuration if not provided, or validate if provided
        if let Some(manual_deployment) = &args.deployment {
            // Check if there's a default deployment for this chain ID
            if let Some(expected_deployment) = Deployment::from_chain_id(chain_id) {
                Self::validate_deployment_config(manual_deployment, &expected_deployment, chain_id);
            } else {
                tracing::info!("Using manually configured deployment for chain ID {chain_id} (no default available)");
            }
        } else {
            args.deployment = Some(Deployment::from_chain_id(chain_id)
                .with_context(|| format!("No default deployment found for chain ID {chain_id}. Please specify deployment configuration manually."))?);
            tracing::info!("Using default deployment configuration for chain ID {chain_id}");
        }

        let priority_requestors =
            requestor_monitor::PriorityRequestors::new(config_watcher.config.clone(), chain_id);
        let allow_requestors =
            requestor_monitor::AllowRequestors::new(config_watcher.config.clone(), chain_id);

        Ok(Self {
            args,
            db,
            provider: Arc::new(provider),
            config_watcher,
            priority_requestors,
            allow_requestors,
            gas_priority_mode,
        })
    }

    pub fn deployment(&self) -> &Deployment {
        self.args.deployment.as_ref().unwrap()
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
            tracing::warn!("This may indicate a configuration error. Please verify your deployment addresses are correct.");
        }
    }

    async fn fetch_and_upload_set_builder_image(&self, prover: &ProverObj) -> Result<Digest> {
        let set_verifier_contract = SetVerifierService::new(
            self.deployment().set_verifier_address,
            self.provider.clone(),
            Address::ZERO,
        );

        let (image_id, image_url_str) = set_verifier_contract
            .image_info()
            .await
            .context("Failed to get set builder image_info")?;
        let image_id = Digest::from_bytes(image_id.0);
        let (path, default_url) = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
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

    async fn fetch_and_upload_assessor_image(&self, prover: &ProverObj) -> Result<Digest> {
        let boundless_market = BoundlessMarketService::new_for_broker(
            self.deployment().boundless_market_address,
            self.provider.clone(),
            Address::ZERO,
        );
        let (image_id, image_url_str) =
            boundless_market.image_info().await.context("Failed to get assessor image_info")?;
        let image_id = Digest::from_bytes(image_id.0);

        let (path, default_url) = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
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
                        let program = match self.download_image(&contract_url, "contract").await {
                            Ok(bytes) => bytes,
                            Err(_) => {
                                let overridden_url = override_gateway(&contract_url);
                                tracing::debug!("Retrying with overridden URL: {overridden_url}");
                                self.download_image(&overridden_url, "gateway fallback").await?
                            }
                        };
                        program
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to download {} image from default URL: {}, falling back to contract URL",
                        image_label,
                        e
                    );
                    let program = match self.download_image(&contract_url, "contract").await {
                        Ok(bytes) => bytes,
                        Err(_) => {
                            let overridden_url = override_gateway(&contract_url);
                            tracing::debug!("Retrying with overridden URL: {overridden_url}");
                            self.download_image(&overridden_url, "gateway fallback").await?
                        }
                    };
                    program
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

        let handler = create_uri_handler(url, &self.config_watcher.config, false)
            .await
            .with_context(|| format!("Failed to create handler for {} URL", source_name))?;

        let bytes = handler
            .fetch()
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

    pub async fn start_service(&self) -> Result<()> {
        let mut non_critical_tasks: JoinSet<Result<()>> = JoinSet::new();
        let mut critical_tasks: JoinSet<Result<()>> = JoinSet::new();

        let config = self.config_watcher.config.clone();

        let (lookback_blocks, events_poll_blocks, events_poll_ms) = {
            let config = match config.lock_all() {
                Ok(res) => res,
                Err(err) => anyhow::bail!("Failed to lock config in watcher: {err:?}"),
            };
            (
                config.market.lookback_blocks,
                config.market.events_poll_blocks,
                config.market.events_poll_ms,
            )
        };

        // Create two cancellation tokens for graceful shutdown:
        // 1. Non-critical tasks (order discovery, picking, monitoring) - cancelled immediately on shutdown signal
        // 2. Critical tasks (proving, aggregation, submission) - cancelled only after committed orders complete
        let non_critical_cancel_token = CancellationToken::new();
        let critical_cancel_token = CancellationToken::new();

        let chain_monitor = Arc::new(
            chain_monitor::ChainMonitorService::new(self.provider.clone())
                .await
                .context("Failed to initialize chain monitor")?,
        );

        let cloned_chain_monitor = chain_monitor.clone();
        let cloned_config = config.clone();
        // Critical task, as is relied on to query current chain state
        let cancel_token = critical_cancel_token.clone();
        critical_tasks.spawn(async move {
            Supervisor::new(cloned_chain_monitor, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start chain monitor")?;
            Ok(())
        });

        let chain_id = self.provider.get_chain_id().await.context("Failed to get chain ID")?;
        let client = self
            .deployment()
            .order_stream_url
            .clone()
            .map(|url| -> Result<OrderStreamClient> {
                let url = Url::parse(&url).context("Failed to parse order stream URL")?;
                Ok(OrderStreamClient::new(
                    url,
                    self.deployment().boundless_market_address,
                    chain_id,
                ))
            })
            .transpose()?;

        // Create a channel for new orders to be sent to the OrderPicker / from monitors
        let (new_order_tx, new_order_rx) = mpsc::channel(NEW_ORDER_CHANNEL_CAPACITY);

        // Create a broadcast channel for order state change messages
        let (order_state_tx, _) = tokio::sync::broadcast::channel(ORDER_STATE_CHANNEL_CAPACITY);

        // spin up a supervisor for the market monitor
        let market_monitor = Arc::new(market_monitor::MarketMonitor::new(
            lookback_blocks,
            events_poll_blocks,
            events_poll_ms,
            self.deployment().boundless_market_address,
            self.provider.clone(),
            self.db.clone(),
            chain_monitor.clone(),
            self.args.private_key.as_ref().expect("Private key must be set").address(),
            client.clone(),
            new_order_tx.clone(),
            order_state_tx.clone(),
        ));

        let block_times =
            market_monitor.get_block_time().await.context("Failed to sample block times")?;

        tracing::debug!("Estimated block time: {block_times}");

        let cloned_config = config.clone();
        let cancel_token = non_critical_cancel_token.clone();
        non_critical_tasks.spawn(async move {
            Supervisor::new(market_monitor, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start market monitor")?;
            Ok(())
        });

        // spin up a supervisor for the offchain market monitor
        if let Some(client_clone) = client {
            let offchain_market_monitor =
                Arc::new(offchain_market_monitor::OffchainMarketMonitor::new(
                    client_clone,
                    self.args.private_key.clone().expect("Private key must be set"),
                    new_order_tx.clone(),
                ));
            let cloned_config = config.clone();
            let cancel_token = non_critical_cancel_token.clone();
            non_critical_tasks.spawn(async move {
                Supervisor::new(offchain_market_monitor, cloned_config, cancel_token)
                    .spawn()
                    .await
                    .context("Failed to start offchain market monitor")?;
                Ok(())
            });
        }

        // Construct the prover object interface
        let prover: provers::ProverObj;
        let aggregation_prover: provers::ProverObj;
        if is_dev_mode() {
            tracing::warn!("WARNING: Running the Broker in dev mode does not generate valid receipts. \
            Receipts generated from this process are invalid and should never be used in production.");
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
        };

        let prover_addr = self.provider.default_signer_address();

        let (pricing_tx, pricing_rx) = mpsc::channel(PRICING_CHANNEL_CAPACITY);

        let market = Arc::new(BoundlessMarketService::new_for_broker(
            self.deployment().boundless_market_address,
            DynProvider::new(self.provider.clone()),
            prover_addr,
        ));
        let collateral_token_decimals = market
            .collateral_token_decimals()
            .await
            .context("Failed to get stake token decimals. Possible RPC error.")?;

        // Spin up the order picker to pre-flight and find orders to lock
        let order_picker = Arc::new(order_picker::OrderPicker::new(
            self.db.clone(),
            config.clone(),
            prover.clone(),
            self.deployment().boundless_market_address,
            self.provider.clone(),
            chain_monitor.clone(),
            new_order_rx,
            pricing_tx,
            collateral_token_decimals,
            order_state_tx.clone(),
            self.priority_requestors.clone(),
            self.allow_requestors.clone(),
        ));
        let cloned_config = config.clone();
        let cancel_token = non_critical_cancel_token.clone();
        non_critical_tasks.spawn(async move {
            Supervisor::new(order_picker, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start order picker")?;
            Ok(())
        });

        let proving_service = Arc::new(
            proving::ProvingService::new(
                self.db.clone(),
                prover.clone(),
                aggregation_prover.clone(),
                config.clone(),
                order_state_tx.clone(),
                self.priority_requestors.clone(),
                market.clone(),
            )
            .await
            .context("Failed to initialize proving service")?,
        );

        let cloned_config = config.clone();
        let cancel_token = critical_cancel_token.clone();
        critical_tasks.spawn(async move {
            Supervisor::new(proving_service, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start proving service")?;
            Ok(())
        });

        let order_monitor = Arc::new(order_monitor::OrderMonitor::new(
            self.db.clone(),
            self.provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_times,
            prover_addr,
            self.deployment().boundless_market_address,
            pricing_rx,
            collateral_token_decimals,
            order_monitor::RpcRetryConfig {
                retry_count: self.args.rpc_retry_max.into(),
                retry_sleep_ms: self.args.rpc_retry_backoff,
            },
            self.gas_priority_mode.clone(),
        )?);
        let cloned_config = config.clone();
        let cancel_token = non_critical_cancel_token.clone();
        non_critical_tasks.spawn(async move {
            Supervisor::new(order_monitor, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start order monitor")?;
            Ok(())
        });

        let set_builder_img_id = self.fetch_and_upload_set_builder_image(&prover).await?;
        let assessor_img_id = self.fetch_and_upload_assessor_image(&prover).await?;

        let aggregator = Arc::new(
            aggregator::AggregatorService::new(
                self.db.clone(),
                chain_id,
                set_builder_img_id,
                assessor_img_id,
                self.deployment().boundless_market_address,
                prover_addr,
                config.clone(),
                aggregation_prover.clone(),
            )
            .await
            .context("Failed to initialize aggregator service")?,
        );

        let cloned_config = config.clone();
        let cancel_token = critical_cancel_token.clone();
        critical_tasks.spawn(async move {
            Supervisor::new(aggregator, cloned_config, cancel_token)
                .with_retry_policy(RetryPolicy::CRITICAL_SERVICE)
                .spawn()
                .await
                .context("Failed to start aggregator service")?;
            Ok(())
        });

        // Start the ReaperTask to check for expired committed orders
        let reaper =
            Arc::new(reaper::ReaperTask::new(self.db.clone(), config.clone(), prover.clone()));
        let cloned_config = config.clone();
        // Using critical cancel token to ensure no stuck expired jobs on shutdown
        let cancel_token = critical_cancel_token.clone();
        critical_tasks.spawn(async move {
            Supervisor::new(reaper, cloned_config, cancel_token)
                .spawn()
                .await
                .context("Failed to start reaper service")?;
            Ok(())
        });

        // Start the RequestorMonitor to periodically fetch priority and allow lists
        let requestor_monitor = Arc::new(requestor_monitor::RequestorMonitor::new(
            self.priority_requestors.clone(),
            self.allow_requestors.clone(),
        ));
        let config_clone = config.clone();
        let non_critical_cancel_token_clone = non_critical_cancel_token.clone();
        non_critical_tasks.spawn(async move {
            Supervisor::new(requestor_monitor, config_clone, non_critical_cancel_token_clone)
                .spawn()
                .await
                .context("Requestor list monitor panicked")?;
            Ok(())
        });

        let submitter = Arc::new(submitter::Submitter::new(
            self.db.clone(),
            config.clone(),
            aggregation_prover.clone(),
            self.provider.clone(),
            self.deployment().set_verifier_address,
            self.deployment().boundless_market_address,
            set_builder_img_id,
        )?);
        let cloned_config = config.clone();
        let cancel_token = critical_cancel_token.clone();
        critical_tasks.spawn(async move {
            Supervisor::new(submitter, cloned_config, cancel_token)
                .with_retry_policy(RetryPolicy::CRITICAL_SERVICE)
                .spawn()
                .await
                .context("Failed to start submitter service")?;
            Ok(())
        });

        // Monitor the different supervisor tasks and handle shutdown
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
        self.shutdown_and_cancel_critical_tasks(critical_cancel_token).await?;

        Ok(())
    }

    async fn shutdown_and_cancel_critical_tasks(
        &self,
        critical_cancel_token: CancellationToken,
    ) -> Result<(), anyhow::Error> {
        // 2 hour max to shutdown time, to avoid indefinite shutdown time.
        const SHUTDOWN_GRACE_PERIOD_SECS: u32 = 2 * 60 * 60;
        const SLEEP_DURATION: std::time::Duration = std::time::Duration::from_secs(10);

        let start_time = std::time::Instant::now();
        let grace_period = std::time::Duration::from_secs(SHUTDOWN_GRACE_PERIOD_SECS as u64);
        let mut last_log = "".to_string();
        while start_time.elapsed() < grace_period {
            let in_progress_orders = self.db.get_committed_orders().await?;
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
            let in_progress_orders = self.db.get_committed_orders().await?;
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

/// Returns `true` if the `ALLOW_LOCAL_FILE_STORAGE` environment variable is enabled.
pub(crate) fn allow_local_file_storage() -> bool {
    std::env::var("ALLOW_LOCAL_FILE_STORAGE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(feature = "test-utils")]
pub mod test_utils {
    use std::sync::Arc;

    use alloy::network::Ethereum;
    use alloy::providers::{Provider, WalletProvider};
    use anyhow::Result;
    use boundless_market::dynamic_gas_filler::PriorityMode;
    use boundless_test_utils::{
        guests::{ASSESSOR_GUEST_PATH, SET_BUILDER_PATH},
        market::TestCtx,
    };
    use tempfile::NamedTempFile;
    use url::Url;

    use crate::config::ConfigWatcher;
    use crate::{config::Config, Args, Broker};

    pub struct BrokerBuilder<P> {
        args: Args,
        provider: P,
        config_file: NamedTempFile,
    }

    impl<P> BrokerBuilder<P>
    where
        P: Provider<Ethereum> + 'static + Clone + WalletProvider,
    {
        pub async fn new_test(ctx: &TestCtx<P>, rpc_url: Url) -> Self {
            let config_file: NamedTempFile = NamedTempFile::new().unwrap();
            let mut config = Config::default();
            config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
            config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
            config.market.min_mcycle_price = "0.00001".into();
            config.batcher.min_batch_size = 1;
            config.market.min_deadline = 30;
            config.write(config_file.path()).await.unwrap();

            let args = Args {
                db_url: "sqlite::memory:".into(),
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
                log_json: false,
            };
            Self { args, provider: ctx.prover_provider.clone(), config_file }
        }

        pub fn with_db_url(mut self, db_url: String) -> Self {
            self.args.db_url = db_url;
            self
        }

        pub async fn build(self) -> Result<(Broker<P>, NamedTempFile)> {
            let gas_priority_mode = Arc::new(tokio::sync::RwLock::new(PriorityMode::Medium));
            Ok((
                Broker::new(
                    self.args,
                    self.provider,
                    ConfigWatcher::new(self.config_file.path()).await?,
                    gas_priority_mode,
                )
                .await?,
                self.config_file,
            ))
        }
    }
}

#[cfg(test)]
pub mod tests;
