// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, sync::Arc, time::SystemTime};

use crate::config::ConfigLock;
use crate::storage::create_uri_handler;
use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, U256},
    providers::{Provider, WalletProvider},
    signers::local::PrivateKeySigner,
};
use anyhow::{ensure, Context, Result};
use boundless_market::{
    contracts::{boundless_market::BoundlessMarketService, InputType, ProofRequest},
    input::GuestEnv,
    order_stream_client::Client as OrderStreamClient,
    selector::is_groth16_selector,
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
use tokio::task::JoinSet;
use url::Url;

pub(crate) mod aggregator;
pub(crate) mod chain_monitor;
pub(crate) mod config;
pub(crate) mod db;
pub mod futures_retry;
pub(crate) mod market_monitor;
pub(crate) mod offchain_market_monitor;
pub(crate) mod order_monitor;
pub(crate) mod order_picker;
pub(crate) mod provers;
pub(crate) mod proving;
pub(crate) mod rpc_retry_policy;
pub(crate) mod storage;
pub(crate) mod submitter;
pub(crate) mod task;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// sqlite database connection url
    #[clap(short = 's', long, env, default_value = "sqlite::memory:")]
    pub db_url: String,

    /// RPC URL
    #[clap(long, env, default_value = "http://localhost:8545")]
    pub rpc_url: Url,

    /// Order stream server URL
    #[clap(long, env)]
    pub order_stream_url: Option<Url>,

    /// wallet key
    #[clap(long, env)]
    pub private_key: PrivateKeySigner,

    /// Boundless market address
    #[clap(long, env)]
    pub boundless_market_address: Address,

    /// Risc zero Set verifier address
    // TODO: Get this from the market contract via view call
    #[clap(long, env)]
    set_verifier_address: Address,

    /// local prover API (Bento)
    ///
    /// Setting this value toggles using Bento for proving and disables Bonsai
    #[clap(long, env, default_value = "http://localhost:8081", conflicts_with_all = ["bonsai_api_url", "bonsai_api_key"])]
    bento_api_url: Option<Url>,

    /// Bonsai API URL
    ///
    /// Toggling this disables Bento proving and uses Bonsai as a backend
    #[clap(long, env, conflicts_with = "bento_api_url")]
    bonsai_api_url: Option<Url>,

    /// Bonsai API Key
    ///
    /// Required if using BONSAI_API_URL
    #[clap(long, env, conflicts_with = "bento_api_url")]
    bonsai_api_key: Option<String>,

    /// Config file path
    #[clap(short, long, default_value = "broker.toml")]
    pub config_file: PathBuf,

    /// Pre deposit amount
    ///
    /// Amount of HP tokens to pre-deposit into the contract for staking eg: 100
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
}

/// Status of a order as it moves through the lifecycle
#[derive(Clone, Copy, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
enum OrderStatus {
    /// New order found on chain, waiting pricing analysis
    New,
    /// Order is in the process of being priced
    Pricing,
    /// Order is ready to lock at target_timestamp and then be fulfilled
    WaitingToLock,
    /// Order is ready to be fulfilled when its lock expires at target_timestamp
    WaitingForLockToExpire,
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
#[derive(Serialize, Deserialize, Debug, Clone)]
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
}

impl Order {
    pub fn new(
        request: ProofRequest,
        client_sig: Bytes,
        fulfillment_type: FulfillmentType,
        boundless_market_address: Address,
        chain_id: u64,
    ) -> Self {
        Self {
            boundless_market_address,
            chain_id,
            request,
            status: OrderStatus::New,
            updated_at: Utc::now(),
            target_timestamp: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig,
            lock_price: None,
            fulfillment_type,
            error_msg: None,
            total_cycles: None,
            proving_started_at: None,
        }
    }

    // An Order is identified by the request_id, the fulfillment type, and the hash of the proof request.
    // This structure supports multiple different ProofRequests with the same request_id, and different
    // fulfillment types.
    pub fn id(&self) -> String {
        format!(
            "0x{:x}-{}-{:?}",
            self.request.id,
            self.request.signing_hash(self.boundless_market_address, self.chain_id).unwrap(),
            self.fulfillment_type
        )
    }

    pub fn is_groth16(&self) -> bool {
        is_groth16_selector(self.request.requirements.selector)
    }
}

impl std::fmt::Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id())
    }
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
}

