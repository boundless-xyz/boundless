// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, sync::Arc};

use alloy::{
    network::Ethereum,
    primitives::{Address, Bytes, U256},
    providers::{Provider, WalletProvider},
    signers::local::PrivateKeySigner,
    transports::Transport,
};
use anyhow::{ensure, Context, Result};
use boundless_market::contracts::{
    proof_market::ProofMarketService, set_verifier::SetVerifierService, InputType, ProvingRequest,
};
use chrono::{serde::ts_seconds, DateTime, Utc};
use clap::Parser;
use config::ConfigWatcher;
use db::{DbObj, SqliteDb};
use provers::ProverObj;
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};
use storage::UriHandlerBuilder;
use tokio::task::JoinSet;
use url::Url;

pub(crate) mod aggregator;
pub(crate) mod config;
pub(crate) mod db;
pub(crate) mod market_monitor;
pub(crate) mod order_monitor;
pub(crate) mod order_picker;
pub(crate) mod provers;
pub(crate) mod proving;
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

    /// wallet key
    #[clap(long, env)]
    pub priv_key: PrivateKeySigner,

    /// Proof market address
    #[clap(long, env)]
    pub proof_market_addr: Address,

    /// Risc zero Set verifier address
    // TODO: Get this from the proof market via view call
    #[clap(long, env)]
    set_verifier_addr: Address,

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
    config_file: PathBuf,

    /// Pre deposit amount
    ///
    /// Amount of ETH to pre-deposit into the contract for staking eg: 0.1 ETH
    #[clap(short, long)]
    pub deposit_amount: Option<String>,
}

/// Status of a order as it moves through the lifecycle
#[derive(Clone, sqlx::Type, Debug, PartialEq, Serialize, Deserialize)]
enum OrderStatus {
    /// New order found on chain, waiting pricing analysis
    New,
    /// Order is in the process of being priced
    Pricing,
    /// Order is ready to lock at target_block
    Locking,
    /// Order has been locked in and ready to begin proving
    Locked,
    /// Order is actively ready for proving
    Proving,
    /// Order is ready for aggregation
    PendingAgg,
    /// Order is in the process of Aggregation
    Aggregating,
    /// Pending on chain finalization
    PendingSubmission,
    /// Order has been completed
    Done,
    /// Order failed
    Failed,
    /// Order was analyzed and marked as skipable
    Skipped,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct Order {
    /// Proving request object
    request: ProvingRequest,
    /// status of the order
    status: OrderStatus,
    /// Last update time
    #[serde(with = "ts_seconds")]
    updated_at: DateTime<Utc>,
    /// Locking status target block
    target_block: Option<u64>,
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
    /// Block the order expires at
    ///
    /// Populated during order picking
    expire_block: Option<u64>,
    /// Order merkle inclusion path
    ///
    /// Populated after batch including order completes
    path: Option<Vec<Digest>>,
    /// Client Signature
    client_sig: Bytes,
    /// Price the lockin was set at
    lock_price: Option<U256>,
    /// Failure message
    error_msg: Option<String>,
}

impl Order {
    pub fn new(request: ProvingRequest, client_sig: Bytes) -> Self {
        Self {
            request,
            status: OrderStatus::New,
            updated_at: Utc::now(),
            target_block: None,
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig,
            lock_price: None,
            error_msg: None,
        }
    }
}

/// A node in the Merkle tree.
#[derive(Serialize, PartialEq, Deserialize, Debug, Clone)]
enum Node {
    Singleton { proof_id: String, order_id: U256, root: Digest },
    Join { proof_id: String, height: usize, left: Box<Node>, right: Box<Node>, root: Digest },
}

impl Node {
    fn singleton(proof_id: String, order_id: U256, root: Digest) -> Self {
        Node::Singleton { proof_id, order_id, root }
    }
    fn join(proof_id: String, height: usize, left: Node, right: Node, root: Digest) -> Self {
        Node::Join { proof_id, height, left: left.into(), right: right.into(), root }
    }

    fn proof_id(&self) -> &str {
        match self {
            Node::Singleton { proof_id, .. } => proof_id,
            Node::Join { proof_id, .. } => proof_id,
        }
    }
    fn height(&self) -> usize {
        match self {
            Node::Singleton { .. } => 0,
            Node::Join { height, .. } => *height,
        }
    }
    fn root(&self) -> Digest {
        match self {
            Node::Singleton { root, .. } => *root,
            Node::Join { root, .. } => *root,
        }
    }

