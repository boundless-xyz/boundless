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

use std::{cmp::max, future::Future, str::FromStr, sync::Arc, time::Duration};

use alloy::{
    network::{Ethereum, EthereumWallet, TxSigner},
    primitives::{Address, Bytes, U256},
    providers::{fillers::ChainIdFiller, DynProvider, Provider, ProviderBuilder},
    rpc::client::RpcClient,
    signers::{
        local::{LocalSignerError, PrivateKeySigner},
        Signer,
    },
    transports::{http::Http, layers::FallbackLayer},
};
use alloy_primitives::{utils::format_ether, Signature, B256};
use anyhow::{anyhow, bail, Context, Result};
use risc0_aggregation::SetInclusionReceipt;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use tower::ServiceBuilder;
use url::Url;

use crate::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        Fulfillment, FulfillmentData, ProofRequest, RequestError,
    },
    deployments::Deployment,
    dynamic_gas_filler::{DynamicGasFiller, PriorityMode},
    indexer_client::IndexerClient,
    nonce_layer::NonceProvider,
    order_stream_client::OrderStreamClient,
    price_provider::{
        MarketPricing, MarketPricingConfigBuilder, PriceProviderArc, StandardPriceProvider,
    },
    prover_utils::local_executor::LocalExecutor,
    request_builder::{
        Finalizer, FinalizerConfigBuilder, OfferLayer, OfferLayerConfigBuilder,
        ParameterizationMode, PreflightLayer, RequestBuilder, RequestIdLayer,
        RequestIdLayerConfigBuilder, StandardRequestBuilder, StandardRequestBuilderBuilderError,
        StorageLayer, StorageLayerConfigBuilder,
    },
    storage::{
        StandardDownloader, StandardUploader, StorageDownloader, StorageError, StorageUploader,
        StorageUploaderConfig,
    },
    util::NotProvided,
};

/// Funding mode for requests submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FundingMode {
    /// Always send `max_price` as the tx value.
    Always,

    /// Never send value with the request.
    ///
    /// Use this mode only if you are managing the onchain balance through other means
    /// (e.g., manual top-ups, external funding management).
    Never,

    /// Use available balance for funding the request.
    ///
    /// If the onchain balance is insufficient, the difference will be sent as value.
    AvailableBalance,

    /// Send value if the balance is below a configurable threshold.
    ///
    /// If the onchain balance is insufficient, the difference will be sent as value.
    /// It's important to set the threshold appropriately to avoid underfunding.
    BelowThreshold(U256),

    /// Maintain a minimum and maximum balance by funding the request accordingly.
    ///
    /// If the onchain balance is below `min_balance`, the request will be funded
    /// to bring the balance up to `max_balance`. This should minimize the number of
    /// onchain fundings while ensuring sufficient balance is maintained.
    MinMaxBalance {
        /// Minimum balance to maintain.
        min_balance: U256,
        /// Maximum balance to maintain.
        max_balance: U256,
    },
}
/// Builder for the [Client] with standard implementations for the required components.
#[derive(Clone)]
pub struct ClientBuilder<U, D, S> {
    deployment: Option<Deployment>,
    rpc_url: Option<Url>,
    rpc_urls: Vec<Url>,
    signer: Option<S>,
    uploader: Option<U>,
    downloader: Option<D>,
    tx_timeout: Option<std::time::Duration>,
    balance_alerts: Option<BalanceAlertConfig>,
    /// Optional price provider for fetching market prices.
    /// If set, takes precedence over the indexer URL from [Deployment]. Allows using any [PriceProviderArc] implementation.
    price_provider: Option<PriceProviderArc>,
    /// Optional price oracle manager for USD conversions in [OfferLayer].
    /// Required if [OfferLayerConfig] contains USD-denominated amounts.
    price_oracle_manager: Option<Arc<crate::price_oracle::PriceOracleManager>>,
    /// Configuration builder for [OfferLayer], part of [StandardRequestBuilder].
    pub offer_layer_config: OfferLayerConfigBuilder,
    /// Configuration builder for [StorageLayer], part of [StandardRequestBuilder].
    pub storage_layer_config: StorageLayerConfigBuilder,
    /// Configuration builder for [RequestIdLayer], part of [StandardRequestBuilder].
    pub request_id_layer_config: RequestIdLayerConfigBuilder,
    /// Configuration builder for [Finalizer], part of [StandardRequestBuilder].
    pub request_finalizer_config: FinalizerConfigBuilder,
    /// Funding mode for onchain requests.
    ///
    /// Defaults to [FundingMode::Always].
    /// [FundingMode::Never] can be used to never send value with the request.
    /// [FundingMode::AvailableBalance] can be used to only send value if the current onchain balance is insufficient.
    /// [FundingMode::BelowThreshold] can be used to send value only if the balance is below a configurable threshold.
    /// [FundingMode::MinMaxBalance] can be used to maintain a minimum balance by funding requests accordingly.
    pub funding_mode: FundingMode,
    /// Whether to skip preflight/pricing checks.
    ///
    /// If `Some(true)`, preflight checks are skipped.
    /// If `Some(false)`, preflight checks are run.
    /// If `None`, falls back to checking the `BOUNDLESS_IGNORE_PREFLIGHT` environment variable.
    pub skip_preflight: Option<bool>,
}

impl<U, D, S> Default for ClientBuilder<U, D, S> {
    fn default() -> Self {
        Self {
            deployment: None,
            rpc_url: None,
            rpc_urls: Vec::new(),
            signer: None,
            uploader: None,
            downloader: None,
            tx_timeout: None,
            balance_alerts: None,
            price_provider: None,
            price_oracle_manager: None,
            offer_layer_config: Default::default(),
            storage_layer_config: Default::default(),
            request_id_layer_config: Default::default(),
            request_finalizer_config: Default::default(),
            funding_mode: FundingMode::Always,
            skip_preflight: None,
        }
    }
}

impl ClientBuilder<NotProvided, NotProvided, NotProvided> {
    /// Create a new client builder.
    pub fn new() -> Self {
        // When GCS feature is enabled, install aws-lc-rs as the default crypto provider.
        // This is needed because GCS deps use aws-lc-rs while alloy uses ring.
        // Without this, rustls panics when both providers are compiled in.
        #[cfg(feature = "gcs")]
        {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }

        Self::default()
    }
}

/// A utility trait used in the [ClientBuilder] to handle construction of the [alloy] [Provider].
pub trait ClientProviderBuilder {
    /// Error returned by methods on this [ClientProviderBuilder].
    type Error;

    /// Build a provider connected to the given RPC URLs.
    fn build_provider(
        &self,
        rpc_urls: Vec<Url>,
    ) -> impl Future<Output = Result<DynProvider, Self::Error>>;

    /// Get the default signer address that will be used by this provider, or `None` if no signer.
    fn signer_address(&self) -> Option<Address>;
}

impl<U, D, S> ClientBuilder<U, D, S> {
    /// Collect all RPC URLs by merging rpc_url and rpc_urls.
    /// If both are provided, they are merged into a single list.
    fn collect_rpc_urls(&self) -> Result<Vec<Url>, anyhow::Error> {
        // Collect all RPC URLs (merge rpc_url and rpc_urls) and deduplicate
        let mut seen = std::collections::HashSet::new();
        if let Some(ref rpc_url) = self.rpc_url {
            seen.insert(rpc_url.clone());
        }
        seen.extend(self.rpc_urls.iter().cloned());
        let all_urls: Vec<Url> = seen.into_iter().collect();

        if all_urls.is_empty() {
            bail!("no RPC URLs provided, set at least one using with_rpc_url or with_rpc_urls");
        }

        Ok(all_urls)
    }