impl<P> Broker<P>
where
    P: Provider<Ethereum> + 'static + Clone + WalletProvider,
{
    pub async fn new(args: Args, provider: P) -> Result<Self> {
        let config_watcher =
            ConfigWatcher::new(&args.config_file).await.context("Failed to load broker config")?;

        let db: DbObj =
            Arc::new(SqliteDb::new(&args.db_url).await.context("Failed to connect to sqlite DB")?);

        Ok(Self { args, db, provider: Arc::new(provider), config_watcher })
    }

    async fn get_assessor_image(&self) -> Result<(Digest, Vec<u8>)> {
        let assessor_path = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
            config.prover.assessor_set_guest_path.clone()
        };

        if let Some(path) = assessor_path {
            let program_buf = std::fs::read(path).context("Failed to read assessor path")?;
            let img_id = risc0_zkvm::compute_image_id(&program_buf)
                .context("Failed to compute assessor imageId")?;

            Ok((img_id, program_buf))
        } else {
            let boundless_market = BoundlessMarketService::new(
                self.args.boundless_market_address,
                self.provider.clone(),
                Address::ZERO,
            );

            let (image_id, image_url_str) =
                boundless_market.image_info().await.context("Failed to get contract image_info")?;
            let image_uri = create_uri_handler(&image_url_str, &self.config_watcher.config)
                .await
                .context("Failed to parse image URI")?;
            tracing::debug!("Downloading assessor image from: {image_uri}");
            let image_data =
                image_uri.fetch().await.context("Failed to download assessor image")?;

            Ok((Digest::from_bytes(image_id.0), image_data))
        }
    }

    async fn get_set_builder_image(&self) -> Result<(Digest, Vec<u8>)> {
        let set_builder_path = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
            config.prover.set_builder_guest_path.clone()
        };

        if let Some(path) = set_builder_path {
            let program_buf = std::fs::read(path).context("Failed to read set-builder path")?;
            let img_id = risc0_zkvm::compute_image_id(&program_buf)
                .context("Failed to compute set-builder imageId")?;

            Ok((img_id, program_buf))
        } else {
            let set_verifier_contract = SetVerifierService::new(
                self.args.set_verifier_address,
                self.provider.clone(),
                Address::ZERO,
            );

            let (image_id, image_url_str) = set_verifier_contract
                .image_info()
                .await
                .context("Failed to get contract image_info")?;
            let image_uri = create_uri_handler(&image_url_str, &self.config_watcher.config)
                .await
                .context("Failed to parse image URI")?;
            tracing::debug!("Downloading aggregation-set image from: {image_uri}");
            let image_data =
                image_uri.fetch().await.context("Failed to download aggregation-set image")?;

            Ok((Digest::from_bytes(image_id.0), image_data))
        }
    }

    pub async fn start_service(&self) -> Result<()> {
        let mut supervisor_tasks: JoinSet<Result<()>> = JoinSet::new();

        let config = self.config_watcher.config.clone();

        let loopback_blocks = {
            let config = match config.lock_all() {
                Ok(res) => res,
                Err(err) => anyhow::bail!("Failed to lock config in watcher: {err:?}"),
            };
            config.market.lookback_blocks
        };

        let chain_monitor = Arc::new(
            chain_monitor::ChainMonitorService::new(self.provider.clone())
                .await
                .context("Failed to initialize chain monitor")?,
        );

        let cloned_chain_monitor = chain_monitor.clone();
        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(cloned_chain_monitor, cloned_config)
                .spawn()
                .await
                .context("Failed to start chain monitor")?;
            Ok(())
        });

        let chain_id = self.provider.get_chain_id().await.context("Failed to get chain ID")?;
        let client =
            self.args.order_stream_url.clone().map(|url| {
                OrderStreamClient::new(url, self.args.boundless_market_address, chain_id)
            });

        // spin up a supervisor for the market monitor
        let market_monitor = Arc::new(market_monitor::MarketMonitor::new(
            loopback_blocks,
            self.args.boundless_market_address,
            self.provider.clone(),
            self.db.clone(),
            chain_monitor.clone(),
            self.args.private_key.address(),
            client.clone(),
        ));

        let block_times =
            market_monitor.get_block_time().await.context("Failed to sample block times")?;

        tracing::debug!("Estimated block time: {block_times}");

        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(market_monitor, cloned_config)
                .spawn()
                .await
                .context("Failed to start market monitor")?;
            Ok(())
        });

        // spin up a supervisor for the offchain market monitor
        if let Some(client) = client {
            let offchain_market_monitor =
                Arc::new(offchain_market_monitor::OffchainMarketMonitor::new(
                    self.db.clone(),
                    client.clone(),
                    self.args.private_key.clone(),
                ));
            let cloned_config = config.clone();
            supervisor_tasks.spawn(async move {
                Supervisor::new(offchain_market_monitor, cloned_config)
                    .spawn()
                    .await
                    .context("Failed to start offchain market monitor")?;
                Ok(())
            });
        }

        // Construct the prover object interface
        let prover: provers::ProverObj = if risc0_zkvm::is_dev_mode() {
            tracing::warn!("WARNING: Running the Broker in dev mode does not generate valid receipts. \
            Receipts generated from this process are invalid and should never be used in production.");
            Arc::new(provers::DefaultProver::new())
        } else if let (Some(bonsai_api_key), Some(bonsai_api_url)) =
            (self.args.bonsai_api_key.as_ref(), self.args.bonsai_api_url.as_ref())
        {
            tracing::info!("Configured to run with Bonsai backend");
            Arc::new(
                provers::Bonsai::new(config.clone(), bonsai_api_url.as_ref(), bonsai_api_key)
                    .context("Failed to construct Bonsai client")?,
            )
        } else if let Some(bento_api_url) = self.args.bento_api_url.as_ref() {
            tracing::info!("Configured to run with Bento backend");

            Arc::new(
                provers::Bonsai::new(config.clone(), bento_api_url.as_ref(), "")
                    .context("Failed to initialize Bento client")?,
            )
        } else {
            Arc::new(provers::DefaultProver::new())
        };

        // Spin up the order picker to pre-flight and find orders to lock
        let order_picker = Arc::new(order_picker::OrderPicker::new(
            self.db.clone(),
            config.clone(),
            prover.clone(),
            self.args.boundless_market_address,
            self.provider.clone(),
            chain_monitor.clone(),
        ));
        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(order_picker, cloned_config)
                .spawn()
                .await
                .context("Failed to start order picker")?;
            Ok(())
        });

        let order_monitor = Arc::new(order_monitor::OrderMonitor::new(
            self.db.clone(),
            self.provider.clone(),
            chain_monitor.clone(),
            config.clone(),
            block_times,
            self.args.boundless_market_address,
        )?);
        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(order_monitor, cloned_config)
                .spawn()
                .await
                .context("Failed to start order monitor")?;
            Ok(())
        });

        let proving_service = Arc::new(
            proving::ProvingService::new(self.db.clone(), prover.clone(), config.clone())
                .await
                .context("Failed to initialize proving service")?,
        );

        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(proving_service, cloned_config)
                .spawn()
                .await
                .context("Failed to start proving service")?;
            Ok(())
        });

        let set_builder_img_data = self.get_set_builder_image().await?;
        let assessor_img_data = self.get_assessor_image().await?;

        let prover_addr = self.args.private_key.address();
        let aggregator = Arc::new(
            aggregator::AggregatorService::new(
                self.db.clone(),
                chain_id,
                set_builder_img_data.0,
                set_builder_img_data.1,
                assessor_img_data.0,
                assessor_img_data.1,
                self.args.boundless_market_address,
                prover_addr,
                config.clone(),
                prover.clone(),
            )
            .await
            .context("Failed to initialize aggregator service")?,
        );

        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(aggregator, cloned_config)
                .with_retry_policy(RetryPolicy::CRITICAL_SERVICE)
                .spawn()
                .await
                .context("Failed to start aggregator service")?;
            Ok(())
        });

        let submitter = Arc::new(submitter::Submitter::new(
            self.db.clone(),
            config.clone(),
            prover.clone(),
            self.provider.clone(),
            self.args.set_verifier_address,
            self.args.boundless_market_address,
            set_builder_img_data.0,
        )?);
        let cloned_config = config.clone();
        supervisor_tasks.spawn(async move {
            Supervisor::new(submitter, cloned_config)
                .with_retry_policy(RetryPolicy::CRITICAL_SERVICE)
                .spawn()
                .await
                .context("Failed to start submitter service")?;
            Ok(())
        });

        // Monitor the different supervisor tasks
        while let Some(res) = supervisor_tasks.join_next().await {
            let status = match res {
                Err(join_err) if join_err.is_cancelled() => {
                    tracing::info!("Tokio task exited with cancellation status: {join_err:?}");
                    continue;
                }
                Err(join_err) => {
                    tracing::error!("Tokio task exited with error status: {join_err:?}");
                    // TODO(#BM-470): Here, we should be using a cancellation token to signal to all
                    // the tasks under this supervisor that they should exit, then set a timer (e.g.
                    // for 30) to give them time to gracefully shut down.
                    anyhow::bail!("Task exited with error status: {join_err:?}")
                }
                Ok(status) => status,
            };
            match status {
                Err(err) => {
                    tracing::error!("Task exited with error status: {err:?}");
                    // TODO(#BM-470): Here, we should be using a cancellation token to signal to all
                    // the tasks under this supervisor that they should exit, then set a timer (e.g.
                    // for 30) to give them time to gracefully shut down.
                    anyhow::bail!("Task exited with error status: {err:?}")
                }
                Ok(()) => {
                    tracing::info!("Task exited with ok status");
                }
            }
        }

        Ok(())
    }
}