    fn order_ids(&self) -> Vec<U256> {
        fn rec_order_ids(node: &Node, ids: &mut Vec<U256>) {
            match node {
                Node::Singleton { order_id, .. } => ids.push(*order_id),
                Node::Join { left, right, .. } => {
                    rec_order_ids(left, ids);
                    rec_order_ids(right, ids);
                }
            }
        }

        let mut ids = vec![];
        rec_order_ids(self, &mut ids);
        ids
    }

    /// Recursively gets all paths for orderID's into `output`
    fn get_order_paths(
        &self,
        mut path: Vec<Digest>,
        mut output: &mut Vec<(U256, Vec<Digest>)>,
    ) -> Result<()> {
        match &self {
            Node::Singleton { order_id, .. } => {
                // TODO: This is kinda hacky, I would like a better way to flag this or
                // disconnect the DB from this tree model a bit better
                //
                // Skips "extra" proofs in the batch, like assessor leafs
                if *order_id != U256::ZERO {
                    path.reverse();
                    output.push((*order_id, path));
                    // db.set_order_path(*order_id, path).await?;
                }
            }
            Node::Join { left, right, .. } => {
                let mut left_path = path.clone();
                left_path.push(right.root());
                left.get_order_paths(left_path, &mut output)?;

                let mut right_path = path.clone();
                right_path.push(left.root());
                right.get_order_paths(right_path, &mut output)?;
            }
        };

        Ok(())
    }
}

#[derive(sqlx::Type, Default, Serialize, Deserialize, Debug, Clone, PartialEq)]
enum BatchStatus {
    #[default]
    Aggregating,
    Complete,
    PendingSubmission,
    Submitted,
    Failed,
}

#[derive(Serialize, Deserialize, Default, Clone)]
struct Batch {
    pub status: BatchStatus,
    pub orders: Vec<U256>,
    pub root: Option<Digest>,
    pub orders_root: Option<Digest>,
    pub groth16_proof_id: String,
    pub start_time: DateTime<Utc>,
    pub block_deadline: Option<u64>,
    pub fees: U256,
    pub error_msg: Option<String>,
    pub peaks: Vec<Node>,
}

pub struct Broker<T, P> {
    args: Args,
    provider: Arc<P>,
    db: DbObj,
    config_watcher: ConfigWatcher,
    _phantom_t: std::marker::PhantomData<T>,
}

impl<T, P> Broker<T, P>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone + WalletProvider,
{
    pub async fn new(args: Args, provider: P) -> Result<Self> {
        let config_watcher =
            ConfigWatcher::new(&args.config_file).await.context("Failed to load broker config")?;

        let db: DbObj =
            Arc::new(SqliteDb::new(&args.db_url).await.context("Failed to connect to sqlite DB")?);

        Ok(Self {
            args,
            db,
            provider: Arc::new(provider),
            config_watcher,
            _phantom_t: Default::default(),
        })
    }

    async fn get_assessor_image(&self) -> Result<(Digest, Vec<u8>)> {
        let (assessor_path, max_file_size) = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
            (config.prover.assessor_set_guest_path.clone(), config.market.max_file_size)
        };

        if let Some(path) = assessor_path {
            let elf_buf = std::fs::read(path).context("Failed to read assessor path")?;
            let img_id = risc0_zkvm::compute_image_id(&elf_buf)
                .context("Failed to compute assessor imageId")?;

            Ok((img_id, elf_buf))
        } else {
            let proof_market = ProofMarketService::new(
                self.args.proof_market_addr,
                self.provider.clone(),
                Address::ZERO,
            );

            let (image_id, image_url_str) =
                proof_market.image_info().await.context("Failed to get contract image_info")?;
            let image_uri = UriHandlerBuilder::new(&image_url_str)
                .set_max_size(max_file_size)
                .build()
                .context("Failed to parse image URI")?;
            tracing::debug!("Downloading assessor image from: {image_uri}");
            let image_data = image_uri.fetch().await.context("Failed to download sot image")?;

            Ok((Digest::from_bytes(image_id.0), image_data))
        }
    }

