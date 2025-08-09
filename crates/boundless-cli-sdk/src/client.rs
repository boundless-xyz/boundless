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

use std::io::Write;
use std::process::Command;

use alloy_primitives::{
    utils::{format_ether, parse_ether, parse_units},
    Address, Bytes, U256,
};
use anyhow::{anyhow, bail, Context, Error, Ok, Result};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;
use url::Url;

use crate::{
    contracts::ProofRequest,
    deployments::Deployment,
    request_builder::{RequestIdLayer, StandardRequestBuilder},
    storage::{StandardStorageProvider, StandardStorageProviderError, StorageProviderConfig},
    util::NotProvided,
};
use crate::{
    request_builder::{
        FinalizerConfigBuilder, OfferLayerConfigBuilder, RequestBuilder,
        RequestIdLayerConfigBuilder, StorageLayerConfigBuilder,
    },
    rpc::RpcProvider,
};
use crate::{
    request_builder::{OfferLayer, StorageLayer},
    storage::StorageProvider,
};

/// Status of a proof request
#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq)]
pub enum RequestStatus {
    /// The request has expired.
    Expired,
    /// The request is locked in and waiting for fulfillment.
    Locked,
    /// The request has been fulfilled.
    Fulfilled,
    /// The request has an unknown status.
    ///
    /// This is used to represent the status of a request
    /// with no evidence in the state. The request may be
    /// open for bidding or it may not exist.
    #[default]
    Unknown,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "command", rename_all = "kebab-case")]
