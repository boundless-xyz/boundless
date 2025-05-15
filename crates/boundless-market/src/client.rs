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

use std::{env, str::FromStr, time::Duration};

use alloy::{
    network::{Ethereum, EthereumWallet},
    primitives::{Address, Bytes, U256},
    providers::{Provider, ProviderBuilder},
    signers::{local::PrivateKeySigner, Signer},
};
use alloy_primitives::{Signature, B256};
use alloy_sol_types::SolStruct;
use anyhow::{anyhow, Context, Result};
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
    order_stream_client::{Client as OrderStreamClient, Order},
    request_builder::{
        FinalizerConfigBuilder, OfferLayer, OfferLayerConfigBuilder, RequestBuilder,
        RequestIdLayer, RequestIdLayerConfigBuilder, StandardRequestBuilder,
        StandardRequestBuilderBuilderError, StorageLayer, StorageLayerConfigBuilder,
    },
    storage::{
        storage_provider_from_env, StandardStorageProvider, StandardStorageProviderError,
        StorageProvider, StorageProviderConfig,
    },
    util::{NotProvided, StandardRpcProvider},
};

/// Builder for the client
// TODO: Improve this docstring.
#[derive(Clone)]
pub struct ClientBuilder<St = NotProvided, Si = NotProvided> {
    boundless_market_address: Option<Address>,
    set_verifier_address: Option<Address>,
    rpc_url: Option<Url>,
    wallet: Option<EthereumWallet>,
    signer: Option<Si>,
    order_stream_url: Option<Url>,
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
            boundless_market_address: None,
            set_verifier_address: None,
            rpc_url: None,
            wallet: None,
            signer: None,
            order_stream_url: None,
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

impl<St, Si> ClientBuilder<St, Si> {
    /// Build the client
    pub async fn build(
        self,
    ) -> Result<Client<StandardRpcProvider, St, StandardRequestBuilder<StandardRpcProvider, St>, Si>>
    where
        St: Clone,
        Si: Clone,
    {
        let wallet = self.wallet.context("wallet is not set on ClientBuilder")?;
        let rpc_url = self.rpc_url.context("rpc_url is not set on ClientBuilder")?;
        let boundless_market_address = self
            .boundless_market_address
            .context("boundless_market_address is not set on ClientBuilder")?;
        let set_verifier_address = self
            .set_verifier_address
            .context("set_verifier_address is not set on ClientBuilder")?;

        let caller = wallet.default_signer().address();

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .layer(BalanceAlertLayer::new(self.balance_alerts.unwrap_or_default()))
            .connect(rpc_url.as_str())
            .await
            .with_context(|| format!("failed to connect provider to {rpc_url}"))?;

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let set_verifier = SetVerifierService::new(set_verifier_address, provider.clone(), caller);

        let chain_id = provider.get_chain_id().await.context("failed to get chain ID")?;
        let offchain_client = self
            .order_stream_url
            .map(|url| OrderStreamClient::new(url, boundless_market_address, chain_id));

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
        };

        if let Some(timeout) = self.tx_timeout {
            client = client.with_timeout(timeout);
        }

        Ok(client)
    }

    /// Set the Boundless market address
    pub fn with_boundless_market_address(self, boundless_market_addr: Address) -> Self {
        Self { boundless_market_address: Some(boundless_market_addr), ..self }
    }

    /// Set the set verifier address
    pub fn with_set_verifier_address(self, set_verifier_addr: Address) -> Self {
        Self { set_verifier_address: Some(set_verifier_addr), ..self }
    }

    /// Set the RPC URL
    pub fn with_rpc_url(self, rpc_url: Url) -> Self {
        Self { rpc_url: Some(rpc_url), ..self }
    }

    /// Set the private key
    pub fn with_private_key(
        self,
        private_key: PrivateKeySigner,
    ) -> ClientBuilder<St, PrivateKeySigner> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            wallet: Some(EthereumWallet::from(private_key.clone())),
            signer: Some(private_key),
            storage_provider: self.storage_provider,
            rpc_url: self.rpc_url,
            order_stream_url: self.order_stream_url,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            set_verifier_address: self.set_verifier_address,
            boundless_market_address: self.boundless_market_address,
            offer_layer_config: self.offer_layer_config,
            storage_layer_config: self.storage_layer_config,
            request_id_layer_config: self.request_id_layer_config,
            request_finalizer_config: self.request_finalizer_config,
        }
    }

    /// Set the wallet
    pub fn with_wallet(self, wallet: EthereumWallet) -> Self {
        Self { wallet: Some(wallet), ..self }
    }

    /// Set the order stream URL
    pub fn with_order_stream_url(self, order_stream_url: Option<Url>) -> Self {
        Self { order_stream_url, ..self }
    }

    /// Set the transaction timeout in seconds
    pub fn with_timeout(self, tx_timeout: Option<Duration>) -> Self {
        Self { tx_timeout, ..self }
    }

    /// Set the balance alerts configuration
    pub fn with_balance_alerts(self, config: BalanceAlertConfig) -> Self {
        Self { balance_alerts: Some(config), ..self }
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
            boundless_market_address: self.boundless_market_address,
            set_verifier_address: self.set_verifier_address,
            rpc_url: self.rpc_url,
            wallet: self.wallet,
            signer: self.signer,
            order_stream_url: self.order_stream_url,
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
}

