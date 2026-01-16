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
    request_builder::{
        FinalizerConfigBuilder, OfferLayer, OfferLayerConfigBuilder, ParameterizationMode,
        RequestBuilder, RequestIdLayer, RequestIdLayerConfigBuilder, StandardRequestBuilder,
        StandardRequestBuilderBuilderError, StorageLayer, StorageLayerConfigBuilder,
    },
    storage::{
        StandardStorageProvider, StandardStorageProviderError, StorageProvider,
        StorageProviderConfig,
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
pub struct ClientBuilder<St = NotProvided, Si = NotProvided> {
    deployment: Option<Deployment>,
    rpc_url: Option<Url>,
    rpc_urls: Vec<Url>,
    signer: Option<Si>,
    storage_provider: Option<St>,
    tx_timeout: Option<std::time::Duration>,
    balance_alerts: Option<BalanceAlertConfig>,
    /// Optional price provider for fetching market prices.
    /// If set, takes precedence over the indexer URL from [Deployment]. Allows using any [PriceProviderArc] implementation.
    price_provider: Option<PriceProviderArc>,
    /// Configuration builder for [OfferLayer], part of [StandardRequestBuilder].
    pub offer_layer_config: OfferLayerConfigBuilder,
    /// Configuration builder for [StorageLayer], part of [StandardRequestBuilder].
    pub storage_layer_config: StorageLayerConfigBuilder,
    /// Configuration builder for [RequestIdLayer], part of [StandardRequestBuilder].
    pub request_id_layer_config: RequestIdLayerConfigBuilder,
    /// Configuration builder for [Finalizer][crate::request_builder::Finalizer], part of [StandardRequestBuilder].
    pub request_finalizer_config: FinalizerConfigBuilder,
    /// Funding mode for onchain requests.
    ///
    /// Defaults to [FundingMode::Always].
    /// [FundingMode::Never] can be used to never send value with the request.
    /// [FundingMode::AvailableBalance] can be used to only send value if the current onchain balance is insufficient.
    /// [FundingMode::BelowThreshold] can be used to send value only if the balance is below a configurable threshold.
    /// [FundingMode::MinMaxBalance] can be used to maintain a minimum balance by funding requests accordingly.
    pub funding_mode: FundingMode,
}

impl<St, Si> Default for ClientBuilder<St, Si> {
    fn default() -> Self {
        Self {
            deployment: None,
            rpc_url: None,
            rpc_urls: Vec::new(),
            signer: None,
            storage_provider: None,
            tx_timeout: None,
            balance_alerts: None,
            price_provider: None,
            offer_layer_config: Default::default(),
            storage_layer_config: Default::default(),
            request_id_layer_config: Default::default(),
            request_finalizer_config: Default::default(),
            funding_mode: FundingMode::Always,
        }
    }
}

impl ClientBuilder {
    /// Create a new client builder.
    pub fn new() -> Self {
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

impl<St, Si> ClientBuilder<St, Si> {
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

impl<St, Si> ClientProviderBuilder for ClientBuilder<St, Si>
where
    Si: TxSigner<Signature> + Send + Sync + Clone + 'static,
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

impl<St> ClientProviderBuilder for ClientBuilder<St, NotProvided> {
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

impl<St, Si> ClientBuilder<St, Si> {
    /// Build the client
    pub async fn build(
        self,
    ) -> Result<Client<DynProvider, St, StandardRequestBuilder<DynProvider, St>, Si>>
    where
        St: Clone,
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
        let price_provider: Option<PriceProviderArc> = if let Some(provider) =
            self.price_provider.clone()
        {
            Some(provider)
        } else if let Some(url_str) = deployment.indexer_url.as_ref() {
            let url = Url::parse(url_str.as_ref()).with_context(|| {
                format!("Failed to parse indexer URL from deployment: {}", url_str)
            })?;
            let indexer_client = IndexerClient::new(url).with_context(|| {
                format!("Failed to create indexer client from deployment indexer URL: {}", url_str)
            })?;
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
            Some(Arc::new(StandardPriceProvider::new(indexer_client).with_fallback(market_pricing)))
        } else {
            None
        };

        // Build the RequestBuilder.
        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(StorageLayer::new(
                self.storage_provider.clone(),
                self.storage_layer_config.build()?,
            ))
            .offer_layer(
                OfferLayer::new(provider.clone(), self.offer_layer_config.build()?)
                    .with_price_provider(price_provider),
            )
            .request_id_layer(RequestIdLayer::new(
                boundless_market.clone(),
                self.request_id_layer_config.build()?,
            ))
            .finalizer(self.request_finalizer_config.build()?)
            .build()?;

        let mut client = Client {
            boundless_market,
            set_verifier,
            storage_provider: self.storage_provider,
            offchain_client,
            signer: self.signer,
            request_builder: Some(request_builder),
            deployment,
            funding_mode: self.funding_mode,
        };

        if let Some(timeout) = self.tx_timeout {
            client = client.with_timeout(timeout);
        }

        Ok(client)
    }

    /// Set the [Deployment] of the Boundless Market that this client will use.
    ///
    /// If `None`, the builder will attempty to infer the deployment from the chain ID.
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
    /// The fulfillment parameterization mode is [ParameterizationMode::fulfillment()], which is the default.
    /// The latency parameterization mode is [ParameterizationMode::latency()], which is faster and allows for faster fulfillment,
    /// at the cost of higher prices and lower fulfillment guarantees.
    ///
    /// # Example
    /// ```rust
    /// # use boundless_market::Client;
    /// use boundless_market::request_builder::ParameterizationMode;
    ///
    /// Client::builder().with_parameterization_mode(ParameterizationMode::latency());
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
    ) -> ClientBuilder<St, PrivateKeySigner> {
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
    ) -> Result<ClientBuilder<St, PrivateKeySigner>, LocalSignerError> {
        Ok(self.with_signer(PrivateKeySigner::from_str(private_key.as_ref())?))
    }

    /// Set the signer and wallet.
    pub fn with_signer<Zi>(self, signer: impl Into<Option<Zi>>) -> ClientBuilder<St, Zi>
    where
        Zi: Signer + Clone + TxSigner<Signature> + Send + Sync + 'static,
    {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            signer: signer.into(),
            deployment: self.deployment,
            storage_provider: self.storage_provider,
            rpc_url: self.rpc_url,
            rpc_urls: self.rpc_urls,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            price_provider: self.price_provider.clone(),
            offer_layer_config: self.offer_layer_config,
            storage_layer_config: self.storage_layer_config,
            request_id_layer_config: self.request_id_layer_config,
            request_finalizer_config: self.request_finalizer_config,
            funding_mode: self.funding_mode,
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

    /// Set the storage provider.
    ///
    /// The returned [ClientBuilder] will be generic over the provider [StorageProvider] type.
    pub fn with_storage_provider<Z: StorageProvider>(
        self,
        storage_provider: Option<Z>,
    ) -> ClientBuilder<Z, Si> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            storage_provider,
            deployment: self.deployment,
            rpc_url: self.rpc_url,
            rpc_urls: self.rpc_urls,
            signer: self.signer,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            price_provider: self.price_provider.clone(),
            request_finalizer_config: self.request_finalizer_config,
            request_id_layer_config: self.request_id_layer_config,
            storage_layer_config: self.storage_layer_config,
            offer_layer_config: self.offer_layer_config,
            funding_mode: self.funding_mode,
        }
    }

    /// Set the storage provider from the given config
    pub fn with_storage_provider_config(
        self,
        config: &StorageProviderConfig,
    ) -> Result<ClientBuilder<StandardStorageProvider, Si>, StandardStorageProviderError> {
        let storage_provider = match StandardStorageProvider::from_config(config) {
            Ok(storage_provider) => Some(storage_provider),
            Err(StandardStorageProviderError::NoProvider) => None,
            Err(e) => return Err(e),
        };
        Ok(self.with_storage_provider(storage_provider))
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
    /// # use boundless_market::request_builder::PriceProviderArc;
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

    /// Modify the [OfferLayer] configuration used in the [StandardRequestBuilder].
    ///
    /// ```rust
    /// # use boundless_market::client::ClientBuilder;
    /// use alloy::primitives::utils::parse_units;
    ///
    /// ClientBuilder::new().config_offer_layer(|config| config
    ///     .max_price_per_cycle(parse_units("0.1", "gwei").unwrap())
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

    /// Modify the [Finalizer][crate::request_builder::Finalizer] configuration used in the [StandardRequestBuilder].
    pub fn config_request_finalizer(
        mut self,
        f: impl FnOnce(&mut FinalizerConfigBuilder) -> &mut FinalizerConfigBuilder,
    ) -> Self {
        f(&mut self.request_finalizer_config);
        self
    }
}

#[derive(Clone)]
#[non_exhaustive]
/// Client for interacting with the boundless market.
pub struct Client<
    P = DynProvider,
    St = StandardStorageProvider,
    R = StandardRequestBuilder,
    Si = PrivateKeySigner,
> {
    /// Boundless market service.
    pub boundless_market: BoundlessMarketService<P>,
    /// Set verifier service.
    pub set_verifier: SetVerifierService<P>,
    /// [StorageProvider] to upload programs and inputs.
    ///
    /// If not provided, this client will not be able to upload programs or inputs.
    pub storage_provider: Option<St>,
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
    StandardStorageProvider,
    StandardRequestBuilder<DynProvider>,
    PrivateKeySigner,
>;

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
/// Client error
pub enum ClientError {
    /// Storage provider error
    #[error("Storage provider error {0}")]
    StorageProviderError(#[from] StandardStorageProviderError),
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

impl Client<NotProvided, NotProvided, NotProvided, NotProvided> {
    /// Create a [ClientBuilder] to construct a [Client].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }
}

impl<P> Client<P, NotProvided, NotProvided, NotProvided>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    /// Create a new client
    pub fn new(
        boundless_market: BoundlessMarketService<P>,
        set_verifier: SetVerifierService<P>,
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
            storage_provider: None,
            offchain_client: None,
            signer: None,
            request_builder: None,
            funding_mode: FundingMode::Always,
        }
    }
}

impl<P, St, R, Si> Client<P, St, R, Si>
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

    /// Set the storage provider
    pub fn with_storage_provider(self, storage_provider: St) -> Self
    where
        St: StorageProvider,
    {
        Self { storage_provider: Some(storage_provider), ..self }
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
    pub fn with_signer<Zi>(self, signer: Zi) -> Client<P, St, R, Zi> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        Client {
            signer: Some(signer),
            boundless_market: self.boundless_market,
            set_verifier: self.set_verifier,
            storage_provider: self.storage_provider,
            offchain_client: self.offchain_client,
            request_builder: self.request_builder,
            deployment: self.deployment,
            funding_mode: self.funding_mode,
        }
    }

    /// Upload a program binary to the storage provider.
    pub async fn upload_program(&self, program: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageProvider,
        <St as StorageProvider>::Error: std::error::Error + Send + Sync + 'static,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_program(program)
            .await
            .context("Failed to upload program")?)
    }

    /// Upload input to the storage provider.
    pub async fn upload_input(&self, input: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageProvider,
        <St as StorageProvider>::Error: std::error::Error + Send + Sync + 'static,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_input(input)
            .await
            .context("Failed to upload input")?)
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
    /// Requires a a [RequestBuilder] to be provided.
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

    async fn compute_funding_value(
        &self,
        client_address: Address,
        max_price: U256,
    ) -> Result<U256, ClientError> {
        let balance = self.boundless_market.balance_of(client_address).await?;

        let value = match self.funding_mode {
            FundingMode::Always => {
                if balance > max_price.saturating_mul(U256::from(3u8)) {
                    tracing::warn!(
                        "Client balance is {} ETH, that is more than 3x the value being sent. \
                         Consider switching to a different funding mode to avoid overfunding.",
                        format_ether(balance),
                    );
                }
                max_price
            }

            FundingMode::Never => U256::ZERO,

            FundingMode::AvailableBalance => {
                if balance < max_price {
                    max_price.saturating_sub(balance)
                } else {
                    U256::ZERO
                }
            }

            FundingMode::BelowThreshold(threshold) => {
                if balance < threshold {
                    let to_send =
                        max(threshold.saturating_sub(balance), max_price.saturating_sub(balance));
                    tracing::warn!(
                        "Client balance is {} ETH < threshold {} ETH. \
                         Sending additional funds to top up the balance.",
                        format_ether(balance),
                        format_ether(threshold),
                    );
                    to_send
                } else {
                    U256::ZERO
                }
            }

            FundingMode::MinMaxBalance { min_balance, max_balance } => {
                if balance < min_balance {
                    let to_send =
                        max(max_balance.saturating_sub(balance), max_price.saturating_sub(balance));
                    tracing::warn!(
                        "Client balance is {} ETH < min {} ETH. \
                         Sending {} ETH (max target {}).",
                        format_ether(balance),
                        format_ether(min_balance),
                        format_ether(to_send),
                        format_ether(max_balance),
                    );
                    to_send
                } else {
                    U256::ZERO
                }
            }
        };

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
    ///     let (journal, receipt) = client.fetch_set_inclusion_receipt(request_id, image_id).await?;
    ///     Ok((journal, receipt))
    /// }
    /// ```
    pub async fn fetch_set_inclusion_receipt(
        &self,
        request_id: U256,
        image_id: B256,
    ) -> Result<(Bytes, SetInclusionReceipt<ReceiptClaim>), ClientError> {
        // TODO(#646): This logic is only correct under the assumption there is a single set
        // verifier.
        let fulfillment = self.boundless_market.get_request_fulfillment(request_id).await?;
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
    /// Providing a `tx_hash` will speed up onchain queries, by fetching the transaction containing
    /// the request. Providing the `request_digest` allows differentiating between multiple
    /// requests with the same ID. If set to `None`, the first found request matching the ID will
    /// be returned.
    pub async fn fetch_proof_request(
        &self,
        request_id: U256,
        tx_hash: Option<B256>,
        request_digest: Option<B256>,
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
        match self.boundless_market.get_submitted_request(request_id, tx_hash).await {
            Ok((proof_request, signature)) => Ok((proof_request, signature)),
            Err(err @ MarketError::RequestNotFound(_)) => Err(err.into()),
            err @ Err(_) => err
                .with_context(|| format!("error querying for 0x{request_id:x} onchain"))
                .map_err(Into::into),
        }
    }
}
