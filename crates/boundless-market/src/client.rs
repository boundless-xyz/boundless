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

use std::{future::Future, time::Duration};

use alloy::{
    network::{Ethereum, EthereumWallet, TxSigner},
    primitives::{Address, Bytes, U256},
    providers::{fillers::ChainIdFiller, DynProvider, Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signer},
};
use alloy_primitives::{Signature, B256};
use alloy_sol_types::SolStruct;
use anyhow::{anyhow, bail, Context, Result};
use risc0_aggregation::SetInclusionReceipt;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use url::Url;

use crate::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer},
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        ProofRequest, RequestError,
    },
    deployments::Deployment,
    dynamic_gas_filler::DynamicGasFiller,
    nonce_layer::NonceProvider,
    order_stream_client::{Order, OrderStreamClient},
    request_builder::{
        FinalizerConfigBuilder, OfferLayer, OfferLayerConfigBuilder, RequestBuilder,
        RequestIdLayer, RequestIdLayerConfigBuilder, StandardRequestBuilder,
        StandardRequestBuilderBuilderError, StorageLayer, StorageLayerConfigBuilder,
    },
    storage::{
        StandardStorageProvider, StandardStorageProviderError, StorageProvider,
        StorageProviderConfig,
    },
    util::NotProvided,
};

/// Builder for the [Client] with standard implementations for the required components.
#[derive(Clone)]
pub struct ClientBuilder<St = NotProvided, Si = NotProvided> {
    deployment: Option<Deployment>,
    rpc_url: Option<Url>,
    signer: Option<Si>,
    storage_provider: Option<St>,
    tx_timeout: Option<std::time::Duration>,
    balance_alerts: Option<BalanceAlertConfig>,
    /// Configuration builder for [OfferLayer], part of [StandardRequestBuilder].
    pub offer_layer_config: OfferLayerConfigBuilder,
    /// Configuration builder for [StorageLayer], part of [StandardRequestBuilder].
    pub storage_layer_config: StorageLayerConfigBuilder,
    /// Configuration builder for [RequestIdLayer], part of [StandardRequestBuilder].
    pub request_id_layer_config: RequestIdLayerConfigBuilder,
    /// Configuration builder for [Finalizer][crate::request_builder::Finalizer], part of [StandardRequestBuilder].
    pub request_finalizer_config: FinalizerConfigBuilder,
}

impl<St, Si> Default for ClientBuilder<St, Si> {
    fn default() -> Self {
        Self {
            deployment: None,
            rpc_url: None,
            signer: None,
            storage_provider: None,
            tx_timeout: None,
            balance_alerts: None,
            offer_layer_config: Default::default(),
            storage_layer_config: Default::default(),
            request_id_layer_config: Default::default(),
            request_finalizer_config: Default::default(),
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

    /// Build a provider connected to the given RPC URL.
    fn build_provider(
        &self,
        rpc_url: impl AsRef<str>,
    ) -> impl Future<Output = Result<DynProvider, Self::Error>>;

    /// Get the default signer address that will be used by this provider, or `None` if no signer.
    fn signer_address(&self) -> Option<Address>;
}

impl<St, Si> ClientProviderBuilder for ClientBuilder<St, Si>
where
    Si: TxSigner<Signature> + Send + Sync + Clone + 'static,
{
    type Error = anyhow::Error;

    async fn build_provider(&self, rpc_url: impl AsRef<str>) -> Result<DynProvider, Self::Error> {
        let rpc_url = rpc_url.as_ref();
        let provider = match self.signer.clone() {
            Some(signer) => {
                let dynamic_gas_filler = DynamicGasFiller::new(
                    0.2,  // 20% increase of gas limit
                    0.05, // 5% increase of gas_price per pending transaction
                    2.0,  // 2x max gas multiplier
                    signer.address(),
                );

                // Connect the RPC provider.
                let base_provider = ProviderBuilder::new()
                    .disable_recommended_fillers()
                    .filler(ChainIdFiller::default())
                    .filler(dynamic_gas_filler)
                    .layer(BalanceAlertLayer::new(self.balance_alerts.clone().unwrap_or_default()))
                    .connect(rpc_url)
                    .await
                    .with_context(|| format!("failed to connect provider to {rpc_url}"))?;
                NonceProvider::new(base_provider, EthereumWallet::from(signer)).erased()
            }
            None => ProviderBuilder::new()
                .connect(rpc_url)
                .await
                .with_context(|| format!("failed to connect provider to {rpc_url}"))?
                .erased(),
        };
        Ok(provider)
    }

    fn signer_address(&self) -> Option<Address> {
        self.signer.as_ref().map(|signer| signer.address())
    }
}

impl<St> ClientProviderBuilder for ClientBuilder<St, NotProvided> {
    type Error = anyhow::Error;

    async fn build_provider(&self, rpc_url: impl AsRef<str>) -> Result<DynProvider, Self::Error> {
        let rpc_url = rpc_url.as_ref();
        let provider = ProviderBuilder::new()
            .connect(rpc_url)
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?
            .erased();
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
        let rpc_url = self.rpc_url.clone().context("rpc_url is not set on ClientBuilder")?;
        let provider = self.build_provider(&rpc_url).await?;

        // Resolve the deployment information.
        let chain_id =
            provider.get_chain_id().await.context("failed to query chain ID from RPC provider")?;
        let deployment =
            self.deployment.clone().or_else(|| Deployment::from_chain_id(chain_id)).with_context(
                || format!("no deployment provided for unknown chain_id {chain_id}"),
            )?;

        // Check that the chain ID is matches the deployment, to avoid misconfigurations.
        if deployment.chain_id.map(|id| id != chain_id).unwrap_or(false) {
            bail!("provided deployment does not match chain_id reported by RPC provider: {chain_id} != {}", deployment.chain_id.unwrap());
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

        // Build the RequestBuilder.
        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(StorageLayer::new(
                self.storage_provider.clone(),
                self.storage_layer_config.build()?,
            ))
            .offer_layer(OfferLayer::new(provider.clone(), self.offer_layer_config.build()?))
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

    /// Set the signer from the given private key.
    pub fn with_private_key(
        self,
        private_key: impl Into<PrivateKeySigner>,
    ) -> ClientBuilder<St, PrivateKeySigner> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            signer: Some(private_key.into()),
            deployment: self.deployment,
            storage_provider: self.storage_provider,
            rpc_url: self.rpc_url,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            offer_layer_config: self.offer_layer_config,
            storage_layer_config: self.storage_layer_config,
            request_id_layer_config: self.request_id_layer_config,
            request_finalizer_config: self.request_finalizer_config,
        }
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
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            offer_layer_config: self.offer_layer_config,
            storage_layer_config: self.storage_layer_config,
            request_id_layer_config: self.request_id_layer_config,
            request_finalizer_config: self.request_finalizer_config,
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
            signer: self.signer,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            request_finalizer_config: self.request_finalizer_config,
            request_id_layer_config: self.request_id_layer_config,
            storage_layer_config: self.storage_layer_config,
            offer_layer_config: self.offer_layer_config,
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
                chain_id: None,
                order_stream_url: None,
                stake_token_address: None,
                verifier_router_address: None,
            },
            boundless_market,
            set_verifier,
            storage_provider: None,
            offchain_client: None,
            signer: None,
            request_builder: None,
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
        }
    }