    /// Build a custom RPC client transport with fallback support for multiple URLs.
    fn build_fallback_transport(&self, urls: &[Url]) -> Result<RpcClient, anyhow::Error> {
        // Create HTTP transports for each URL
        let transports: Vec<Http<_>> = urls.iter().map(|url| Http::new(url.clone())).collect();

        // Configure FallbackLayer with all transports active
        let active_count = std::num::NonZeroUsize::new(transports.len())
            .context("at least one transport is required")?;
        let fallback_layer = FallbackLayer::default().with_active_transport_count(active_count);

        tracing::info!(
            "Configuring provider with fallback support: {} URLs: {:?}",
            urls.len(),
            urls
        );

        // Build transport with fallback layer
        let transport = ServiceBuilder::new().layer(fallback_layer).service(transports);

        // Create RPC client with the transport
        Ok(RpcClient::builder().transport(transport, false))
    }
}

impl<U, D, S> ClientProviderBuilder for ClientBuilder<U, D, S>
where
    S: TxSigner<Signature> + Send + Sync + Clone + 'static,
{
    type Error = anyhow::Error;

    async fn build_provider(&self, rpc_urls: Vec<Url>) -> Result<DynProvider, Self::Error> {
        let provider = match self.signer.clone() {
            Some(signer) => {
                let dynamic_gas_filler = DynamicGasFiller::new(
                    20, // 20% increase of gas limit
                    PriorityMode::Medium,
                    signer.address(),
                );

                // Build provider without erasing first (NonceProvider needs FillProvider)
                let balance_alerts = self.balance_alerts.clone().unwrap_or_default();

                let base_provider = if rpc_urls.len() > 1 {
                    // Multiple URLs - use fallback transport
                    let client = self.build_fallback_transport(&rpc_urls)?;
                    ProviderBuilder::new()
                        .disable_recommended_fillers()
                        .filler(ChainIdFiller::default())
                        .filler(dynamic_gas_filler)
                        .layer(BalanceAlertLayer::new(balance_alerts))
                        .connect_client(client)
                } else {
                    // Single URL - use regular provider
                    let url = rpc_urls.first().unwrap();
                    ProviderBuilder::new()
                        .disable_recommended_fillers()
                        .filler(ChainIdFiller::default())
                        .filler(dynamic_gas_filler)
                        .layer(BalanceAlertLayer::new(balance_alerts))
                        .connect(url.as_str())
                        .await
                        .with_context(|| format!("failed to connect provider to {url}"))?
                };

                NonceProvider::new(base_provider, EthereumWallet::from(signer)).erased()
            }
            None => {
                if rpc_urls.len() > 1 {
                    // Multiple URLs - use fallback transport
                    let client = self.build_fallback_transport(&rpc_urls)?;
                    ProviderBuilder::new().connect_client(client).erased()
                } else {
                    // Single URL - use regular provider
                    let url = rpc_urls.first().context("no RPC URL provided")?;
                    ProviderBuilder::new()
                        .connect(url.as_str())
                        .await
                        .with_context(|| format!("failed to connect provider to {url}"))?
                        .erased()
                }
            }
        };
        Ok(provider)
    }

    fn signer_address(&self) -> Option<Address> {
        self.signer.as_ref().map(|signer| signer.address())
    }
}

impl<U, D> ClientProviderBuilder for ClientBuilder<U, D, NotProvided> {
    type Error = anyhow::Error;

    async fn build_provider(&self, rpc_urls: Vec<Url>) -> Result<DynProvider, Self::Error> {
        let provider = if rpc_urls.len() > 1 {
            // Multiple URLs - use fallback transport
            let client = self.build_fallback_transport(&rpc_urls)?;
            ProviderBuilder::new().connect_client(client).erased()
        } else {
            // Single URL - use regular provider
            let url = rpc_urls.first().unwrap();
            ProviderBuilder::new()
                .connect(url.as_str())
                .await
                .with_context(|| format!("failed to connect provider to {url}"))?
                .erased()
        };
        Ok(provider)
    }

    fn signer_address(&self) -> Option<Address> {
        None
    }
}

impl<U, S> ClientBuilder<U, NotProvided, S> {
    /// Build the client with the [StandardDownloader].
    pub async fn build(
        self,
    ) -> Result<
        Client<
            DynProvider,
            U,
            StandardDownloader,
            StandardRequestBuilder<DynProvider, U, StandardDownloader>,
            S,
        >,
    >
    where
        U: Clone,
        ClientBuilder<U, StandardDownloader, S>: ClientProviderBuilder<Error = anyhow::Error>,
    {
        self.with_downloader(StandardDownloader::new().await).build().await
    }
}