async fn upload_image_uri(
    prover: &ProverObj,
    order: &Order,
    config: &ConfigLock,
) -> Result<String> {
    let uri =
        create_uri_handler(&order.request.imageUrl, config).await.context("URL handling failed")?;

    let image_data = uri
        .fetch()
        .await
        .with_context(|| format!("Failed to fetch image URI: {}", order.request.imageUrl))?;
    let image_id =
        risc0_zkvm::compute_image_id(&image_data).context("Failed to compute image ID")?;

    let required_image_id = Digest::from(order.request.requirements.imageId.0);
    ensure!(
        image_id == required_image_id,
        "image ID does not match requirements; expect {}, got {}",
        required_image_id,
        image_id
    );
    let image_id = image_id.to_string();

    prover.upload_image(&image_id, image_data).await.context("Failed to upload image to prover")?;

    Ok(image_id)
}

async fn upload_input_uri(
    prover: &ProverObj,
    order: &Order,
    config: &ConfigLock,
) -> Result<String> {
    Ok(match order.request.input.inputType {
        InputType::Inline => prover
            .upload_input(
                GuestEnv::decode(&order.request.input.data)
                    .with_context(|| "Failed to decode input")?
                    .stdin,
            )
            .await
            .context("Failed to upload input data")?,

        InputType::Url => {
            let input_uri_str =
                std::str::from_utf8(&order.request.input.data).context("input url is not utf8")?;
            tracing::debug!("Input URI string: {input_uri_str}");
            let input_uri =
                create_uri_handler(input_uri_str, config).await.context("URL handling failed")?;

            let input_data = GuestEnv::decode(
                &input_uri
                    .fetch()
                    .await
                    .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?,
            )
            .with_context(|| format!("Failed to decode input from URI: {input_uri_str}"))?
            .stdin;

            prover.upload_input(input_data).await.context("Failed to upload input")?
        }
        //???
        _ => anyhow::bail!("Invalid input type: {:?}", order.request.input.inputType),
    })
}

