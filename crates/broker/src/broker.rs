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

//! The top-level [`Broker`] orchestration: builds provers, starts the per-chain
//! pipelines, and runs the two-phase shutdown loop.
//!
//! Also hosts deployment resolution helpers ([`resolve_deployment`] and the
//! private `validate_deployment_config` it delegates to). Everything here used
//! to live in `lib.rs`.

use std::{path::PathBuf, sync::Arc};

use alloy::{
    network::Ethereum,
    primitives::Address,
    providers::{DynProvider, Provider, WalletProvider},
};
use alloy_chains::NamedChain;
use anyhow::{Context, Result};
use boundless_market::{
    contracts::boundless_market::BoundlessMarketService,
    order_stream_client::OrderStreamClient,
    prover_utils::{Erc1271GasCache, OrderRequest},
    storage::StorageDownloader,
    Deployment,
};
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::sha::Digest;
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    aggregator,
    args::{ChainPipeline, CoreArgs},
    chain_monitor_v2, channels,
    config::{ConfigLock, ConfigWatcher, TelemetryMode},
    db::DbObj,
    is_dev_mode, market_monitor, offchain_market_monitor, order_committer, order_evaluator,
    order_locker, order_pricer,
    order_types::OrderStateChange,
    provers,
    provers::ProverObj,
    proving, reaper, requestor_monitor, service_runner,
    service_runner::ServiceRunner,
    storage::ConfigurableDownloader,
    submitter, telemetry, version_check,
};

const NEW_ORDER_CHANNEL_CAPACITY: usize = 1000;
const ORDER_STATE_CHANNEL_CAPACITY: usize = 1000;
const TELEMETRY_CHANNEL_CAPACITY: usize = 40960;
const PRICER_CHANNEL_CAPACITY: usize = 1000;
const COMPLETION_CHANNEL_CAPACITY: usize = 1000;
const COMMITMENT_CHANNEL_CAPACITY: usize = 1000;
const COMMITMENT_COMPLETION_CHANNEL_CAPACITY: usize = 1000;

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

        // Decide once whether telemetry uses the full constructor (which requires
        // an order-stream client) or the debug constructor; warn if Enabled was
        // requested but no client is available, then fall through to debug mode.
        let telemetry_client = match telemetry_mode {
            TelemetryMode::Enabled => match order_stream_client.as_ref() {
                Some(client) => Some(client.clone()),
                None => {
                    tracing::warn!(
                        chain_id,
                        "TelemetryMode::Enabled requires order-stream; falling back to LogsOnly"
                    );
                    None
                }
            },
            TelemetryMode::LogsOnly => None,
        };

        let _telemetry_handle = telemetry::init_for_chain(chain_id, || {
            let (tx, rx) = mpsc::channel::<telemetry::TelemetryEvent>(TELEMETRY_CHANNEL_CAPACITY);
            let handle = telemetry::TelemetryHandle::new(tx);
            let service = match telemetry_client {
                Some(client) => telemetry::TelemetryService::new(
                    rx,
                    handle.clone(),
                    client,
                    signer.clone(),
                    config.clone(),
                    db.clone(),
                ),
                None => telemetry::TelemetryService::new_debug(
                    rx,
                    handle.clone(),
                    signer.clone(),
                    config.clone(),
                    db.clone(),
                ),
            };
            // Telemetry rides on the critical cancel token so it keeps emitting
            // broker heartbeats during the Phase 2 drain (in-flight committed
            // orders). Otherwise, monitoring sees the broker as offline within
            // seconds of SIGTERM, while it is in fact still actively proving.
            let cancel_token = runner.critical_cancel_token();
            runner.spawn_future_in_span(
                service_runner::Criticality::Critical,
                "telemetry service",
                chain_span.clone(),
                async move { telemetry::run_telemetry_service(service, cancel_token).await },
            );
            handle
        });

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
            (monitor as chain_monitor_v2::ChainMonitorObj, block_times)
        } else {
            let chain_monitor_service = Arc::new(
                chain_monitor_v2::ChainMonitorService::new(
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

            (chain_monitor_service as chain_monitor_v2::ChainMonitorObj, block_times)
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