impl<U, D: StorageDownloader, S> ClientBuilder<U, D, S> {
    /// Build the client.
    pub async fn build(
        self,
    ) -> Result<Client<DynProvider, U, D, StandardRequestBuilder<DynProvider, U, D>, S>>
    where
        U: Clone,
        D: Clone,
        Self: ClientProviderBuilder<Error = anyhow::Error>,
    {
        let all_urls = self.collect_rpc_urls()?;
        // It's safe to unwrap here because we know there's at least one URL.
        let first_rpc_url = all_urls.first().cloned().unwrap();
        let provider = self.build_provider(all_urls).await?;

        // Resolve the deployment information.
        let chain_id =
            provider.get_chain_id().await.context("failed to query chain ID from RPC provider")?;
        let deployment =
            self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id)).with_context(
                || format!("no deployment provided for unknown chain_id {chain_id}"),
            )?;

        // Check that the chain ID is matches the deployment, to avoid misconfigurations.
        if deployment.market_chain_id.map(|id| id != chain_id).unwrap_or(false) {
            bail!("RPC url does not match specified Boundless deployment: {chain_id} (RPC) != {} (Boundless)", deployment.market_chain_id.unwrap());
        }

        // Build the contract instances.
        let boundless_market = BoundlessMarketService::new(
            deployment.boundless_market_address,
            provider.clone(),
            self.signer_address().unwrap_or(Address::ZERO),
        );
        let set_verifier = SetVerifierService::new(
            deployment.set_verifier_address,
            provider.clone(),
            self.signer_address().unwrap_or(Address::ZERO),
        );

        // Safe unwrap: Since D is not NotProvided a downloader must be set
        let downloader = self.downloader.unwrap();

        // Build the order stream client, if a URL was provided.
        let offchain_client = deployment
            .order_stream_url
            .as_ref()
            .map(|order_stream_url| {
                let url = Url::parse(order_stream_url.as_ref())
                    .context("failed to parse order_stream_url")?;
                anyhow::Ok(OrderStreamClient::new(
                    url,
                    deployment.boundless_market_address,
                    chain_id,
                ))
            })
            .transpose()?;

        // Build the price provider - use explicit provider if set, otherwise create from deployment.indexer_url
        let price_provider: Option<PriceProviderArc> =
            if let Some(provider) = self.price_provider.clone() {
                Some(provider)
            } else {
                let market_pricing = MarketPricing::new(
                    first_rpc_url,
                    MarketPricingConfigBuilder::default()
                        .deployment(deployment.clone())
                        .build()
                        .with_context(|| {
                            format!(
                            "Failed to build MarketPricingConfig for deployment: {deployment:?}",
                        )
                        })?,
                );
                if let Some(url_str) = deployment.indexer_url.as_ref() {
                    let url = Url::parse(url_str.as_ref()).with_context(|| {
                        format!("Failed to parse indexer URL from deployment: {}", url_str)
                    })?;
                    let indexer_client = IndexerClient::new(url).with_context(|| {
                        format!(
                            "Failed to create indexer client from deployment indexer URL: {}",
                            url_str
                        )
                    })?;
                    Some(Arc::new(
                        StandardPriceProvider::new(indexer_client).with_fallback(market_pricing),
                    ))
                } else {
                    Some(Arc::new(StandardPriceProvider::<MarketPricing, MarketPricing>::new(
                        market_pricing,
                    )))
                }
            };

        // Set up the price oracle for USD conversions.
        // If none was explicitly provided, create one from the default config (CoinGecko + Chainlink).
        let price_oracle_manager = if self.price_oracle_manager.is_some() {
            self.price_oracle_manager.clone()
        } else {
            // Disable staleness check: no background refresh is spawned for the default oracle.
            let oracle_config = crate::price_oracle::PriceOracleConfig {
                max_secs_without_price_update: 0,
                ..Default::default()
            };
            match oracle_config.build(
                alloy_chains::NamedChain::try_from(chain_id)
                    .unwrap_or(alloy_chains::NamedChain::Mainnet),
                provider.clone(),
            ) {
                Ok(oracle) => {
                    oracle.refresh_all_rates().await;
                    tracing::debug!("Default price oracle initialized for USD conversions");
                    Some(Arc::new(oracle))
                }
                Err(e) => {
                    tracing::warn!("Failed to create default price oracle, USD preflight checks will be degraded: {e}");
                    None
                }
            }
        };

        // Build the RequestBuilder.
        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(StorageLayer::new(
                self.uploader.clone(),
                self.storage_layer_config.build()?,
            ))
            .preflight_layer(PreflightLayer::new(
                LocalExecutor::default(),
                Some(downloader.clone()),
            ))
            .offer_layer(
                OfferLayer::new(provider.clone(), self.offer_layer_config.build()?)
                    .with_price_provider(price_provider)
                    .with_price_oracle_manager(price_oracle_manager.clone()),
            )
            .request_id_layer(RequestIdLayer::new(
                boundless_market.clone(),
                self.request_id_layer_config.build()?,
            ))
            .finalizer(
                Finalizer::from(self.request_finalizer_config.build()?)
                    .with_price_oracle_manager(price_oracle_manager.clone()),
            )
            .build()?;

        let mut client = Client {
            boundless_market,
            set_verifier,
            uploader: self.uploader,
            downloader,
            offchain_client,
            signer: self.signer,
            request_builder: Some(request_builder),
            deployment,
            funding_mode: self.funding_mode,
        };

        if let Some(timeout) = self.tx_timeout {
            client = client.with_timeout(timeout);
        }

        if let Some(skip_preflight) = self.skip_preflight {
            client = client.with_skip_preflight(skip_preflight);
        }

        Ok(client)
    }
}

impl<U, D, S> ClientBuilder<U, D, S> {
    /// Set the [Deployment] of the Boundless Market that this client will use.
    ///
    /// If `None`, the builder will attempt to infer the deployment from the chain ID.
    pub fn with_deployment(self, deployment: impl Into<Option<Deployment>>) -> Self {
        Self { deployment: deployment.into(), ..self }
    }

    /// Set the RPC URL
    pub fn with_rpc_url(self, rpc_url: Url) -> Self {
        Self { rpc_url: Some(rpc_url), ..self }
    }

    /// Set the funding mode for onchain requests.
    pub fn with_funding_mode(self, funding_mode: FundingMode) -> Self {
        Self { funding_mode, ..self }
    }

    /// Set the parameterization mode for the offer layer.
    ///
    /// The parameterization mode is used to define the offering parameters for the request.
    /// The default is [ParameterizationMode::fulfillment()], which is conservative and ensures
    /// more provers can fulfill the request.
    ///
    /// # Example
    /// ```rust
    /// # use boundless_market::Client;
    /// use boundless_market::request_builder::ParameterizationMode;
    ///
    /// Client::builder().with_parameterization_mode(ParameterizationMode::fulfillment());
    /// ```
    pub fn with_parameterization_mode(self, parameterization_mode: ParameterizationMode) -> Self {
        self.config_offer_layer(|config| config.parameterization_mode(parameterization_mode))
    }

    /// Set additional RPC URLs for automatic failover.
    ///
    /// When multiple URLs are provided (via `with_rpc_url` and/or `with_rpc_urls`),
    /// they are merged into a single list. If 2+ URLs are provided, the client will
    /// use Alloy's FallbackLayer to distribute requests across multiple RPC endpoints
    /// with automatic failover. If only 1 URL is provided, a regular provider is used.
    ///
    /// # Example
    /// ```rust
    /// # use boundless_market::Client;
    /// # use url::Url;
    /// // Multiple URLs - uses fallback provider
    /// Client::builder()
    ///     .with_rpc_urls(vec![
    ///         Url::parse("https://rpc2.example.com").unwrap(),
    ///         Url::parse("https://rpc3.example.com").unwrap(),
    ///     ]);
    ///
    /// // Single URL - uses regular provider
    /// Client::builder()
    ///     .with_rpc_urls(vec![Url::parse("https://rpc.example.com").unwrap()]);
    /// ```
    pub fn with_rpc_urls(self, rpc_urls: Vec<Url>) -> Self {
        Self { rpc_urls, ..self }
    }

    /// Set the signer from the given private key.
    /// ```rust
    /// # use boundless_market::Client;
    /// use alloy::signers::local::PrivateKeySigner;
    ///
    /// Client::builder().with_private_key(PrivateKeySigner::random());
    /// ```
    pub fn with_private_key(
        self,
        private_key: impl Into<PrivateKeySigner>,
    ) -> ClientBuilder<U, D, PrivateKeySigner> {
        self.with_signer(private_key.into())
    }

    /// Set the signer from the given private key as a string.
    /// ```rust
    /// # use boundless_market::Client;
    ///
    /// Client::builder().with_private_key_str(
    ///     "0x1cee2499e12204c2ed600d780a22a67b3c5fff3310d984cca1f24983d565265c"
    /// ).unwrap();
    /// ```
    pub fn with_private_key_str(
        self,
        private_key: impl AsRef<str>,
    ) -> Result<ClientBuilder<U, D, PrivateKeySigner>, LocalSignerError> {
        Ok(self.with_signer(PrivateKeySigner::from_str(private_key.as_ref())?))
    }