enum Output {
    Ok,
    AccountAmount { amount_eth: String },
    AccountStakeAmount { amount: String, decimals: u8, symbol: String },
    RequestStatus { status: RequestStatus },
    RequestSubmitted { request_id: U256, expires_at: u64 },
    RequestFulfilled { journal: Bytes, seal: Bytes },
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CliResponse<T: Serialize> {
    pub(crate) success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
}

/// Builder for the [Client] with standard implementations for the required components.
#[derive(Clone)]
pub struct ClientBuilder<S = NotProvided> {
    deployment: Option<Deployment>,
    rpc_url: Option<Url>,
    caller: Option<Address>,
    private_key: Option<String>,
    storage_provider: Option<S>,
    /// Configuration builder for [OfferLayer], part of [StandardRequestBuilder].
    pub offer_layer_config: OfferLayerConfigBuilder,
    /// Configuration builder for [StorageLayer], part of [StandardRequestBuilder].
    pub storage_layer_config: StorageLayerConfigBuilder,
    /// Configuration builder for [RequestIdLayer], part of [StandardRequestBuilder].
    pub request_id_layer_config: RequestIdLayerConfigBuilder,
    /// Configuration builder for [Finalizer][crate::request_builder::Finalizer], part of [StandardRequestBuilder].
    pub request_finalizer_config: FinalizerConfigBuilder,
}

impl<S> Default for ClientBuilder<S> {
    fn default() -> Self {
        Self {
            deployment: None,
            rpc_url: None,
            caller: None,
            private_key: None,
            storage_provider: None,
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

impl<S> ClientBuilder<S> {
    /// Build the client
    pub async fn build(self) -> Result<Client<S, StandardRequestBuilder<S>>>
    where
        S: Clone,
    {
        let rpc_url = self.rpc_url.clone().context("rpc_url is not set on ClientBuilder")?;
        let caller = self.caller.context("caller is not set on ClientBuilder")?;
        let provider = RpcProvider::new(rpc_url.clone(), caller);

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

        // Build the RequestBuilder.
        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(StorageLayer::new(
                self.storage_provider.clone(),
                self.storage_layer_config.build()?,
            ))
            .offer_layer(OfferLayer::new(provider.clone(), self.offer_layer_config.build()?))
            .request_id_layer(RequestIdLayer::new(
                provider.clone(),
                self.request_id_layer_config.build()?,
            ))
            .finalizer(self.request_finalizer_config.build()?)
            .build()?;

        let client = Client {
            rpc_url: rpc_url.to_string(),
            private_key: self.private_key,
            storage_provider: self.storage_provider,
            request_builder: Some(request_builder),
            deployment,
        };

        Ok(client)
    }

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

    pub fn with_caller(self, caller: Address) -> Self {
        Self { caller: Some(caller), ..self }
    }

    /// Set the signer from the given private key.
    pub fn with_private_key_str(self, private_key: impl AsRef<str>) -> Self {
        Self { private_key: Some(private_key.as_ref().to_string()), ..self }
    }

    /// Set the storage provider.
    ///
    /// The returned [ClientBuilder] will be generic over the provider [StorageProvider] type.
    pub fn with_storage_provider<Z: StorageProvider>(
        self,
        storage_provider: Option<Z>,
    ) -> ClientBuilder<Z> {
        // NOTE: We can't use the ..self syntax here because return is not Self.
        ClientBuilder {
            storage_provider,
            deployment: self.deployment,
            rpc_url: self.rpc_url,
            caller: self.caller,
            private_key: self.private_key,
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
    ) -> Result<ClientBuilder<StandardStorageProvider>, Error> {
        let storage_provider = match StandardStorageProvider::from_config(config) {
            std::result::Result::Ok(storage_provider) => Some(storage_provider),
            Err(StandardStorageProviderError::NoProvider) => None,
            Err(e) => return Err(e.into()),
        };
        Ok(self.with_storage_provider(storage_provider))
    }

    /// Modify the [OfferLayer] configuration used in the [StandardRequestBuilder].
    ///
    /// ```rust
    /// # use boundless_cli_sdk::client::ClientBuilder;
    /// use alloy_primitives::utils::parse_units;
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
    /// # use boundless_cli_sdk::client::ClientBuilder;
    /// use boundless_cli_sdk::request_builder::RequestIdLayerMode;
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
    /// # use boundless_cli_sdk::client::ClientBuilder;
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

/// Alias for a [Client] instantiated with the standard implementations provided by this crate.
pub type StandardClient = Client<StandardStorageProvider, StandardRequestBuilder>;

#[derive(Debug, Clone)]
pub struct Client<S = StandardStorageProvider, R = StandardRequestBuilder> {
    /// RPC URL of the node to connect to.
    pub rpc_url: String,
    /// Deployment of Boundless that this client is connected to.
    pub deployment: Deployment,
    /// Private key of the account that will be used to sign transactions.
    pub private_key: Option<String>,
    /// [StorageProvider] to upload programs and inputs.
    ///
    /// If not provided, this client will not be able to upload programs or inputs.
    pub storage_provider: Option<S>,
    /// [RequestBuilder] to construct [ProofRequest].
    ///
    /// If not provided, requests must be fully constructed before handing them to this client.
    pub request_builder: Option<R>,
}

impl Client<NotProvided, NotProvided> {
    /// Create a new client with the given RPC URL and deployment.
    pub fn new(rpc_url: String, boundless_market: Address, set_verifier: Address) -> Self {
        Self {
            rpc_url,
            deployment: Deployment {
                boundless_market_address: boundless_market,
                set_verifier_address: set_verifier,
                order_stream_url: None,
                chain_id: None,
                verifier_router_address: None,
                stake_token_address: None,
            },
            private_key: None,
            storage_provider: None,
            request_builder: None,
        }
    }
}

impl<S, R> Client<S, R> {
    /// Set the Boundless market address
    pub fn with_boundless_market(self, boundless_market: Address) -> Self {
        Self {
            deployment: Deployment {
                boundless_market_address: boundless_market,
                ..self.deployment
            },
            ..self
        }
    }

    /// Set the set verifier address
    pub fn with_set_verifier(self, set_verifier: Address) -> Self {
        Self {
            deployment: Deployment { set_verifier_address: set_verifier, ..self.deployment },
            ..self
        }
    }

    /// Set the storage provider
    pub fn with_storage_provider(self, storage_provider: S) -> Self
    where
        S: StorageProvider,
    {
        Self { storage_provider: Some(storage_provider), ..self }
    }

    /// Set the order stream url
    pub fn order_stream_url(self, order_stream: String) -> Self {
        Self {
            deployment: Deployment {
                order_stream_url: Some(order_stream.into()),
                ..self.deployment
            },
            ..self
        }
    }

    /// Set the private key to use for signing transactions.
    /// This is required for submitting requests.
    pub fn with_private_key(self, private_key: String) -> Self {
        Self { private_key: Some(private_key), ..self }
    }

    /// Upload a program binary to the storage provider.
    pub async fn upload_program(&self, program: &[u8]) -> Result<Url, Error>
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

    /// Upload input to the storage provider.
    pub async fn upload_input(&self, input: &[u8]) -> Result<Url, Error>
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
    ) -> Result<ProofRequest, Error>
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
    pub async fn submit_onchain<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<(U256, u64), Error>
    where
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        self.submit_request_onchain(&self.build_request(params).await?).await
    }

    /// Submit a proof request in an onchain transaction.
    pub async fn submit_request_onchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before submitting a request.",
            )
        })?;

