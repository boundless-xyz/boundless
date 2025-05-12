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
    providers::{
        fillers::{
            BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            WalletFiller,
        },
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    signers::{
        k256::ecdsa::SigningKey,
        local::{LocalSigner, PrivateKeySigner},
        Signer,
    },
};
use alloy_primitives::{PrimitiveSignature, B256};
use alloy_sol_types::SolStruct;
use anyhow::{anyhow, Context, Result};
use risc0_aggregation::SetInclusionReceipt;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use url::Url;

use crate::{
    balance_alerts_layer::{BalanceAlertConfig, BalanceAlertLayer, BalanceAlertProvider},
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        ProofRequest, RequestError,
    },
    now_timestamp,
    order_stream_client::{Client as OrderStreamClient, Order},
    storage::{
        storage_provider_from_env, BuiltinStorageProvider, BuiltinStorageProviderError,
        StorageProvider, StorageProviderConfig,
    },
};

// Default bidding start delay (from the current time) in seconds
const BIDDING_START_DELAY: u64 = 30;

type ProviderWallet = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    BalanceAlertProvider<RootProvider>,
>;

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
/// Client error
pub enum ClientError {
    /// Storage provider error
    #[error("Storage provider error {0}")]
    StorageProviderError(#[from] BuiltinStorageProviderError),
    /// Market error
    #[error("Market error {0}")]
    MarketError(#[from] MarketError),
    /// Request error
    #[error("RequestError {0}")]
    RequestError(#[from] RequestError),
    /// General error
    #[error("Error {0}")]
    Error(#[from] anyhow::Error),
}

/// Builder for the client
pub struct ClientBuilder<P = BuiltinStorageProvider> {
    boundless_market_addr: Option<Address>,
    set_verifier_addr: Option<Address>,
    rpc_url: Option<Url>,
    wallet: Option<EthereumWallet>,
    local_signer: Option<PrivateKeySigner>,
    order_stream_url: Option<Url>,
    storage_provider: Option<P>,
    tx_timeout: Option<std::time::Duration>,
    bidding_start_delay: u64,
    balance_alerts: Option<BalanceAlertConfig>,
}

impl<P> Default for ClientBuilder<P> {
    /// Creates a new `ClientBuilder` with all configuration options set to their default values.
    fn default() -> Self {
        Self {
            boundless_market_addr: None,
            set_verifier_addr: None,
            rpc_url: None,
            wallet: None,
            local_signer: None,
            order_stream_url: None,
            storage_provider: None,
            tx_timeout: None,
            bidding_start_delay: BIDDING_START_DELAY,
            balance_alerts: None,
        }
    }
}

impl ClientBuilder<BuiltinStorageProvider> {
    /// Create a new client builder.
    ///
    /// Equivalent to [Default] when using [BuiltinStorageProvider].
    pub fn new() -> Self {
        Self::default()
    }
}

impl<P> ClientBuilder<P> {
    /// Build the client
    pub async fn build(self) -> Result<Client<ProviderWallet, P>> {
        let mut client = Client::from_parts(
            self.wallet.context("Wallet not set")?,
            self.rpc_url.context("RPC URL not set")?,
            self.boundless_market_addr.context("Boundless market address not set")?,
            self.set_verifier_addr.context("Set verifier address not set")?,
            self.order_stream_url,
            self.storage_provider,
            self.balance_alerts,
        )
        .await?;
        if let Some(timeout) = self.tx_timeout {
            client = client.with_timeout(timeout);
        }
        if let Some(local_signer) = self.local_signer {
            client = client.with_local_signer(local_signer);
        }
        client = client.with_bidding_start_delay(self.bidding_start_delay);
        Ok(client)
    }

    /// Set the Boundless market address
    pub fn with_boundless_market_address(self, boundless_market_addr: Address) -> Self {
        Self { boundless_market_addr: Some(boundless_market_addr), ..self }
    }

    /// Set the set verifier address
    pub fn with_set_verifier_address(self, set_verifier_addr: Address) -> Self {
        Self { set_verifier_addr: Some(set_verifier_addr), ..self }
    }

    /// Set the RPC URL
    pub fn with_rpc_url(self, rpc_url: Url) -> Self {
        Self { rpc_url: Some(rpc_url), ..self }
    }

    /// Set the private key
    pub fn with_private_key(self, private_key: PrivateKeySigner) -> Self {
        Self {
            wallet: Some(EthereumWallet::from(private_key.clone())),
            local_signer: Some(private_key),
            ..self
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

    /// Set the bidding start delay in seconds, from the current time.
    ///
    /// Used to set the bidding start time on requests, when a start time is not specified.
    pub fn with_bidding_start_delay(self, bidding_start_delay: u64) -> Self {
        Self { bidding_start_delay, ..self }
    }

    /// Set the balance alerts configuration
    pub fn with_balance_alerts(self, config: BalanceAlertConfig) -> Self {
        Self { balance_alerts: Some(config), ..self }
    }

    /// Set the storage provider.
    ///
    /// The returned [ClientBuilder] will be generic over the provider [StorageProvider] type.
    pub fn with_storage_provider<S: StorageProvider>(
        self,
        storage_provider: Option<S>,
    ) -> ClientBuilder<S> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            storage_provider,
            boundless_market_addr: self.boundless_market_addr,
            set_verifier_addr: self.set_verifier_addr,
            rpc_url: self.rpc_url,
            wallet: self.wallet,
            local_signer: self.local_signer,
            order_stream_url: self.order_stream_url,
            tx_timeout: self.tx_timeout,
            balance_alerts: self.balance_alerts,
            bidding_start_delay: self.bidding_start_delay,
        }
    }

    /// Set the storage provider from the given config
    pub async fn with_storage_provider_config(
        self,
        config: &StorageProviderConfig,
    ) -> Result<ClientBuilder<BuiltinStorageProvider>, BuiltinStorageProviderError> {
        let storage_provider = match BuiltinStorageProvider::from_config(config) {
            Ok(storage_provider) => Some(storage_provider),
            Err(BuiltinStorageProviderError::NoProvider) => None,
            Err(e) => return Err(e),
        };
        Ok(self.with_storage_provider(storage_provider))
    }
}

#[derive(Clone)]
#[non_exhaustive]
/// Client for interacting with the boundless market.
pub struct Client<P, S> {
    /// Boundless market service.
    pub boundless_market: BoundlessMarketService<P>,
    /// Set verifier service.
    pub set_verifier: SetVerifierService<P>,
    /// Storage provider to upload programs and inputs.
    pub storage_provider: Option<S>,
    /// Order stream client to submit requests off-chain.
    pub offchain_client: Option<OrderStreamClient>,
    /// Local signer for signing requests.
    pub local_signer: Option<LocalSigner<SigningKey>>,
    /// Bidding start delay with regard to the current time, in seconds.
    pub bidding_start_delay: u64,
}

impl<P, S> Client<P, S>
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
            local_signer: None,
            bidding_start_delay: BIDDING_START_DELAY,
        }
    }

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
    pub fn with_storage_provider(self, storage_provider: S) -> Self
    where
        S: StorageProvider,
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

    /// Set the local signer
    pub fn with_local_signer(self, local_signer: LocalSigner<SigningKey>) -> Self {
        Self { local_signer: Some(local_signer), ..self }
    }

    /// Set the bidding start delay, in seconds.
    pub fn with_bidding_start_delay(self, bidding_start_delay: u64) -> Self {
        Self { bidding_start_delay, ..self }
    }

    /// Upload a program binary to the storage provider
    pub async fn upload_program(&self, program: &[u8]) -> Result<Url, ClientError>
    where
        S: StorageProvider,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_program(program)
            .await
            .map_err(|_| anyhow!("Failed to upload program"))?)
    }

    /// Upload input to the storage provider
    pub async fn upload_input(&self, input: &[u8]) -> Result<Url, ClientError>
    where
        S: StorageProvider,
    {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_input(input)
            .await
            .map_err(|_| anyhow!("Failed to upload input"))?)
    }

    /// Submit a proof request.
    ///
    /// Requires a local signer to be set to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current time will be used, plus a delay.
    pub async fn submit_request(&self, request: &ProofRequest) -> Result<(U256, u64), ClientError> {
        let signer = self.local_signer.as_ref().context("Local signer not set")?;
        self.submit_request_with_signer(request, signer).await
    }

    /// Submit a proof request.
    ///
    /// Accepts a signer to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current time will be used, plus a delay.
    pub async fn submit_request_with_signer(
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
        if request.offer.biddingStart == 0 {
            request.offer.biddingStart = now_timestamp() + self.bidding_start_delay
        };

        request.validate()?;

        let request_id = self.boundless_market.submit_request(&request, signer).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Submit a proof request with a signature bytes.
    ///
    /// Accepts a signature bytes to be used as the request signature.
    pub async fn submit_request_with_signature_bytes(
        &self,
        request: &ProofRequest,
        signature: &Bytes,
    ) -> Result<(U256, u64), ClientError> {
        let request = request.clone();
        request.validate()?;

        let request_id =
            self.boundless_market.submit_request_with_signature_bytes(&request, signature).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Submit a proof request offchain via the order stream service.
    ///
    /// Accepts a signer to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current time plus a delay will be used.
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
        if request.offer.biddingStart == 0 {
            request.offer.biddingStart = now_timestamp() + self.bidding_start_delay
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

    /// Submit a proof request offchain via the order stream service.
    ///
    /// Requires a local signer to be set to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current timestamp plus a delay will be used.
    pub async fn submit_request_offchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), ClientError> {
        let signer = self.local_signer.as_ref().context("Local signer not set")?;
        self.submit_request_offchain_with_signer(request, signer).await
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
                    signature: PrimitiveSignature::try_from(signature_bytes.as_ref())
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

impl Client<ProviderWallet, BuiltinStorageProvider> {
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
            .on_http(rpc_url);

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

        Ok(Self {
            boundless_market,
            set_verifier,
            storage_provider,
            offchain_client,
            local_signer: Some(private_key),
            bidding_start_delay: BIDDING_START_DELAY,
        })
    }
}

impl<P> Client<ProviderWallet, P> {
    /// Create a new client from parts
    pub async fn from_parts(
        wallet: EthereumWallet,
        rpc_url: Url,
        boundless_market_address: Address,
        set_verifier_address: Address,
        order_stream_url: Option<Url>,
        storage_provider: Option<P>,
        balance_alerts: Option<BalanceAlertConfig>,
    ) -> Result<Self, ClientError> {
        let caller = wallet.default_signer().address();

        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .layer(BalanceAlertLayer::new(balance_alerts.unwrap_or_default()))
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let set_verifier = SetVerifierService::new(set_verifier_address, provider.clone(), caller);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let offchain_client = order_stream_url
            .map(|url| OrderStreamClient::new(url, boundless_market_address, chain_id));

        Ok(Self {
            boundless_market,
            set_verifier,
            storage_provider,
            offchain_client,
            local_signer: None,
            bidding_start_delay: BIDDING_START_DELAY,
        })
    }
}