    /// Set the signer and wallet.
    pub fn with_signer<Zi>(self, signer: impl Into<Option<Zi>>) -> ClientBuilder<U, D, Zi>
    where
        Zi: Signer + Clone + TxSigner<Signature> + Send + Sync + 'static,
    {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            signer: signer.into(),
            deployment: self.deployment,
            uploader: self.uploader,
            downloader: self.downloader,
            rpc_url: self.rpc_url,
            rpc_urls: self.rpc_urls,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            price_provider: self.price_provider.clone(),
            price_oracle_manager: self.price_oracle_manager.clone(),
            offer_layer_config: self.offer_layer_config,
            storage_layer_config: self.storage_layer_config,
            request_id_layer_config: self.request_id_layer_config,
            request_finalizer_config: self.request_finalizer_config,
            funding_mode: self.funding_mode,
            skip_preflight: self.skip_preflight,
        }
    }

    /// Set the transaction timeout in seconds
    pub fn with_timeout(self, tx_timeout: impl Into<Option<Duration>>) -> Self {
        Self { tx_timeout: tx_timeout.into(), ..self }
    }

    /// Set the balance alerts configuration
    pub fn with_balance_alerts(self, config: impl Into<Option<BalanceAlertConfig>>) -> Self {
        Self { balance_alerts: config.into(), ..self }
    }

    /// Set the storage uploader.
    ///
    /// The returned [ClientBuilder] will be generic over the provider [StorageUploader] type.
    pub fn with_uploader<Z: StorageUploader>(self, uploader: Option<Z>) -> ClientBuilder<Z, D, S> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            deployment: self.deployment,
            rpc_url: self.rpc_url,
            rpc_urls: self.rpc_urls,
            signer: self.signer,
            uploader,
            downloader: self.downloader,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            price_provider: self.price_provider.clone(),
            price_oracle_manager: self.price_oracle_manager.clone(),
            request_finalizer_config: self.request_finalizer_config,
            request_id_layer_config: self.request_id_layer_config,
            storage_layer_config: self.storage_layer_config,
            offer_layer_config: self.offer_layer_config,
            funding_mode: self.funding_mode,
            skip_preflight: self.skip_preflight,
        }
    }

    /// Sets the storage downloader for fetching data from URLs.
    pub fn with_downloader<Z: StorageDownloader>(self, downloader: Z) -> ClientBuilder<U, Z, S> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            deployment: self.deployment,
            rpc_url: self.rpc_url,
            rpc_urls: self.rpc_urls,
            signer: self.signer,
            uploader: self.uploader,
            downloader: Some(downloader),
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            price_provider: self.price_provider,
            price_oracle_manager: self.price_oracle_manager,
            request_finalizer_config: self.request_finalizer_config,
            request_id_layer_config: self.request_id_layer_config,
            storage_layer_config: self.storage_layer_config,
            offer_layer_config: self.offer_layer_config,
            funding_mode: self.funding_mode,
            skip_preflight: self.skip_preflight,
        }
    }

    /// Set the storage uploader from the given config
    pub async fn with_uploader_config(
        self,
        config: &StorageUploaderConfig,
    ) -> Result<ClientBuilder<StandardUploader, D, S>, StorageError> {
        let storage_uploader = match StandardUploader::from_config(config).await {
            Ok(storage_uploader) => Some(storage_uploader),
            Err(StorageError::NoUploader) => None,
            Err(e) => return Err(e),
        };
        Ok(self.with_uploader(storage_uploader))
    }

    /// Set a custom price provider for fetching market prices.
    ///
    /// If provided, the [OfferLayer] will use market prices (p10 and p99 percentiles)
    /// when [`OfferParams`](crate::request_builder::OfferParams) doesn't explicitly set min_price or max_price.
    ///
    /// This method allows you to use any implementation of [`PriceProvider`](crate::price_provider::PriceProvider), not just [IndexerClient].
    /// This is useful for testing with mock providers or using alternative price data sources.
    ///
    /// If not set, the indexer URL from the [Deployment] will be used to create an [IndexerClient].
    /// The price provider takes precedence over the deployment's indexer URL.
    ///
    /// ```rust
    /// # use boundless_market::client::ClientBuilder;
    /// # use boundless_market::price_provider::PriceProviderArc;
    /// // Example: Use a custom price provider
    /// // let custom_provider: PriceProviderArc = ...;
    /// // ClientBuilder::new().with_price_provider(Some(custom_provider));
    /// ```
    pub fn with_price_provider(
        mut self,
        price_provider: impl Into<Option<PriceProviderArc>>,
    ) -> Self {
        self.price_provider = price_provider.into();
        self
    }

    /// Set the price oracle manager for USD conversions in [OfferLayer].
    ///
    /// The price oracle manager is required if [crate::request_builder::OfferLayerConfig] contains USD-denominated
    /// amounts for pricing or collateral fields. If USD amounts are specified without a
    /// price oracle manager, an error will be returned during request submission.
    ///
    /// # Example
    /// ```rust,no_run
    /// # use boundless_market::client::ClientBuilder;
    /// # use boundless_market::price_oracle::PriceOracleManager;
    /// # use std::sync::Arc;
    /// // Example: Configure price oracle manager for USD conversions
    /// // let oracle_manager: Arc<PriceOracleManager> = ...;
    /// // ClientBuilder::new().with_price_oracle_manager(Some(oracle_manager));
    /// ```
    pub fn with_price_oracle_manager(
        mut self,
        price_oracle_manager: impl Into<Option<Arc<crate::price_oracle::PriceOracleManager>>>,
    ) -> Self {
        self.price_oracle_manager = price_oracle_manager.into();
        self
    }

    /// Modify the [OfferLayer] configuration used in the [StandardRequestBuilder].
    ///
    /// ```rust
    /// # use boundless_market::client::ClientBuilder;
    /// use boundless_market::price_oracle::{Amount, Asset};
    /// use alloy::primitives::utils::parse_units;
    ///
    /// ClientBuilder::new().config_offer_layer(|config| config
    ///     .max_price_per_cycle(Amount::new(
    ///         parse_units("0.1", "gwei").unwrap().into(),
    ///         Asset::ETH
    ///     ))
    ///     .ramp_up_period(36)
    ///     .lock_timeout(120)
    ///     .timeout(300)
    /// );
    /// ```
    pub fn config_offer_layer(
        mut self,
        f: impl FnOnce(&mut OfferLayerConfigBuilder) -> &mut OfferLayerConfigBuilder,
    ) -> Self {
        f(&mut self.offer_layer_config);
        self
    }

    /// Modify the [RequestIdLayer] configuration used in the [StandardRequestBuilder].
    ///
    /// ```rust
    /// # use boundless_market::client::ClientBuilder;
    /// use boundless_market::request_builder::RequestIdLayerMode;
    ///
    /// ClientBuilder::new().config_request_id_layer(|config| config
    ///     .mode(RequestIdLayerMode::Nonce)
    /// );
    /// ```
    pub fn config_request_id_layer(
        mut self,
        f: impl FnOnce(&mut RequestIdLayerConfigBuilder) -> &mut RequestIdLayerConfigBuilder,
    ) -> Self {
        f(&mut self.request_id_layer_config);
        self
    }

    /// Modify the [StorageLayer] configuration used in the [StandardRequestBuilder].
    ///
    /// ```rust
    /// # use boundless_market::client::ClientBuilder;
    /// ClientBuilder::new().config_storage_layer(|config| config
    ///     .inline_input_max_bytes(10240)
    /// );
    /// ```
    pub fn config_storage_layer(
        mut self,
        f: impl FnOnce(&mut StorageLayerConfigBuilder) -> &mut StorageLayerConfigBuilder,
    ) -> Self {
        f(&mut self.storage_layer_config);
        self
    }

    /// Modify the [Finalizer] configuration used in the [StandardRequestBuilder].
    pub fn config_request_finalizer(
        mut self,
        f: impl FnOnce(&mut FinalizerConfigBuilder) -> &mut FinalizerConfigBuilder,
    ) -> Self {
        f(&mut self.request_finalizer_config);
        self
    }

    /// Set whether to skip preflight/pricing checks on the request builder.
    ///
    /// If `true`, preflight checks are skipped.
    /// If `false`, preflight checks are run.
    /// If not called, falls back to checking the `BOUNDLESS_IGNORE_PREFLIGHT` environment variable.
    pub fn with_skip_preflight(self, skip: bool) -> Self {
        Self { skip_preflight: Some(skip), ..self }
    }
}