    async fn get_set_builder_image(&self) -> Result<(Digest, Vec<u8>)> {
        let (set_builder_path, max_file_size) = {
            let config = self.config_watcher.config.lock_all().context("Failed to lock config")?;
            (config.prover.set_builder_guest_path.clone(), config.market.max_file_size)
        };

        if let Some(path) = set_builder_path {
            let elf_buf = std::fs::read(path).context("Failed to read set-builder path")?;
            let img_id = risc0_zkvm::compute_image_id(&elf_buf)
                .context("Failed to compute set-builder imageId")?;

            Ok((img_id, elf_buf))
        } else {
            let set_verifier_contract = SetVerifierService::new(
                self.args.set_verifier_addr,
                self.provider.clone(),
                Address::ZERO,
            );

            let (image_id, image_url_str) = set_verifier_contract
                .image_info()
                .await
                .context("Failed to get contract image_info")?;
            let image_uri = UriHandlerBuilder::new(&image_url_str)
                .set_max_size(max_file_size)
                .build()
                .context("Failed to parse image URI")?;
            tracing::debug!("Downloading aggregation-set image from: {image_uri}");
            let image_data = image_uri.fetch().await.context("Failed to download sot image")?;

            Ok((Digest::from_bytes(image_id.0), image_data))
        }
    }

    pub async fn start_service(&self) -> Result<()> {
        let mut supervisor_tasks: JoinSet<Result<()>> = JoinSet::new();

        let loopback_blocks = {
            let config = match self.config_watcher.config.lock_all() {
                Ok(res) => res,
                Err(err) => anyhow::bail!("Failed to lock config in watcher: {err:?}"),
            };
            config.market.lookback_blocks
        };

        // spin up a supervisor for the market monitor
        let market_monitor = Arc::new(market_monitor::MarketMonitor::new(
            loopback_blocks,
            self.args.proof_market_addr,
            self.provider.clone(),
            self.db.clone(),
        ));

        let block_times =
            market_monitor.get_block_time().await.context("Failed to sample block times")?;

        tracing::debug!("Estimated block time: {block_times}");

        supervisor_tasks.spawn(async move {
            task::supervisor(1, market_monitor).await.context("Failed to start market monitor")?;
            Ok(())
        });

        // Construct the prover object interface
        let prover: provers::ProverObj = if risc0_zkvm::is_dev_mode() {
            tracing::warn!("WARNING: Running the Broker in dev mode does not generate valid receipts. \
            Receipts generated from this process are invalid and should never be used in production.");
            Arc::new(provers::MockProver::default())
        } else if let (Some(bonsai_api_key), Some(bonsai_api_url)) =
            (self.args.bonsai_api_key.as_ref(), self.args.bonsai_api_url.as_ref())
        {
            tracing::info!("Configured to run with Bonsai backend");
            Arc::new(
                provers::Bonsai::new(
                    self.config_watcher.config.clone(),
                    bonsai_api_url.as_ref(),
                    bonsai_api_key,
                )
                .context("Failed to construct Bonsai client")?,
            )
        } else if let Some(bento_api_url) = self.args.bento_api_url.as_ref() {
            tracing::info!("Configured to run with Bento backend");

            Arc::new(
                provers::Bonsai::new(
                    self.config_watcher.config.clone(),
                    &bento_api_url.to_string(),
                    "",
                )
                .context("Failed to initialize Bento client")?,
            )
        } else if cfg!(test) {
            Arc::new(provers::MockProver::default())
        } else {
            anyhow::bail!("Failed to select a proving backend");
        };

        // Spin up the order picker to pre-flight and find orders to lock
        let order_picker = Arc::new(order_picker::OrderPicker::new(
            self.db.clone(),
            self.config_watcher.config.clone(),
            prover.clone(),
            block_times,
            self.args.proof_market_addr,
            self.provider.clone(),
        ));
        supervisor_tasks.spawn(async move {
            task::supervisor(1, order_picker).await.context("Failed to start order picker")?;
            Ok(())
        });

        let order_monitor = Arc::new(order_monitor::OrderMonitor::new(
            self.db.clone(),
            self.provider.clone(),
            self.config_watcher.config.clone(),
            block_times,
            self.args.proof_market_addr,
        ));
        supervisor_tasks.spawn(async move {
            task::supervisor(1, order_monitor).await.context("Failed to start order monitor")?;
            Ok(())
        });

        let proving_service = Arc::new(
            proving::ProvingService::new(
                self.db.clone(),
                prover.clone(),
                self.config_watcher.config.clone(),
            )
            .await
            .context("Failed to initialize proving service")?,
        );

        supervisor_tasks.spawn(async move {
            task::supervisor(1, proving_service)
                .await
                .context("Failed to start proving service")?;
            Ok(())
        });

        let set_builder_img_data = self.get_set_builder_image().await?;
        let assessor_img_data = self.get_assessor_image().await?;

        let aggregator = Arc::new(
            aggregator::AggregatorService::new(
                self.db.clone(),
                self.provider.clone(),
                set_builder_img_data.0,
                set_builder_img_data.1,
                assessor_img_data.0,
                assessor_img_data.1,
                self.args.proof_market_addr,
                self.config_watcher.config.clone(),
                prover.clone(),
                block_times,
            )
            .await
            .context("Failed to initialize aggregator service")?,
        );

        supervisor_tasks.spawn(async move {
            task::supervisor(1, aggregator).await.context("Failed to start aggregator service")?;
            Ok(())
        });

        let submitter = Arc::new(submitter::Submitter::new(
            self.db.clone(),
            prover.clone(),
            self.provider.clone(),
            self.args.set_verifier_addr,
            self.args.proof_market_addr,
            set_builder_img_data.0,
        ));
        supervisor_tasks.spawn(async move {
            task::supervisor(1, submitter).await.context("Failed to start submitter service")?;
            Ok(())
        });

        // Monitor the different supervisor tasks
        while let Some(res) = supervisor_tasks.join_next().await {
            tracing::info!("Task exited: {res:?}");
            // TODO: Handle supervisor errors
        }

        Ok(())
    }
}