#[derive(Clone)]
#[non_exhaustive]
/// Client for interacting with the boundless market.
pub struct Client<
    P = StandardRpcProvider,
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
}

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
        Self { boundless_market, ..self }
    }

    /// Set the set verifier service
    pub fn with_set_verifier(self, set_verifier: SetVerifierService<P>) -> Self {
        Self { set_verifier, ..self }
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
        Self { offchain_client: Some(offchain_client), ..self }
    }

    /// Set the transaction timeout
    pub fn with_timeout(self, tx_timeout: std::time::Duration) -> Self {
        Self {
            boundless_market: self.boundless_market.with_timeout(tx_timeout),
            set_verifier: self.set_verifier.with_timeout(tx_timeout),
            ..self
        }
    }

    /// Set the signer
    // TODO: Add an example of providing a local signer.
    pub fn with_signer<Zi>(self, signer: Zi) -> Client<P, St, R, Zi> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        Client {
            signer: Some(signer),
            boundless_market: self.boundless_market,
            set_verifier: self.set_verifier,
            storage_provider: self.storage_provider,
            offchain_client: self.offchain_client,
            request_builder: self.request_builder,
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
    pub fn request_params<Params>(&self) -> Params
    where
        R: RequestBuilder<Params>,
        Params: Default,
    {
        Params::default()
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
        let request_builder =
            self.request_builder.as_ref().context("request_builder is not set on Client")?;
        let request = request_builder.build(params).await.map_err(Into::into)?;
        self.submit_request_onchain_with_signer(&request, signer).await
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
        let request_builder =
            self.request_builder.as_ref().context("request_builder is not set on Client")?;
        let request = request_builder.build(params).await.map_err(Into::into)?;
        self.submit_request_offchain_with_signer(&request, signer).await
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
    /// Example:
    /// ```
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
    ///
    pub async fn fetch_set_inclusion_receipt(
        &self,
        request_id: U256,
        image_id: B256,
    ) -> Result<(Bytes, SetInclusionReceipt<ReceiptClaim>), ClientError> {
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

/// Alias for a [Client] instantiated with the standard implementations provided by this crate.
pub type StandardClient = Client<
    StandardRpcProvider,
    StandardStorageProvider,
    StandardRequestBuilder<StandardRpcProvider>,
    PrivateKeySigner,
>;

impl StandardClient {
    /// Create a new client from environment variables
    ///
    /// The following environment variables are required:
    /// - PRIVATE_KEY: The private key of the wallet
    /// - RPC_URL: The URL of the RPC server
    /// - ORDER_STREAM_URL: The URL of the order stream server
    /// - BOUNDLESS_MARKET_ADDRESS: The address of the market contract
    /// - SET_VERIFIER_ADDRESS: The address of the set verifier contract
    pub async fn from_env() -> Result<Self, ClientError> {
        let private_key_str = env::var("private_key").context("private_key not set")?;
        let private_key =
            PrivateKeySigner::from_str(&private_key_str).context("Invalid private_key")?;
        let rpc_url_str = env::var("RPC_URL").context("RPC_URL not set")?;
        let rpc_url = Url::parse(&rpc_url_str).context("Invalid RPC_URL")?;
        let boundless_market_address_str =
            env::var("BOUNDLESS_MARKET_ADDRESS").context("BOUNDLESS_MARKET_ADDRESS not set")?;
        let boundless_market_address = Address::from_str(&boundless_market_address_str)
            .context("Invalid BOUNDLESS_MARKET_ADDRESS")?;
        let set_verifier_address_str =
            env::var("SET_VERIFIER_ADDRESS").context("SET_VERIFIER_ADDRESS not set")?;
        let set_verifier_address =
            Address::from_str(&set_verifier_address_str).context("Invalid SET_VERIFIER_ADDRESS")?;

        let caller = private_key.address();
        let wallet = EthereumWallet::from(private_key.clone());
        let provider = ProviderBuilder::new()
            .wallet(wallet.clone())
            .layer(BalanceAlertLayer::default())
            .connect_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let set_verifier = SetVerifierService::new(set_verifier_address, provider.clone(), caller);

        let storage_provider = match storage_provider_from_env() {
            Ok(provider) => Some(provider),
            Err(_) => None,
        };

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;

        let order_stream_url = env::var("ORDER_STREAM_URL");
        let offchain_client = match order_stream_url {
            Ok(url) => Some(OrderStreamClient::new(
                Url::parse(&url).context("Invalid ORDER_STREAM_URL")?,
                boundless_market_address,
                chain_id,
            )),
            Err(_) => None,
        };

        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(storage_provider.clone())
            .offer_layer(provider.clone())
            .request_id_layer(boundless_market.clone())
            .build()?;

        Ok(Self {
            boundless_market,
            set_verifier,
            storage_provider,
            offchain_client,
            signer: Some(private_key),
            request_builder: Some(request_builder),
        })
    }
}