#[derive(Clone)]
#[non_exhaustive]
/// Client for interacting with the boundless market.
pub struct Client<
    P = DynProvider,
    U = StandardUploader,
    D = StandardDownloader,
    R = StandardRequestBuilder,
    Si = PrivateKeySigner,
> {
    /// Boundless market service.
    pub boundless_market: BoundlessMarketService<P>,
    /// Set verifier service.
    pub set_verifier: SetVerifierService<P>,
    /// [StandardUploader] to upload programs and inputs.
    ///
    /// If not provided, this client will not be able to upload programs or inputs.
    pub uploader: Option<U>,
    /// Downloader for fetching data from storage.
    pub downloader: D,
    /// [OrderStreamClient] to submit requests off-chain.
    ///
    /// If not provided, requests not only be sent onchain via a transaction.
    pub offchain_client: Option<OrderStreamClient>,
    /// Alloy [Signer] for signing requests.
    ///
    /// If not provided, requests must be pre-signed handing them to this client.
    pub signer: Option<Si>,
    /// [RequestBuilder] to construct [ProofRequest].
    ///
    /// If not provided, requests must be fully constructed before handing them to this client.
    pub request_builder: Option<R>,
    /// Deployment of Boundless that this client is connected to.
    pub deployment: Deployment,
    /// Funding mode for onchain requests.
    ///
    /// Defaults to [FundingMode::Always].
    /// [FundingMode::Never] can be used to never send value with the request.
    /// [FundingMode::AvailableBalance] can be used to only send value if the current onchain balance is insufficient.
    /// [FundingMode::BelowThreshold] can be used to send value only if the balance is below a configurable threshold.
    /// [FundingMode::MinMaxBalance] can be used to maintain a minimum balance by funding requests accordingly.
    pub funding_mode: FundingMode,
}

/// Alias for a [Client] instantiated with the standard implementations provided by this crate.
pub type StandardClient = Client<
    DynProvider,
    StandardUploader,
    StandardDownloader,
    StandardRequestBuilder<DynProvider, StandardUploader, StandardDownloader>,
    PrivateKeySigner,
>;

