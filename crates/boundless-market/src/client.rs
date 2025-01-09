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
    transports::{http::Http, Transport},
};
use anyhow::{anyhow, Context, Result};
use reqwest::Client as HttpClient;
use url::Url;

use crate::{
    contracts::{
        boundless_market::{BoundlessMarketService, MarketError},
        set_verifier::SetVerifierService,
        ProofRequest, RequestError,
    },
    order_stream_client::Client as OrderStreamClient,
    storage::{
        storage_provider_from_config, storage_provider_from_env, BuiltinStorageProvider,
        BuiltinStorageProviderError, StorageProvider, StorageProviderConfig,
    },
};

// Default bidding start offset (from the current block) in blocks
const BIDDING_START_OFFSET: u64 = 5;

type ProviderWallet = FillProvider<
    JoinFill<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider<Http<HttpClient>>,
    Http<HttpClient>,
    Ethereum,
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
pub struct ClientBuilder {
    boundless_market_addr: Option<Address>,
    set_verifier_addr: Option<Address>,
    rpc_url: Option<Url>,
    wallet: Option<EthereumWallet>,
    local_signer: Option<PrivateKeySigner>,
    order_stream_url: Option<Url>,
    storage_config: Option<StorageProviderConfig>,
    tx_timeout: Option<std::time::Duration>,
    bidding_start_offset: u64,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            boundless_market_addr: None,
            set_verifier_addr: None,
            rpc_url: None,
            wallet: None,
            local_signer: None,
            order_stream_url: None,
            storage_config: None,
            tx_timeout: None,
            bidding_start_offset: BIDDING_START_OFFSET,
        }
    }
}