    /// Upload a program binary to the storage provider.
    pub async fn upload_program(&self, program: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageProvider,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_program(program)
            .await
            .map_err(|_| anyhow!("Failed to upload program"))?)
    }

    /// Upload input to the storage provider.
    pub async fn upload_input(&self, input: &[u8]) -> Result<Url, ClientError>
    where
        St: StorageProvider,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_input(input)
            .await
            .map_err(|_| anyhow!("Failed to upload input"))?)
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

        let request_id = self.boundless_market.submit_request(&request, signer).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Submit a pre-signed proof in an onchain transaction.
    ///
    /// Accepts a signature bytes to be used as the request signature.
    pub async fn submit_request_onchain_with_signature(
        &self,
        request: &ProofRequest,
        signature: &Bytes,
    ) -> Result<(U256, u64), ClientError> {
        let request = request.clone();
        request.validate()?;

        let request_id =
            self.boundless_market.submit_request_with_signature(&request, signature).await?;
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
        // Ensure address' balance is sufficient to cover the request
        let balance = self.boundless_market.balance_of(client_address).await?;
        if balance < U256::from(request.offer.maxPrice) {
            return Err(ClientError::Error(anyhow!(
                "Insufficient balance to cover request: {} < {}.\nMake sure to top up your balance by depositing on the Boundless Market.",
                balance,
                request.offer.maxPrice
            )));
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
    ) -> Result<(Bytes, Bytes), ClientError> {
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
        let (journal, seal) = self.boundless_market.get_request_fulfillment(request_id).await?;
        let claim = ReceiptClaim::ok(Digest::from(image_id.0), journal.to_vec());
        let receipt =
            self.set_verifier.fetch_receipt_with_claim(seal, claim, journal.to_vec()).await?;
        Ok((journal, receipt))
    }

    /// Fetch an order as a proof request and signature pair.
    ///
    /// If the request is not found in the boundless market, it will be fetched from the order stream service.
    pub async fn fetch_order(
        &self,
        request_id: U256,
        tx_hash: Option<B256>,
        request_digest: Option<B256>,
    ) -> Result<Order, ClientError> {
        match self.boundless_market.get_submitted_request(request_id, tx_hash).await {
            Ok((request, signature_bytes)) => {
                let domain = self.boundless_market.eip712_domain().await?;
                let digest = request.eip712_signing_hash(&domain.alloy_struct());
                if let Some(expected_digest) = request_digest {
                    if digest != expected_digest {
                        return Err(ClientError::RequestError(RequestError::DigestMismatch));
                    }
                }
                Ok(Order {
                    request,
                    request_digest: digest,
                    signature: Signature::try_from(signature_bytes.as_ref())
                        .map_err(|_| ClientError::Error(anyhow!("Failed to parse signature")))?,
                })
            }
            Err(_) => Ok(self
                .offchain_client
                .as_ref()
                .context("Request not found on-chain and order stream client not available. Please provide an order stream URL")?
                .fetch_order(request_id, request_digest)
                .await?),
        }
    }
}