impl<P, U, D, Si> Client<P, U, D, StandardRequestBuilder<P, U, D>, Si> {
    fn with_skip_preflight(mut self, skip: bool) -> Self {
        if let Some(ref mut builder) = self.request_builder {
            builder.skip_preflight = Some(skip);
        }
        self
    }
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
/// Client error
pub enum ClientError {
    /// Storage error
    #[error("Storage error {0}")]
    StorageError(#[from] StorageError),
    /// Market error
    #[error("Market error {0}")]
    MarketError(#[from] MarketError),
    /// Request error
    #[error("RequestError {0}")]
    RequestError(#[from] RequestError),
    /// Error when trying to construct a [RequestBuilder].
    #[error("Error building RequestBuilder {0}")]
    BuilderError(#[from] StandardRequestBuilderBuilderError),
    /// General error
    #[error("Error {0}")]
    Error(#[from] anyhow::Error),
}

impl Client<NotProvided, NotProvided, NotProvided, NotProvided, NotProvided> {
    /// Create a [ClientBuilder] to construct a [Client].
    pub fn builder() -> ClientBuilder<NotProvided, NotProvided, NotProvided> {
        ClientBuilder::new()
    }
}

impl<P, D> Client<P, NotProvided, D, NotProvided, NotProvided>
where
    P: Provider<Ethereum> + 'static + Clone,
    D: StorageDownloader,
{
    /// Create a new client
    pub fn new(
        boundless_market: BoundlessMarketService<P>,
        set_verifier: SetVerifierService<P>,
        downloader: D,
    ) -> Self {
        let boundless_market = boundless_market.clone();
        let set_verifier = set_verifier.clone();
        Self {
            deployment: Deployment {
                boundless_market_address: *boundless_market.instance().address(),
                set_verifier_address: *set_verifier.instance().address(),
                market_chain_id: None,
                order_stream_url: None,
                collateral_token_address: None,
                verifier_router_address: None,
                indexer_url: None,
                deployment_block: None,
            },
            boundless_market,
            set_verifier,
            uploader: None,
            downloader,
            offchain_client: None,
            signer: None,
            request_builder: None,
            funding_mode: FundingMode::Always,
        }
    }
}

/// Computes the funding value to send for a given balance, max price, and funding mode.
/// Used by [Client::compute_funding_value] and by unit tests.
fn funding_value_for_balance(balance: U256, max_price: U256, funding_mode: FundingMode) -> U256 {
    match funding_mode {
        FundingMode::Always => max_price,

        FundingMode::Never => U256::ZERO,

        FundingMode::AvailableBalance => {
            if balance < max_price {
                max_price.saturating_sub(balance)
            } else {
                U256::ZERO
            }
        }

        FundingMode::BelowThreshold(threshold) => {
            if balance < threshold || balance < max_price {
                max(threshold.saturating_sub(balance), max_price.saturating_sub(balance))
            } else {
                U256::ZERO
            }
        }

        FundingMode::MinMaxBalance { min_balance, max_balance } => {
            if balance < min_balance || balance < max_price {
                let topup = if balance < min_balance {
                    max_balance.saturating_sub(balance)
                } else {
                    U256::ZERO
                };
                max(topup, max_price.saturating_sub(balance))
            } else {
                U256::ZERO
            }
        }
    }
}

impl<P, St, D, R, Si> Client<P, St, D, R, Si>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Get the provider
    pub fn provider(&self) -> P {
        self.boundless_market.instance().provider().clone()
    }

    /// Get the caller address
    pub fn caller(&self) -> Address {
        self.boundless_market.caller()
    }

    /// Set the Boundless market service
    pub fn with_boundless_market(self, boundless_market: BoundlessMarketService<P>) -> Self {
        Self {
            deployment: Deployment {
                boundless_market_address: *boundless_market.instance().address(),
                ..self.deployment
            },
            boundless_market,
            ..self
        }
    }

    /// Set the set verifier service
    pub fn with_set_verifier(self, set_verifier: SetVerifierService<P>) -> Self {
        Self {
            deployment: Deployment {
                set_verifier_address: *set_verifier.instance().address(),
                ..self.deployment
            },
            set_verifier,
            ..self
        }
    }

    /// Set the offchain client
    pub fn with_offchain_client(self, offchain_client: OrderStreamClient) -> Self {
        Self {
            deployment: Deployment {
                order_stream_url: Some(offchain_client.base_url.to_string().into()),
                ..self.deployment
            },
            offchain_client: Some(offchain_client),
            ..self
        }
    }

    /// Set the transaction timeout
    pub fn with_timeout(self, tx_timeout: Duration) -> Self {
        Self {
            boundless_market: self.boundless_market.with_timeout(tx_timeout),
            set_verifier: self.set_verifier.with_timeout(tx_timeout),
            ..self
        }
    }

    /// Set the funding mode for onchain requests.
    pub fn with_funding_mode(self, funding_mode: FundingMode) -> Self {
        Self { funding_mode, ..self }
    }

    /// Set the signer that will be used for signing [ProofRequest].
    /// ```rust
    /// # use boundless_market::Client;
    /// # use std::str::FromStr;
    /// # |client: Client| {
    /// use alloy::signers::local::PrivateKeySigner;
    ///
    /// client.with_signer(PrivateKeySigner::from_str(
    ///     "0x1cee2499e12204c2ed600d780a22a67b3c5fff3310d984cca1f24983d565265c")
    ///     .unwrap());
    /// # };
    /// ```
    pub fn with_signer<Zi>(self, signer: Zi) -> Client<P, St, D, R, Zi> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        Client {
            signer: Some(signer),
            boundless_market: self.boundless_market,
            set_verifier: self.set_verifier,
            uploader: self.uploader,
            downloader: self.downloader,
            offchain_client: self.offchain_client,
            request_builder: self.request_builder,
            deployment: self.deployment,
            funding_mode: self.funding_mode,
        }
    }

    /// Upload a program binary to the storage uploader.
    pub async fn upload_program(&self, program: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageUploader,
    {
        Ok(self
            .uploader
            .as_ref()
            .context("Storage uploader not set")?
            .upload_program(program)
            .await
            .context("Failed to upload program")?)
    }

    /// Upload input to the storage uploader.
    pub async fn upload_input(&self, input: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageUploader,
    {
        Ok(self
            .uploader
            .as_ref()
            .context("Storage uploader not set")?
            .upload_input(input)
            .await
            .context("Failed to upload input")?)
    }

    /// Downloads the content at the given URL using the configured downloader.
    pub async fn download(&self, url: &str) -> Result<Vec<u8>, ClientError>
    where
        D: StorageDownloader,
    {
        Ok(self
            .downloader
            .download(url)
            .await
            .with_context(|| format!("Failed to download {}", url))?)
    }

    /// Initial parameters that will be used to build a [ProofRequest] using the [RequestBuilder].
    pub fn new_request<Params>(&self) -> Params
    where
        R: RequestBuilder<Params>,
        Params: Default,
    {
        Params::default()
    }

    /// Build a proof request from the given parameters.
    ///
    /// Requires a [RequestBuilder] to be provided. After building, pricing validation
    /// is run to check if the request will likely be accepted by provers.
    ///
    /// If a signer is available on the client, the request will be signed for full validation.
    /// If no signer is available, pricing validation still runs but without signing.
    ///
    /// Pricing checks can be skipped by setting the `BOUNDLESS_IGNORE_PREFLIGHT` environment variable.
    pub async fn build_request<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<ProofRequest, ClientError>
    where
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        let request_builder =
            self.request_builder.as_ref().context("request_builder is not set on Client")?;
        tracing::debug!("Building request");
        let request = request_builder.build(params).await.map_err(Into::into)?;
        tracing::debug!("Built request with id {:x}", request.id);

        Ok(request)
    }
}

impl<P, U, D, R, Si> Client<P, U, D, R, Si>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    async fn compute_funding_value(
        &self,
        client_address: Address,
        max_price: U256,
    ) -> Result<U256, ClientError> {
        let balance = self.boundless_market.balance_of(client_address).await?;
        let value = funding_value_for_balance(balance, max_price, self.funding_mode);

        if value > U256::ZERO {
            if let FundingMode::BelowThreshold(threshold) = self.funding_mode {
                if balance < threshold {
                    tracing::warn!(
                        "Client balance is {} ETH < threshold {} ETH. \
                         Sending additional funds to top up the balance.",
                        format_ether(balance),
                        format_ether(threshold),
                    );
                }
            } else if let FundingMode::MinMaxBalance { min_balance, max_balance } =
                self.funding_mode
            {
                if balance < min_balance {
                    tracing::warn!(
                        "Client balance is {} ETH < min {} ETH. \
                         Sending {} ETH (max target {}).",
                        format_ether(balance),
                        format_ether(min_balance),
                        format_ether(value),
                        format_ether(max_balance),
                    );
                }
            }
        }

        if let FundingMode::Always = self.funding_mode {
            if balance > max_price.saturating_mul(U256::from(3u8)) {
                tracing::warn!(
                    "Client balance is {} ETH, that is more than 3x the value being sent. \
                     Consider switching to a different funding mode to avoid overfunding.",
                    format_ether(balance),
                );
            }
        }

        Ok(value)
    }

    /// Build and submit a proof request by sending an onchain transaction.
    ///
    /// Requires a [Signer] to be provided to sign the request, and a [RequestBuilder] to be
    /// provided to build the request from the given parameters.
    pub async fn submit_onchain<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        let signer = self.signer.as_ref().context("signer is set on Client")?;
        self.submit_request_onchain_with_signer(&self.build_request(params).await?, signer).await
    }

    /// Submit a proof request in an onchain transaction.
    ///
    /// Requires a signer to be set to sign the request.
    pub async fn submit_request_onchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
    {
        let signer = self.signer.as_ref().context("signer not set")?;
        self.submit_request_onchain_with_signer(request, signer).await
    }

    /// Submit a proof request in a transaction.
    ///
    /// Accepts a signer to sign the request. Note that the transaction will be signed by the alloy
    /// [Provider] on this [Client].
    pub async fn submit_request_onchain_with_signer(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<(U256, u64), ClientError> {
        let mut request = request.clone();

        if request.id == U256::ZERO {
            request.id = self.boundless_market.request_id_from_rand().await?;
        };
        let client_address = request.client_address();
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()))?;
        };

        request.validate()?;

        let max_price = U256::from(request.offer.maxPrice);
        let value = self.compute_funding_value(client_address, max_price).await?;

        let request_id =
            self.boundless_market.submit_request_with_value(&request, signer, value).await?;

        Ok((request_id, request.expires_at()))
    }

    /// Submit a pre-signed proof in an onchain transaction.
    ///
    /// Accepts a signature bytes to be used as the request signature.
    pub async fn submit_request_onchain_with_signature(
        &self,
        request: &ProofRequest,
        signature: impl Into<Bytes>,
    ) -> Result<(U256, u64), ClientError> {
        let request = request.clone();
        request.validate()?;

        let request_id =
            self.boundless_market.submit_request_with_signature(&request, signature).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Build and submit a proof request.
    ///
    /// Automatically uses offchain submission via the order stream service if available,
    /// otherwise falls back to onchain submission.
    ///
    /// Requires a [Signer] to be provided to sign the request, and a [RequestBuilder] to be
    /// provided to build the request from the given parameters.
    pub async fn submit<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        let request = self.build_request(params).await?;
        self.submit_request(&request).await
    }

    /// Submit a proof request (already built).
    ///
    /// Automatically uses offchain submission via the order stream service if available,
    /// otherwise falls back to onchain submission.
    ///
    /// Requires a signer to be set on the [Client] to sign the request.
    pub async fn submit_request(&self, request: &ProofRequest) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
    {
        let signer = self.signer.as_ref().context("signer not set")?;
        self.submit_request_with_signer(request, signer).await
    }