        let temp_file = create_proof_request_yaml_temp_file(request)?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--private-key")
            .arg(private_key)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--json")
            .arg("request")
            .arg("submit")
            .arg(temp_file.path());
        let output = cmd.output().context("Failed to execute command to submit request onchain")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to submit request onchain: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request submission")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request submission")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request submission failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::RequestSubmitted { request_id, expires_at }) = response.data {
            return Ok((request_id, expires_at));
        }
        Err(Error::msg("Request submission failed"))
    }

    /// Build and submit a proof request offchain via the order stream service.
    pub async fn submit_offchain<Params>(
        &self,
        params: impl Into<Params>,
    ) -> Result<(U256, u64), Error>
    where
        R: RequestBuilder<Params>,
        R::Error: Into<anyhow::Error>,
    {
        self.submit_request_offchain(&self.build_request(params).await?).await
    }

    /// Submit a proof request offchain via the order stream service.
    pub async fn submit_request_offchain(
        &self,
        request: &ProofRequest,
    ) -> Result<(U256, u64), Error> {
        check_boundless_cli_installed()?;
        let order_stream_url = self.deployment.order_stream_url.as_ref().ok_or_else(|| {
            Error::msg(
                "Order stream URL is not set. Please set the order stream URL before submitting a request offchain.",
            )
        })?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before submitting a request.",
            )
        })?;

        let temp_file = create_proof_request_yaml_temp_file(request)?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--private-key")
            .arg(private_key)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--order-stream-url")
            .arg(order_stream_url.to_string())
            .arg("--json")
            .arg("request")
            .arg("submit")
            .arg(temp_file.path())
            .arg("--offchain");
        let output =
            cmd.output().context("Failed to execute command to submit request offchain")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to submit request offchain: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request submission")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request submission")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request submission failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::RequestSubmitted { request_id, expires_at }) = response.data {
            return Ok((request_id, expires_at));
        }
        Err(Error::msg("Request submission failed"))
    }

    /// Get the status of a request by its ID.
    pub fn status(
        &self,
        request_id: U256,
        expires_at: Option<u64>,
    ) -> Result<RequestStatus, Error> {
        check_boundless_cli_installed()?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--json")
            .arg("request")
            .arg("status")
            .arg(format!("0x{request_id:x}"));
        if let Some(expires_at) = expires_at {
            cmd.arg(format!("{expires_at}"));
        }
        let output = cmd.output().context("Failed to execute command to get request status")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to get request status: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request status")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request status")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request status check failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::RequestStatus { status }) = response.data {
            return Ok(status);
        }
        Err(Error::msg("Request status check failed"))
    }

    /// Wait for a request to be fulfilled.
    ///
    /// The check interval is the time between each check for fulfillment.
    /// `expires_at` is the maximum time to wait for the request to be fulfilled.
    pub async fn wait_for_request_fulfillment(
        &self,
        request_id: U256,
        check_interval: std::time::Duration,
        expires_at: u64,
    ) -> Result<(Bytes, Bytes), Error> {
        check_boundless_cli_installed()?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--json")
            .arg("request")
            .arg("get-proof")
            .arg(format!("0x{request_id:x}"));
        loop {
            let status = &self.status(request_id, Some(expires_at))?;
            match status {
                RequestStatus::Expired => return Err(Error::msg("Request has expired")),
                RequestStatus::Fulfilled => {
                    break;
                }
                _ => {
                    tokio::time::sleep(check_interval).await;
                    continue;
                }
            }
        }
        let output =
            cmd.output().context("Failed to execute command to fetch request fulfillment")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to fetch request fulfillment: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request fulfillment")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request fulfillment")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request fulfillment failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::RequestFulfilled { journal, seal }) = response.data {
            return Ok((journal, seal));
        }
        Err(Error::msg("Request fulfillment failed"))
    }

    /// Lock a request by its ID.
    pub fn lock_request(&self, request_id: U256) -> Result<(), Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before locking a request.",
            )
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("proving")
            .arg("lock")
            .arg("--request-id")
            .arg(format!("0x{request_id:x}"));
        let output = cmd.output().context("Failed to execute command to lock request")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to lock request: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout =
            String::from_utf8(output.stdout).context("Failed to parse output from request lock")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request lock")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request lock failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::Ok) = response.data {
            return Ok(());
        }
        Err(Error::msg("Request lock failed"))
    }

    /// Fulfill a request by its ID.
    pub fn fulfill(&self, request_id: U256) -> Result<(), Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before fulfilling a request.",
            )
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("proving")
            .arg("fulfill")
            .arg("--request-ids")
            .arg(format!("0x{request_id:x}"));
        let output = cmd.output().context("Failed to execute command to fulfill request")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to fulfill request: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request fulfillment")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request fulfillment")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request fulfillment failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::Ok) = response.data {
            return Ok(());
        }
        Err(Error::msg("Request fulfillment failed"))
    }

    /// Get the balance of an account in the Boundless Market.
    pub fn balance_of(&self, address: Address) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--json")
            .arg("account")
            .arg("balance")
            .arg(format!("0x{address:x}"));
        let output = cmd.output().context("Failed to execute command to get account balance")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to get account balance: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account balance")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account balance")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account balance check failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountAmount { amount_eth }) = response.data {
            return Ok(parse_ether(&amount_eth).context("Invalid balance format")?);
        }
        Err(Error::msg("Account balance check failed"))
    }

    /// Deposit an amount into the Boundless Market account.
    pub fn deposit(&self, amount: U256) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg("Private key is not set. Please set the private key before depositing.")
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("account")
            .arg("deposit")
            .arg(format_ether(amount).as_str());
        let output = cmd.output().context("Failed to execute command to deposit")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to deposit: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account deposit")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account deposit")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account deposit failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountAmount { amount_eth }) = response.data {
            return Ok(parse_ether(&amount_eth).context("Invalid amount format")?);
        }
        Err(Error::msg("Account deposit check failed"))
    }

    /// Withdraw an amount from the Boundless Market account.
    pub fn withdraw(&self, amount: U256) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg("Private key is not set. Please set the private key before withdrawing.")
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("account")
            .arg("withdraw")
            .arg(format_ether(amount).as_str());
        let output = cmd.output().context("Failed to execute command to withdraw")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to withdraw: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account withdrawal")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account withdrawal")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account withdrawal failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountAmount { amount_eth }) = response.data {
            return Ok(parse_ether(&amount_eth).context("Invalid amount format")?);
        }
        Err(Error::msg("Account withdrawal check failed"))
    }

    /// Get the stake balance of an account in the Boundless Market.
    pub fn stake_balance_of(&self, address: Address) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--json")
            .arg("account")
            .arg("stake-balance")
            .arg(format!("0x{address:x}"));
        let output =
            cmd.output().context("Failed to execute command to get account stake balance")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to get account stake balance: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account stake balance")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account stake balance")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account stake balance check failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountStakeAmount { amount, symbol: _, decimals }) = response.data {
            return Ok(parse_units(&amount, decimals)
                .context("Invalid stake balance format")?
                .into());
        }
        Err(Error::msg("Account stake balance check failed"))
    }

    /// Deposit stake into the Boundless Market account.
    pub fn deposit_stake(&self, amount: String) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before depositing stake.",
            )
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("account")
            .arg("deposit-stake")
            .arg(amount);
        let output = cmd.output().context("Failed to execute command to deposit stake")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to deposit stake: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account stake deposit")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account stake deposit")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account stake deposit failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountStakeAmount { amount, symbol: _, decimals }) = response.data {
            return Ok(parse_units(&amount, decimals).context("Invalid amount format")?.into());
        }
        Err(Error::msg("Account stake deposit check failed"))
    }

    /// Withdraw stake from the Boundless Market account.
    pub fn withdraw_stake(&self, amount: String) -> Result<U256, Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg(
                "Private key is not set. Please set the private key before withdrawing stake.",
            )
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("account")
            .arg("withdraw-stake")
            .arg(amount);
        let output = cmd.output().context("Failed to execute command to withdraw stake")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to withdraw stake: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from account stake withdrawal")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from account stake withdrawal")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Account stake withdrawal failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::AccountStakeAmount { amount, symbol: _, decimals }) = response.data {
            return Ok(parse_units(&amount, decimals).context("Invalid amount format")?.into());
        }
        Err(Error::msg("Account stake withdrawal check failed"))
    }

    /// Slash a request by its ID.
    pub fn slash(&self, request_id: U256) -> Result<(), Error> {
        check_boundless_cli_installed()?;
        let private_key = self.private_key.as_ref().ok_or_else(|| {
            Error::msg("Private key is not set. Please set the private key before slashing.")
        })?;
        let mut boundless = Command::new("boundless");
        let cmd = boundless
            .arg("--rpc-url")
            .arg(&self.rpc_url)
            .arg("--boundless-market-address")
            .arg(self.deployment.boundless_market_address.to_string())
            .arg("--set-verifier-address")
            .arg(self.deployment.set_verifier_address.to_string())
            .arg("--private-key")
            .arg(private_key)
            .arg("--json")
            .arg("ops")
            .arg("slash")
            .arg(format!("0x{request_id:x}"));
        let output = cmd.output().context("Failed to execute command to slash")?;
        if !output.status.success() {
            return Err(Error::msg(format!(
                "Failed to slash: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let stdout = String::from_utf8(output.stdout)
            .context("Failed to parse output from request slashing")?;
        let response = serde_json::from_str::<CliResponse<Output>>(&stdout)
            .context("Failed to deserialize output from request slashing")?;
        if !response.success {
            return Err(Error::msg(format!(
                "Request slashing failed: {}",
                response.error.unwrap_or_else(|| "Unknown error".to_string())
            )));
        }
        if let Some(Output::Ok) = response.data {
            return Ok(());
        }
        Err(Error::msg("Request slashing failed"))
    }
}

fn check_boundless_cli_installed() -> Result<()> {
    if Command::new("boundless").arg("--version").output().is_err() {
        let error_message = "The 'boundless' CLI tool is not installed or not in your PATH.\n\
            Please install it by running 'cargo install --locked boundless-cli'.";
        return Err(Error::msg(error_message));
    }
    Ok(())
}

fn create_proof_request_yaml_temp_file(proof_request: &ProofRequest) -> Result<NamedTempFile> {
    let yaml_string =
        serde_yaml::to_string(proof_request).context("Failed to serialize ProofRequest to YAML")?;
    let mut temp_file = NamedTempFile::new().context("Failed to create temporary file")?;
    temp_file
        .write_all(yaml_string.as_bytes())
        .context("Failed to write YAML to temporary file")?;
    temp_file.flush().context("Failed to flush temporary file")?;

    Ok(temp_file)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    use crate::{input::GuestEnv, request_builder::OfferParamsBuilder};
    use alloy::{
        node_bindings::{Anvil, AnvilInstance},
        providers::{Provider, WalletProvider},
    };
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::Deployment as BoundlessMarketDeployment;
    use boundless_market_test_utils::{create_test_ctx, TestCtx, ECHO_PATH};

    fn to_deployment(dep: BoundlessMarketDeployment) -> Deployment {
        Deployment {
            chain_id: dep.chain_id,
            boundless_market_address: dep.boundless_market_address,
            verifier_router_address: dep.verifier_router_address,
            set_verifier_address: dep.set_verifier_address,
            stake_token_address: dep.stake_token_address,
            order_stream_url: dep.order_stream_url,
        }
    }

    enum AccountOwner {
        Customer,
        Prover,
    }

    /// Test setup helper that creates common test infrastructure
    async fn setup_test_env(
        owner: AccountOwner,
    ) -> (TestCtx<impl Provider + WalletProvider + Clone + 'static>, AnvilInstance, Client) {
        let anvil = Anvil::new().spawn();

        let ctx = create_test_ctx(&anvil).await.unwrap();

        let private_key = match owner {
            AccountOwner::Customer => {
                ctx.prover_market
                    .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
                    .await
                    .unwrap();
                ctx.customer_signer.clone()
            }
            AccountOwner::Prover => ctx.prover_signer.clone(),
        };

        let client: StandardClient = ClientBuilder::default()
            .with_rpc_url(anvil.endpoint_url())
            .with_caller(private_key.address())
            .with_private_key_str(hex::encode(private_key.to_bytes()))
            .with_deployment(to_deployment(ctx.deployment.clone()))
            .build()
            .await
            .unwrap();

        (ctx, anvil, client)
    }

    #[tokio::test]
    async fn test_account() {
        let (ctx, _anvil, client) = setup_test_env(AccountOwner::Prover).await;

        let amount = client.deposit(parse_ether("1").unwrap()).unwrap();
        assert_eq!(amount, parse_ether("1").unwrap());

        let balance = client.balance_of(ctx.prover_signer.address()).unwrap();
        assert_eq!(balance, amount);

        let amount = client.withdraw(parse_ether("0.5").unwrap()).unwrap();
        assert_eq!(amount, parse_ether("0.5").unwrap());

        let balance = client.balance_of(ctx.prover_signer.address()).unwrap();
        assert_eq!(balance, parse_ether("0.5").unwrap());
    }

    #[tokio::test]
    async fn test_account_stake() {
        let (ctx, _anvil, client) = setup_test_env(AccountOwner::Prover).await;

        let amount = client.deposit_stake("1".into()).unwrap();
        assert_eq!(amount, parse_ether("1").unwrap());

        let balance = client.stake_balance_of(ctx.prover_signer.address()).unwrap();
        assert_eq!(balance, amount);

        let amount = client.withdraw_stake("0.5".into()).unwrap();
        assert_eq!(amount, parse_ether("0.5").unwrap());

        let balance = client.stake_balance_of(ctx.prover_signer.address()).unwrap();
        assert_eq!(balance, parse_ether("0.5").unwrap());
    }

    #[tokio::test]
    #[ignore = "Requires RISC0_DEV_MODE=1"]
    async fn test_slash() {
        let (_ctx, _anvil, client) = setup_test_env(AccountOwner::Customer).await;

        let request_params = client
            .new_request()
            .with_program_url(Url::parse(&format!("file://{ECHO_PATH}")).unwrap())
            .unwrap()
            .with_env(GuestEnv::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]))
            .with_offer(
                OfferParamsBuilder::default()
                    .lock_stake(U256::from(0))
                    .timeout(30)
                    .lock_timeout(30)
                    .ramp_up_period(0),
            );
        let request = client.build_request(request_params).await.unwrap();

        let (request_id, expires_at) = client.submit_request_onchain(&request).await.unwrap();

        client.lock_request(request_id).unwrap();

        loop {
            // Wait for the timeout to expire
            tokio::time::sleep(Duration::from_secs(1)).await;
            let status = client.status(request_id, Some(expires_at)).unwrap();
            if status == RequestStatus::Expired {
                break;
            }
        }

        client.slash(request_id).unwrap();
    }

    #[tokio::test]
    #[ignore = "Requires RISC0_DEV_MODE=1"]
    async fn test_e2e() {
        let (_ctx, _anvil, client) = setup_test_env(AccountOwner::Customer).await;

        let request_params = client
            .new_request()
            .with_program_url(Url::parse(&format!("file://{ECHO_PATH}")).unwrap())
            .unwrap()
            .with_env(GuestEnv::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]))
            .with_offer(OfferParamsBuilder::default().lock_stake(U256::from(0)));
        let request = client.build_request(request_params).await.unwrap();

        let (request_id, expires_at) = client.submit_request_onchain(&request).await.unwrap();
        assert_eq!(request_id, request.id.into());

        let status = client.status(request_id, Some(expires_at)).unwrap();
        assert_eq!(status, RequestStatus::Unknown);
        client.lock_request(request_id).unwrap();
        let status = client.status(request_id, Some(expires_at)).unwrap();
        assert_eq!(status, RequestStatus::Locked);
        client.fulfill(request_id).unwrap();
        let (_journal, _seal) = client
            .wait_for_request_fulfillment(request_id, std::time::Duration::from_secs(1), expires_at)
            .await
            .unwrap();
        let status = client.status(request_id, Some(expires_at)).unwrap();
        assert_eq!(status, RequestStatus::Fulfilled);
    }
}