async fn upload_image_uri(prover: &ProverObj, order: &Order, max_size: usize) -> Result<String> {
    let uri = UriHandlerBuilder::new(&order.request.imageUrl)
        .set_max_size(max_size)
        .build()
        .context("Uri parse failure")?;

    if !uri.exists() {
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

        prover
            .upload_image(&image_id, image_data)
            .await
            .context("Failed to upload image to prover")?;

        Ok(image_id)
    } else {
        Ok(uri.id().context("Invalid image URI type")?)
    }
}
async fn upload_input_uri(prover: &ProverObj, order: &Order, max_size: usize) -> Result<String> {
    Ok(match order.request.input.inputType {
        InputType::Inline => prover
            .upload_input(order.request.input.data.to_vec())
            .await
            .context("Failed to upload input data")?,

        InputType::Url => {
            let input_uri_str =
                std::str::from_utf8(&order.request.input.data).context("input url is not utf8")?;
            tracing::debug!("Input URI string: {input_uri_str}");
            let input_uri = UriHandlerBuilder::new(input_uri_str)
                .set_max_size(max_size)
                .build()
                .context("Failed to parse input uri")?;

            if !input_uri.exists() {
                let input_data = input_uri
                    .fetch()
                    .await
                    .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?;

                prover.upload_input(input_data).await.context("Failed to upload input")?
            } else {
                input_uri.id().context("invalid input URI type")?
            }
        }
        //???
        _ => anyhow::bail!("Invalid input type: {:?}", order.request.input.inputType),
    })
}

#[cfg(feature = "test-utils")]
pub mod test_utils {

    use aggregation_set::SET_BUILDER_GUEST_PATH;
    use alloy::{
        network::{Ethereum, EthereumWallet},
        providers::{
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
                WalletFiller,
            },
            Identity, RootProvider,
        },
        transports::BoxTransport,
    };
    use anyhow::Result;
    use boundless_market::contracts::test_utils::TestCtx;

    use guest_assessor::ASSESSOR_GUEST_PATH;

    use tempfile::NamedTempFile;

    use url::Url;

    use crate::{config::Config, Args, Broker};

    /// Create a new broker from a test context.
    pub async fn broker_from_test_ctx(
        ctx: &TestCtx,
        rpc_url: Url,
    ) -> Result<
        Broker<
            BoxTransport,
            FillProvider<
                JoinFill<
                    JoinFill<
                        Identity,
                        JoinFill<
                            GasFiller,
                            JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>,
                        >,
                    >,
                    WalletFiller<EthereumWallet>,
                >,
                RootProvider<BoxTransport>,
                BoxTransport,
                Ethereum,
            >,
        >,
    > {
        let config_file = NamedTempFile::new().unwrap();
        let mut config = Config::default();
        config.prover.set_builder_guest_path = Some(SET_BUILDER_GUEST_PATH.into());
        config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
        config.market.mcycle_price = "0.00001".into();
        config.batcher.batch_size = Some(1);
        config.write(config_file.path()).await.unwrap();

        let args = Args {
            db_url: "sqlite::memory:".into(),
            config_file: config_file.path().to_path_buf(),
            proof_market_addr: ctx.proof_market_addr,
            set_verifier_addr: ctx.set_verifier_addr,
            rpc_url,
            priv_key: ctx.prover_signer.clone(),
            bento_api_url: None,
            bonsai_api_key: None,
            bonsai_api_url: None,
            deposit_amount: None,
        };
        Broker::new(args, ctx.prover_provider.clone()).await
    }
}

#[cfg(test)]
pub mod tests;