    /// Submit a proof request with a provided signer.
    ///
    /// Automatically uses offchain submission via the order stream service if available,
    /// otherwise falls back to onchain submission.
    ///
    /// Accepts a signer parameter to sign the request.
    pub async fn submit_request_with_signer(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
    {
        let mut request = request.clone();

        if request.id == U256::ZERO {
            request.id = self.boundless_market.request_id_from_rand().await?;
        };
        let client_address = request.client_address();
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()))?;
        };
        request.validate()?;

        let max_price = U256::from(request.offer.maxPrice);
        let mut value = self.compute_funding_value(client_address, max_price).await?;

        // Try offchain submission if available
        if let Some(offchain_client) = &self.offchain_client {
            // For offchain, deposit the value first if needed
            if value > 0 {
                self.boundless_market.deposit(value).await?;
                value = U256::ZERO; // no need to send value again in case we fallback to onchain submission
            }

            match offchain_client.submit_request(&request, signer).await {
                Ok(order) => return Ok((order.request.id, request.expires_at())),
                Err(e) => {
                    tracing::warn!(
                        "Failed to submit request offchain: {e:?}, falling back to onchain submission"
                    );
                    // Fall through to onchain submission
                }
            }
        }

        // Fallback to onchain submission (or use directly if offchain_client is None)
        let request_id =
            self.boundless_market.submit_request_with_value(&request, signer, value).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Build and submit a proof request offchain via the order stream service.
    ///
    /// Requires a [Signer] to be provided to sign the request, and a [RequestBuilder] to be
    /// provided to build the request from the given parameters.
    pub async fn submit_offchain<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        let signer = self.signer.as_ref().context("signer is set on Client")?;
        self.submit_request_offchain_with_signer(&self.build_request(params).await?, signer).await
    }

    /// Submit a proof request offchain via the order stream service.
    ///
    /// Requires a signer to be set to sign the request.
    pub async fn submit_request_offchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), ClientError>
    where
        Si: Signer,
    {
        let signer = self.signer.as_ref().context("signer not set")?;
        self.submit_request_offchain_with_signer(request, signer).await
    }

    /// Submit a proof request offchain via the order stream service.
    ///
    /// Accepts a signer to sign the request.
    pub async fn submit_request_offchain_with_signer(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<(U256, u64), ClientError> {
        let offchain_client = self
            .offchain_client
            .as_ref()
            .context("Order stream client not available. Please provide an order stream URL")?;
        let mut request = request.clone();

        if request.id == U256::ZERO {
            request.id = self.boundless_market.request_id_from_rand().await?;
        };
        let client_address = request.client_address();
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()))?;
        };

        request.validate()?;

        let max_price = U256::from(request.offer.maxPrice);
        let value = self.compute_funding_value(client_address, max_price).await?;
        if value > 0 {
            self.boundless_market.deposit(value).await?;
        }

        let order = offchain_client.submit_request(&request, signer).await?;

        Ok((order.request.id, request.expires_at()))
    }

    /// Wait for a request to be fulfilled.
    ///
    /// The check interval is the time between each check for fulfillment.
    /// The timeout is the maximum time to wait for the request to be fulfilled.
    pub async fn wait_for_request_fulfillment(
        &self,
        request_id: U256,
        check_interval: std::time::Duration,
        expires_at: u64,
    ) -> Result<Fulfillment, ClientError> {
        Ok(self
            .boundless_market
            .wait_for_request_fulfillment(request_id, check_interval, expires_at)
            .await?)
    }

    /// Get the [SetInclusionReceipt] for a request.
    ///
    /// This method fetches the fulfillment data for a request and constructs the set inclusion receipt.
    ///
    /// # Parameters
    ///
    /// * `request_id` - The unique identifier of the proof request
    /// * `image_id` - The image ID for the receipt claim
    /// * `search_to_block` - Optional lower bound for the block search range. The search will go backwards
    ///   down to this block number. Combined with `search_from_block` to define a specific range.
    /// * `search_from_block` - Optional upper bound for the block search range. The search starts backwards
    ///   from this block. Defaults to the latest block if not specified. Set this to a block number near
    ///   when the request was fulfilled to reduce RPC calls and cost when querying old fulfillments.
    ///
    /// # Default Search Behavior
    ///
    /// Without explicit block bounds, the onchain search covers blocks according to
    /// EventQueryConfig.block_range and EventQueryConfig.max_iterations.
    /// Fulfillment events older than this default range will not be found unless you provide explicit `search_to_block` and `search_from_block` parameters.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use anyhow::Result;
    /// use alloy::primitives::{B256, Bytes, U256};
    /// use boundless_market::client::ClientBuilder;
    /// use risc0_aggregation::SetInclusionReceipt;
    /// use risc0_zkvm::ReceiptClaim;
    ///
    /// async fn fetch_set_inclusion_receipt(request_id: U256, image_id: B256) -> Result<(Bytes, SetInclusionReceipt<ReceiptClaim>)> {
    ///     let client = ClientBuilder::new().build().await?;
    ///
    ///     // For recent requests
    ///     let (journal, receipt) = client.fetch_set_inclusion_receipt(
    ///         request_id,
    ///         image_id,
    ///         None,
    ///         None,
    ///     ).await?;
    ///
    ///     // For old requests with explicit block range
    ///     let (journal, receipt) = client.fetch_set_inclusion_receipt(
    ///         request_id,
    ///         image_id,
    ///         Some(1000000),  // search_to_block
    ///         Some(1500000),  // search_from_block
    ///     ).await?;
    ///
    ///     Ok((journal, receipt))
    /// }
    /// ```
    pub async fn fetch_set_inclusion_receipt(
        &self,
        request_id: U256,
        image_id: B256,
        search_to_block: Option<u64>,
        search_from_block: Option<u64>,
    ) -> Result<(Bytes, SetInclusionReceipt<ReceiptClaim>), ClientError> {
        // TODO(#646): This logic is only correct under the assumption there is a single set
        // verifier.
        let fulfillment = self
            .boundless_market
            .get_request_fulfillment(request_id, search_to_block, search_from_block)
            .await?;
        match fulfillment.data().context("failed to decode fulfillment data")? {
            FulfillmentData::None => Err(ClientError::Error(anyhow!(
                "No fulfillment data found for set inclusion receipt"
            ))),
            FulfillmentData::ImageIdAndJournal(_, journal) => {
                let claim = ReceiptClaim::ok(Digest::from(image_id.0), journal.to_vec());
                let receipt = self
                    .set_verifier
                    .fetch_receipt_with_claim(fulfillment.seal, claim, journal.to_vec())
                    .await?;
                Ok((journal, receipt))
            }
        }
    }

    /// Fetch a proof request and its signature, querying first offchain, and then onchain.
    ///
    /// This method does not verify the signature, and the order cannot be guarenteed to be
    /// authorized by this call alone.
    ///
    /// The request is first looked up offchain using the order stream service, then onchain using
    /// event queries. The offchain query is sent first, since it is quick to check. Querying
    /// onchain uses event logs, and will take more time to find requests that are further in the
    /// past.
    ///
    /// # Parameters
    ///
    /// * `request_id` - The unique identifier of the proof request
    /// * `tx_hash` - Optional transaction hash containing the request. Providing this will speed up
    ///   onchain queries by fetching the transaction directly instead of searching through events.
    /// * `request_digest` - Optional digest to differentiate between multiple requests with the same ID.
    ///   If `None`, the first found request matching the ID will be returned.
    /// * `search_to_block` - Optional lower bound for the block search range. The search will go backwards
    ///   down to this block number. Combined with `search_from_block` to define a specific range.
    /// * `search_from_block` - Optional upper bound for the block search range. The search starts backwards
    ///   from this block. Defaults to the latest block if not specified. Set this to a block number near
    ///   when the request was submitted to reduce RPC calls and cost when querying old requests.
    ///
    /// # Default Search Behavior
    ///
    /// Without explicit block bounds, the onchain search covers blocks according to
    /// EventQueryConfig.block_range and EventQueryConfig.max_iterations.
    /// Fulfillment events older than this default range will not be found unless you provide explicit `search_to_block` and `search_from_block` parameters.
    ///
    /// Providing both bounds overrides the default iteration limit to ensure the full specified range is searched.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # use alloy::primitives::U256;
    /// # use boundless_market::client::ClientBuilder;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = ClientBuilder::new().build().await?;
    ///
    /// // Query a recent request (no block bounds needed)
    /// let (request, sig) = client.fetch_proof_request(
    ///     U256::from(123),
    ///     None,
    ///     None,
    ///     None,
    ///     None,
    /// ).await?;
    ///
    /// // Query an old request with explicit block range (e.g., blocks 1000000 to 1500000)
    /// let (request, sig) = client.fetch_proof_request(
    ///     U256::from(456),
    ///     None,
    ///     None,
    ///     Some(1000000),  // search_to_block (lower bound)
    ///     Some(1500000),  // search_from_block (upper bound)
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn fetch_proof_request(
        &self,
        request_id: U256,
        tx_hash: Option<B256>,
        request_digest: Option<B256>,
        search_to_block: Option<u64>,
        search_from_block: Option<u64>,
    ) -> Result<(ProofRequest, Bytes), ClientError> {
        if let Some(ref order_stream_client) = self.offchain_client {
            tracing::debug!("Querying the order stream for request: 0x{request_id:x} using request_digest {request_digest:?}");
            match order_stream_client.fetch_order(request_id, request_digest).await {
                Ok(order) => {
                    tracing::debug!("Found request 0x{request_id:x} offchain");
                    return Ok((order.request, Bytes::from(order.signature.as_bytes())));
                }
                Err(err) => {
                    // TODO: Provide a type-safe way to handle this error.
                    if err.to_string().contains("No order found") {
                        tracing::debug!("Request 0x{request_id:x} not found offchain");
                    } else {
                        tracing::error!(
                            "Error querying order stream for request 0x{request_id:x}; err = {err}"
                        );
                    }
                }
            }
        } else {
            tracing::debug!("Skipping query for request offchain; no order stream client provided");
        }

        tracing::debug!(
            "Querying the blockchain for request: 0x{request_id:x} using tx_hash {tx_hash:?}"
        );
        match self
            .boundless_market
            .get_submitted_request(request_id, tx_hash, search_to_block, search_from_block)
            .await
        {
            Ok((proof_request, signature)) => Ok((proof_request, signature)),
            Err(err @ MarketError::RequestNotFound(..)) => Err(err.into()),
            err @ Err(_) => err
                .with_context(|| format!("error querying for 0x{request_id:x} onchain"))
                .map_err(Into::into),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{funding_value_for_balance, FundingMode};
    use alloy::primitives::U256;

    #[test]
    fn funding_always_sends_max_price() {
        let max_price = U256::from(20u64);
        assert_eq!(
            funding_value_for_balance(U256::ZERO, max_price, FundingMode::Always),
            max_price
        );
        assert_eq!(
            funding_value_for_balance(U256::from(100u64), max_price, FundingMode::Always),
            max_price
        );
    }

    #[test]
    fn funding_never_sends_zero() {
        let balance = U256::from(5u64);
        let max_price = U256::from(20u64);
        assert_eq!(funding_value_for_balance(balance, max_price, FundingMode::Never), U256::ZERO);
    }

    #[test]
    fn funding_available_balance_sends_shortfall_when_insufficient() {
        let balance = U256::from(5u64);
        let max_price = U256::from(20u64);
        assert_eq!(
            funding_value_for_balance(balance, max_price, FundingMode::AvailableBalance),
            U256::from(15u64)
        );
    }

    #[test]
    fn funding_available_balance_sends_zero_when_sufficient() {
        let balance = U256::from(25u64);
        let max_price = U256::from(20u64);
        assert_eq!(
            funding_value_for_balance(balance, max_price, FundingMode::AvailableBalance),
            U256::ZERO
        );
    }

    #[test]
    fn funding_below_threshold_balance_above_threshold_below_max_price_sends_shortfall() {
        // Balance >= threshold but < max_price: must send (max_price - balance) to fund request.
        let balance = U256::from(15u64);
        let threshold = U256::from(10u64);
        let max_price = U256::from(20u64);

        let value =
            funding_value_for_balance(balance, max_price, FundingMode::BelowThreshold(threshold));

        assert_eq!(value, U256::from(5u64), "should send shortfall for this request");
    }

    #[test]
    fn funding_below_threshold_balance_below_threshold_sends_max_of_topup_and_shortfall() {
        let balance = U256::from(5u64);
        let threshold = U256::from(10u64);
        let max_price = U256::from(20u64);

        let value =
            funding_value_for_balance(balance, max_price, FundingMode::BelowThreshold(threshold));

        assert_eq!(value, U256::from(15u64)); // max(5, 15) = 15
    }

    #[test]
    fn funding_below_threshold_balance_above_max_price_sends_zero() {
        let balance = U256::from(25u64);
        let threshold = U256::from(10u64);
        let max_price = U256::from(20u64);

        let value =
            funding_value_for_balance(balance, max_price, FundingMode::BelowThreshold(threshold));

        assert_eq!(value, U256::ZERO);
    }

    #[test]
    fn funding_min_max_balance_above_min_below_max_price_sends_shortfall() {
        // Balance >= min_balance but < max_price: must send (max_price - balance) to fund request.
        let balance = U256::from(15u64);
        let min_balance = U256::from(10u64);
        let max_balance = U256::from(100u64);
        let max_price = U256::from(20u64);

        let value = funding_value_for_balance(
            balance,
            max_price,
            FundingMode::MinMaxBalance { min_balance, max_balance },
        );

        assert_eq!(value, U256::from(5u64), "should send shortfall for this request");
    }

    #[test]
    fn funding_min_max_balance_below_min_sends_max_of_topup_and_shortfall() {
        let balance = U256::from(5u64);
        let min_balance = U256::from(10u64);
        let max_balance = U256::from(100u64);
        let max_price = U256::from(20u64);

        let value = funding_value_for_balance(
            balance,
            max_price,
            FundingMode::MinMaxBalance { min_balance, max_balance },
        );

        assert_eq!(value, U256::from(95u64)); // max(95, 15) = 95
    }

    #[test]
    fn funding_min_max_balance_above_max_price_sends_zero() {
        let balance = U256::from(25u64);
        let min_balance = U256::from(10u64);
        let max_balance = U256::from(100u64);
        let max_price = U256::from(20u64);

        let value = funding_value_for_balance(
            balance,
            max_price,
            FundingMode::MinMaxBalance { min_balance, max_balance },
        );

        assert_eq!(value, U256::ZERO);
    }
}