impl ClientBuilder {
    /// Create a new client builder
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the client
    pub async fn build(
        self,
    ) -> Result<Client<Http<HttpClient>, ProviderWallet, BuiltinStorageProvider>> {
        let mut client = Client::from_parts(
            self.wallet.context("Wallet not set")?,
            self.rpc_url.context("RPC URL not set")?,
            self.boundless_market_addr.context("Boundless market address not set")?,
            self.set_verifier_addr.context("Set verifier address not set")?,
            self.order_stream_url,
            if let Some(storage_config) = self.storage_config {
                Some(storage_provider_from_config(&storage_config).await?)
            } else {
                None
            },
        )
        .await?;
        if let Some(timeout) = self.tx_timeout {
            client = client.with_timeout(timeout);
        }
        if let Some(local_signer) = self.local_signer {
            client = client.with_local_signer(local_signer);
        }
        client = client.with_bidding_start_offset(self.bidding_start_offset);
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

    /// Set the storage provider config
    pub fn with_storage_provider_config(
        self,
        storage_config: Option<StorageProviderConfig>,
    ) -> Self {
        Self { storage_config, ..self }
    }

    /// Set the transaction timeout in seconds
    pub fn with_timeout(self, tx_timeout: Option<Duration>) -> Self {
        Self { tx_timeout, ..self }
    }

    /// Set the bidding start offset in blocks
    pub fn with_bidding_start_offset(self, bidding_start_offset: u64) -> Self {
        Self { bidding_start_offset, ..self }
    }
}

#[derive(Clone)]
/// Client for interacting with the boundless market.
pub struct Client<T, P, S> {
    /// Boundless market service.
    pub boundless_market: BoundlessMarketService<T, P>,
    /// Set verifier service.
    pub set_verifier: SetVerifierService<T, P>,
    /// Storage provider to upload ELFs and inputs.
    pub storage_provider: Option<S>,
    /// Order stream client to submit requests off-chain.
    pub offchain_client: Option<OrderStreamClient>,
    /// Local signer for signing requests.
    pub local_signer: Option<LocalSigner<SigningKey>>,
    /// Bidding start offset wrt the current block (in blocks).
    pub bidding_start_offset: u64,
}

impl<T, P, S> Client<T, P, S>
where
    T: Transport + Clone,
    P: Provider<T, Ethereum> + 'static + Clone,
    S: StorageProvider + Clone,
{
    /// Create a new client
    pub fn new(
        boundless_market: BoundlessMarketService<T, P>,
        set_verifier: SetVerifierService<T, P>,
    ) -> Self {
        let boundless_market = boundless_market.clone();
        let set_verifier = set_verifier.clone();
        Self {
            boundless_market,
            set_verifier,
            storage_provider: None,
            offchain_client: None,
            local_signer: None,
            bidding_start_offset: BIDDING_START_OFFSET,
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
    pub fn with_boundless_market(self, boundless_market: BoundlessMarketService<T, P>) -> Self {
        Self { boundless_market, ..self }
    }

    /// Set the set verifier service
    pub fn with_set_verifier(self, set_verifier: SetVerifierService<T, P>) -> Self {
        Self { set_verifier, ..self }
    }

    /// Set the storage provider
    pub fn with_storage_provider(self, storage_provider: S) -> Self {
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

    /// Set the bidding start offset
    pub fn with_bidding_start_offset(self, bidding_start_offset: u64) -> Self {
        Self { bidding_start_offset, ..self }
    }

    /// Upload an image to the storage provider
    pub async fn upload_image(&self, elf: &[u8]) -> Result<String, ClientError> {
        Ok(self
            .storage_provider
            .as_ref()
            .context("Storage provider not set")?
            .upload_image(elf)
            .await
            .map_err(|_| anyhow!("Failed to upload image"))?)
    }

    /// Upload input to the storage provider
    pub async fn upload_input(&self, input: &[u8]) -> Result<String, ClientError> {
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
    /// If the bidding start is not set, the current block number will be used.
    pub async fn submit_request(&self, request: &ProofRequest) -> Result<(U256, u64), ClientError>
    where
        <S as StorageProvider>::Error: std::fmt::Debug,
    {
        let signer = self.local_signer.as_ref().context("Local signer not set")?;
        self.submit_request_with_signer(request, signer).await
    }

    /// Submit a proof request.
    ///
    /// Accepts a signer to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current block number will be used.
    pub async fn submit_request_with_signer(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<(U256, u64), ClientError>
    where
        <S as StorageProvider>::Error: std::fmt::Debug,
    {
        let mut request = request.clone();

        if request.id == U256::ZERO {
            request.id = self.boundless_market.request_id_from_rand().await?;
        };
        let client_address = request.client_address()?;
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()))?;
        };
        if request.offer.biddingStart == 0 {
            request.offer.biddingStart = self
                .provider()
                .get_block_number()
                .await
                .context("Failed to get current block number")?
                + self.bidding_start_offset
        };

        request.validate()?;

        let request_id = self.boundless_market.submit_request(&request, signer).await?;
        Ok((request_id, request.expires_at()))
    }

    /// Submit a proof request offchain via the order stream service.
    ///
    /// Accepts a signer to sign the request.
    /// If the request ID is not set, a random ID will be generated.
    /// If the bidding start is not set, the current block number will be used.
    pub async fn submit_request_offchain_with_signer(
        &self,
        request: &ProofRequest,
        signer: &impl Signer,
    ) -> Result<(U256, u64), ClientError>
    where
        <S as StorageProvider>::Error: std::fmt::Debug,
    {
        let offchain_client = self
            .offchain_client
            .as_ref()
            .context("Order stream client not available. Please provide an order stream URL")?;
        let mut request = request.clone();

        if request.id == U256::ZERO {
            request.id = self.boundless_market.request_id_from_rand().await?;
        };
        let client_address = request.client_address()?;
        if client_address != signer.address() {
            return Err(MarketError::AddressMismatch(client_address, signer.address()))?;
        };
        if request.offer.biddingStart == 0 {
            request.offer.biddingStart = self
                .provider()
                .get_block_number()
                .await
                .context("Failed to get current block number")?
                + self.bidding_start_offset
        };
        // Ensure address' balance is sufficient to cover the request
        let balance = self.boundless_market.balance_of(request.client_address()?).await?;
        if balance < U256::from(request.offer.maxPrice) {
            return Err(ClientError::Error(anyhow!(
                "Insufficient balance to cover request: {} < {}",
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
    /// If the bidding start is not set, the current block number will be used.
    pub async fn submit_request_offchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), ClientError>
    where
        <S as StorageProvider>::Error: std::fmt::Debug,
    {
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
}

impl Client<Http<HttpClient>, ProviderWallet, BuiltinStorageProvider> {
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
            .with_recommended_fillers()
            .wallet(wallet.clone())
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let set_verifier = SetVerifierService::new(set_verifier_address, provider.clone(), caller);

        let storage_provider = match storage_provider_from_env().await {
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
            bidding_start_offset: BIDDING_START_OFFSET,
        })
    }

    /// Create a new client from parts
    pub async fn from_parts(
        wallet: EthereumWallet,
        rpc_url: Url,
        boundless_market_address: Address,
        set_verifier_address: Address,
        order_stream_url: Option<Url>,
        storage_provider: Option<BuiltinStorageProvider>,
    ) -> Result<Self, ClientError> {
        let caller = wallet.default_signer().address();

        let provider = ProviderBuilder::new()
            .with_recommended_fillers()
            .wallet(wallet.clone())
            .on_http(rpc_url);

        let boundless_market =
            BoundlessMarketService::new(boundless_market_address, provider.clone(), caller);
        let set_verifier = SetVerifierService::new(set_verifier_address, provider.clone(), caller);

        let chain_id = provider.get_chain_id().await.context("Failed to get chain ID")?;
        let offchain_client = if let Some(url) = order_stream_url {
            Some(OrderStreamClient::new(url, boundless_market_address, chain_id))
        } else {
            None
        };

        Ok(Self {
            boundless_market,
            set_verifier,
            storage_provider,
            offchain_client,
            local_signer: None,
            bidding_start_offset: BIDDING_START_OFFSET,
        })
    }
}