/// A very small utility function to get the current unix timestamp in seconds.
// TODO(#379): Avoid drift relative to the chain's timestamps.
pub(crate) fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

#[cfg(feature = "test-utils")]
pub mod test_utils {
    use alloy::network::Ethereum;
    use alloy::providers::{Provider, WalletProvider};
    use anyhow::Result;
    use boundless_market_test_utils::TestCtx;
    use guest_assessor::ASSESSOR_GUEST_PATH;
    use guest_set_builder::SET_BUILDER_PATH;
    use tempfile::NamedTempFile;
    use url::Url;

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
            config.market.mcycle_price = "0.00001".into();
            config.batcher.min_batch_size = Some(1);
            config.write(config_file.path()).await.unwrap();

            let args = Args {
                db_url: "sqlite::memory:".into(),
                config_file: config_file.path().to_path_buf(),
                boundless_market_address: ctx.boundless_market_address,
                set_verifier_address: ctx.set_verifier_address,
                rpc_url,
                order_stream_url: None,
                private_key: ctx.prover_signer.clone(),
                bento_api_url: None,
                bonsai_api_key: None,
                bonsai_api_url: None,
                deposit_amount: None,
                rpc_retry_max: 0,
                rpc_retry_backoff: 200,
                rpc_retry_cu: 1000,
            };
            Self { args, provider: ctx.prover_provider.clone(), config_file }
        }

        pub fn with_db_url(mut self, db_url: String) -> Self {
            self.args.db_url = db_url;
            self
        }

        pub async fn build(self) -> Result<(Broker<P>, NamedTempFile)> {
            Ok((Broker::new(self.args, self.provider).await?, self.config_file))
        }
    }
}

#[cfg(test)]
pub mod tests;
