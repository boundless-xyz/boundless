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

use std::{borrow::Cow, fmt, fmt::Debug, future::Future};

use alloy::{network::Ethereum, providers::Provider};
use derive_builder::Builder;
use risc0_ethereum_contracts::selector::Selector;
use risc0_zkvm::{Digest, Journal};
use url::Url;

use crate::{
    contracts::{Input as RequestInput, ProofRequest, RequestId},
    input::GuestEnv,
    storage::{StandardStorageProvider, StorageProvider},
    util::{NotProvided, StandardRpcProvider},
};
mod preflight_layer;
mod storage_layer;

pub use preflight_layer::PreflightLayer;
pub use storage_layer::{StorageLayer, StorageLayerConfig, StorageLayerConfigBuilder};
mod requirements_layer;
pub use requirements_layer::{RequirementParams, RequirementsLayer};
mod request_id_layer;
pub use request_id_layer::{
    RequestIdLayer, RequestIdLayerConfig, RequestIdLayerConfigBuilder, RequestIdLayerMode,
};
mod offer_layer;
pub use offer_layer::{
    OfferLayer, OfferLayerConfig, OfferLayerConfigBuilder, OfferParams, OfferParamsBuilder,
};
mod finalizer;
pub use finalizer::{Finalizer, FinalizerConfig, FinalizerConfigBuilder};

/// A trait for building proof requests, used by the [Client][crate::Client].
///
/// See [StandardRequestBuilder] for an example implementation.
pub trait RequestBuilder<Params> {
    /// Error type that may be returned by this builder.
    type Error;

    // NOTE: Takes the self receiver so that the caller does not need to explicitly name the
    // RequestBuilder trait (e.g. `<MyRequestBuilder as RequestBuilder>::params()`). This could
    // also be used to set initial values on the params that are specific to the rrequest builder.
    /// Returns a default instance of the parameter type used by this builder.
    fn params(&self) -> Params
    where
        Params: Default,
    {
        Default::default()
    }

    /// Builds a [ProofRequest] using the provided parameters.
    fn build(
        &self,
        params: impl Into<Params>,
    ) -> impl Future<Output = Result<ProofRequest, Self::Error>>;
}

/// Blanket implementation for [RequestBuilder] for all [Layer] that output a proof request.
///
/// This implementation allows for custom and modified layered builders to automatically be usable
/// as a [RequestBuilder].
impl<L, Params> RequestBuilder<Params> for L
where
    L: Layer<Params, Output = ProofRequest>,
{
    type Error = L::Error;

    async fn build(&self, params: impl Into<Params>) -> Result<ProofRequest, Self::Error> {
        self.process(params.into()).await
    }
}

/// A trait representing a processing layer in a request building pipeline.
///
/// Layers can be composed together to form a multi-step processing pipeline where the output
/// of one layer becomes the input to the next. Each layer handles a specific aspect of the
/// request building process.
pub trait Layer<Input> {
    /// The output type produced by this layer.
    type Output;

    /// Error type that may be returned by this layer.
    type Error;

    /// Processes the input and returns the transformed output.
    fn process(&self, input: Input) -> impl Future<Output = Result<Self::Output, Self::Error>>;
}

/// A trait for adapting types to be processed by a [Layer].
///
/// This trait provides a mechanism for a type to be processed by a layer, enabling
/// the composition of multiple layers into a processing pipeline. Inputs are adapted
/// to work with specific layer types, with the output of one layer feeding into the next.
///
/// Existing [Layer] implementations can be adapted to work with new parameter types by
/// implementating `Adapt<Layer>` on the parameter type.
pub trait Adapt<L> {
    /// The output type after processing by the layer.
    type Output;

    /// Error type that may be returned during processing.
    type Error;

    /// Processes this value with the provided layer.
    fn process_with(self, layer: &L) -> impl Future<Output = Result<Self::Output, Self::Error>>;
}

impl<L, I> Adapt<L> for I
where
    L: Layer<I>,
{
    type Output = L::Output;
    type Error = L::Error;

    async fn process_with(self, layer: &L) -> Result<Self::Output, Self::Error> {
        layer.process(self).await
    }
}

/// Define a layer as a stack of two layers. Output of layer A is piped into layer B.
impl<A, B, Input> Layer<Input> for (A, B)
where
    Input: Adapt<A>,
    <Input as Adapt<A>>::Output: Adapt<B>,
    <Input as Adapt<A>>::Error: Into<<<Input as Adapt<A>>::Output as Adapt<B>>::Error>,
{
    type Output = <<Input as Adapt<A>>::Output as Adapt<B>>::Output;
    type Error = <<Input as Adapt<A>>::Output as Adapt<B>>::Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error> {
        input.process_with(&self.0).await.map_err(Into::into)?.process_with(&self.1).await
    }
}

/// A standard implementation of [RequestBuilder] that uses a layered architecture.
///
/// This builder composes multiple layers, each handling a specific aspect of request building:
/// - `storage_layer`: Manages program and input storage
/// - `preflight_layer`: Validates and simulates the request
/// - `requirements_layer`: Sets up verification requirements
/// - `request_id_layer`: Manages request identifier generation
/// - `offer_layer`: Configures the offer details
/// - `finalizer`: Validates and finalizes the request
///
/// Each layer processes the request in sequence, with the output of one layer becoming
/// the input for the next.
#[derive(Clone, Builder)]
#[non_exhaustive]
pub struct StandardRequestBuilder<P = StandardRpcProvider, S = StandardStorageProvider> {
    /// Handles uploading and preparing program and input data.
    #[builder(setter(into))]
    pub storage_layer: StorageLayer<S>,

    /// Executes preflight checks to validate the request.
    #[builder(setter(into), default)]
    pub preflight_layer: PreflightLayer,

    /// Configures the requirements for the proof request.
    #[builder(setter(into), default)]
    pub requirements_layer: RequirementsLayer,

    /// Generates and manages request identifiers.
    #[builder(setter(into))]
    pub request_id_layer: RequestIdLayer<P>,

    /// Configures offer parameters for the request.
    #[builder(setter(into))]
    pub offer_layer: OfferLayer<P>,

    /// Finalizes and validates the complete request.
    #[builder(setter(into), default)]
    pub finalizer: Finalizer,
}

impl StandardRequestBuilder<NotProvided, NotProvided> {
    /// Creates a new builder for constructing a [StandardRequestBuilder].
    ///
    /// This is the entry point for creating a request builder with specific
    /// provider and storage implementations.
    ///
    /// # Type Parameters
    /// * `P` - An Ethereum RPC provider, using alloy.
    /// * `S` - The storage provider type for storing programs and inputs.
    pub fn builder<P: Clone, S: Clone>() -> StandardRequestBuilderBuilder<P, S> {
        Default::default()
    }
}

impl<P, S> Layer<RequestParams> for StandardRequestBuilder<P, S>
where
    S: StorageProvider,
    S::Error: std::error::Error + Send + Sync + 'static,
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(&self, input: RequestParams) -> Result<ProofRequest, Self::Error> {
        input
            .process_with(&self.storage_layer)
            .await?
            .process_with(&self.preflight_layer)
            .await?
            .process_with(&self.requirements_layer)
            .await?
            .process_with(&self.request_id_layer)
            .await?
            .process_with(&self.offer_layer)
            .await?
            .process_with(&self.finalizer)
            .await
    }
}

impl<P> Layer<RequestParams> for StandardRequestBuilder<P, NotProvided>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(&self, input: RequestParams) -> Result<ProofRequest, Self::Error> {
        input
            .process_with(&self.storage_layer)
            .await?
            .process_with(&self.preflight_layer)
            .await?
            .process_with(&self.requirements_layer)
            .await?
            .process_with(&self.request_id_layer)
            .await?
            .process_with(&self.offer_layer)
            .await?
            .process_with(&self.finalizer)
            .await
    }
}

// NOTE: We don't use derive_builder here because we need to be able to access the values on the
// incrementally built parameters.
/// Parameters for building a proof request.
///
/// This struct holds all the necessary information for constructing a [ProofRequest].
/// It provides a builder pattern for incrementally setting fields and methods for
/// validating and accessing the parameters.
///
/// Most fields are optional and can be populated during the request building process
/// by various layers. The structure serves as the central data container that passes
/// through the request builder pipeline.
#[non_exhaustive]
#[derive(Clone, Default)]
pub struct RequestParams {
    /// RISC-V guest program that will be run in the zkVM.
    pub program: Option<Cow<'static, [u8]>>,

    /// Guest execution environment, providing the input for the guest.
    /// See [GuestEnv].
    pub env: Option<GuestEnv>,

    /// Uploaded program URL, from which provers will fetch the program.
    pub program_url: Option<Url>,

    /// Prepared input for the [ProofRequest], containing either a URL or inline input.
    /// See [RequestInput].
    pub request_input: Option<RequestInput>,

    /// Count of the RISC Zero execution cycles. Used to estimate proving cost.
    pub cycles: Option<u64>,

    /// Image ID identifying the program being executed.
    pub image_id: Option<Digest>,

    /// Contents of the [Journal] that results from the execution.
    pub journal: Option<Journal>,

    /// [RequestId] to use for the proof request.
    pub request_id: Option<RequestId>,

    /// [OfferParams] for constructing the [Offer][crate::Offer] to send along with the request.
    pub offer: OfferParams,

    /// [RequirementParams] for constructing the [Requirements][crate::Requirements] for the resulting proof.
    pub requirements: RequirementParams,
}

impl RequestParams {
    /// Creates a new empty instance of [RequestParams].
    ///
    /// This is equivalent to calling `Default::default()` and is provided as a
    /// convenience method for better readability when building requests.
    pub fn new() -> Self {
        Self::default()
    }

    /// Gets the program bytes, returning an error if not set.
    ///
    /// This method is used by layers in the request building pipeline to access
    /// the program when it's required for processing.
    pub fn require_program(&self) -> Result<&[u8], MissingFieldError> {
        self.program.as_deref().ok_or(MissingFieldError::new("program"))
    }

    /// Sets the program to be executed in the zkVM.
    pub fn with_program(self, value: impl Into<Cow<'static, [u8]>>) -> Self {
        Self { program: Some(value.into()), ..self }
    }

    /// Gets the guest environment, returning an error if not set.
    ///
    /// The guest environment contains the input data for the program.
    pub fn require_env(&self) -> Result<&GuestEnv, MissingFieldError> {
        self.env.as_ref().ok_or(MissingFieldError::new("env"))
    }

    /// Sets the [GuestEnv], providing the guest with input.
    ///
    /// Can be constructed with [GuestEnvBuilder][crate::input::GuestEnvBuilder].
    ///
    /// ```rust
    /// # use boundless_market::request_builder::RequestParams;
    /// # const ECHO_ELF: &[u8] = b"";
    /// use boundless_market::GuestEnvBuilder;
    ///
    /// RequestParams::new()
    ///     .with_program(ECHO_ELF)
    ///     .with_env(GuestEnvBuilder::new()
    ///         .write_frame(b"hello!")
    ///         .write_frame(b"goodbye."));
    /// ```
    pub fn with_env(self, value: impl Into<GuestEnv>) -> Self {
        Self { env: Some(value.into()), ..self }
    }

    /// Sets the [GuestEnv] to be contain the given bytes as `stdin`.
    ///
    /// Note that the bytes are passed directly to the guest without encoding. If your guest
    /// expects the input to be encoded in any way (e.g. `bincode`), the caller must encode the
    /// data before passing it.
    ///
    /// If the [GuestEnv] is already set, this replaces it.
    ///
    /// ```rust
    /// # use boundless_market::request_builder::RequestParams;
    /// # const ECHO_ELF: &[u8] = b"";
    /// RequestParams::new()
    ///     .with_program(ECHO_ELF)
    ///     .with_stdin(b"hello!");
    /// ```
    ///
    /// See also [Self::with_env] and [GuestEnvBuilder][crate::input::GuestEnvBuilder]
    pub fn with_stdin(self, value: impl Into<Vec<u8>>) -> Self {
        Self { env: Some(GuestEnv::from_stdin(value)), ..self }
    }

    /// Gets the program URL, returning an error if not set.
    ///
    /// The program URL is where provers will download the program to execute.
    pub fn require_program_url(&self) -> Result<&Url, MissingFieldError> {
        self.program_url.as_ref().ok_or(MissingFieldError::new("program_url"))
    }

    /// Set the program URL, where provers can download the program to be proven.
    ///
    /// ```rust
    /// # use boundless_market::request_builder::RequestParams;
    /// # || -> anyhow::Result<()> {
    /// RequestParams::new()
    ///     .with_program_url("https://fileserver.example/guest.bin")?;
    /// # Ok(())
    /// # }().unwrap();
    /// ```
    pub fn with_program_url<T: TryInto<Url>>(self, value: T) -> Result<Self, T::Error> {
        Ok(Self { program_url: Some(value.try_into()?), ..self })
    }

    /// Gets the request input, returning an error if not set.
    ///
    /// The request input contains the input data for the guest program, either inline or as a URL.
    pub fn require_request_input(&self) -> Result<&RequestInput, MissingFieldError> {
        self.request_input.as_ref().ok_or(MissingFieldError::new("input"))
    }

    /// Sets the input data for the request.
    ///
    /// This is typically created from a GuestEnv by the storage layer.
    pub fn with_request_input(self, value: impl Into<RequestInput>) -> Self {
        Self { request_input: Some(value.into()), ..self }
    }

    /// Gets the cycle count, returning an error if not set.
    ///
    /// The cycle count is used to estimate proving costs.
    pub fn require_cycles(&self) -> Result<u64, MissingFieldError> {
        self.cycles.ok_or(MissingFieldError::new("cycles"))
    }

    /// Sets the cycle count for the proof request.
    ///
    /// This is used to estimate proving costs and determine appropriate pricing.
    pub fn with_cycles(self, value: u64) -> Self {
        Self { cycles: Some(value), ..self }
    }

    /// Gets the journal, returning an error if not set.
    ///
    /// The journal contains the output from the guest program execution.
    pub fn require_journal(&self) -> Result<&Journal, MissingFieldError> {
        self.journal.as_ref().ok_or(MissingFieldError::new("journal"))
    }

    /// Sets the journal for the request.
    ///
    /// The journal is the output from the guest program execution and is used
    /// to configure verification requirements.
    pub fn with_journal(self, value: impl Into<Journal>) -> Self {
        Self { journal: Some(value.into()), ..self }
    }

    /// Gets the image ID, returning an error if not set.
    ///
    /// The image ID uniquely identifies the program being executed.
    pub fn require_image_id(&self) -> Result<Digest, MissingFieldError> {
        self.image_id.ok_or(MissingFieldError::new("image_id"))
    }

    /// Sets the image ID for the request.
    ///
    /// The image ID is the hash of the program binary and uniquely identifies
    /// the program being executed.
    pub fn with_image_id(self, value: impl Into<Digest>) -> Self {
        Self { image_id: Some(value.into()), ..self }
    }

    /// Gets the request ID, returning an error if not set.
    ///
    /// The request ID contains the requestor's address and a unique index,
    /// and is used to track the request throughout its lifecycle.
    pub fn require_request_id(&self) -> Result<&RequestId, MissingFieldError> {
        self.request_id.as_ref().ok_or(MissingFieldError::new("request_id"))
    }

    /// Sets the request ID for the proof request.
    ///
    /// The request ID contains the requestor's address and a unique index,
    /// and is used to track the request throughout its lifecycle.
    pub fn with_request_id(self, value: impl Into<RequestId>) -> Self {
        Self { request_id: Some(value.into()), ..self }
    }

    /// Configure the [Offer][crate::Offer] on the [ProofRequest] by either providing a complete
    /// offer, or a partial offer via [OfferParams].
    ///
    /// ```rust
    /// # use boundless_market::request_builder::{RequestParams, OfferParams};
    /// use alloy::primitives::utils::parse_units;
    ///
    /// RequestParams::new()
    ///     .with_offer(OfferParams::builder()
    ///         .max_price(parse_units("0.01", "ether").unwrap())
    ///         .ramp_up_period(30)
    ///         .lock_timeout(120)
    ///         .timeout(240));
    /// ```
    pub fn with_offer(self, value: impl Into<OfferParams>) -> Self {
        Self { offer: value.into(), ..self }
    }

    /// Configure the [Requirements][crate::Requirements] on the [ProofRequest] by either providing
    /// the complete requirements, or partial requirements via [RequirementParams].
    ///
    /// ```rust
    /// # use boundless_market::request_builder::{RequestParams, RequirementParams};
    /// use alloy::primitives::address;
    ///
    /// RequestParams::new()
    ///     .with_requirements(RequirementParams::builder()
    ///         .callback_address(address!("0x00000000000000000000000000000000deadbeef")));
    /// ```
    pub fn with_requirements(self, value: impl Into<RequirementParams>) -> Self {
        Self { requirements: value.into(), ..self }
    }

    /// Request a stand-alone Groth16 proof for this request.
    ///
    /// This is a convinience method to set the selector on the requirements. Note that calling
    /// [RequestParams::with_requirements] after this function will overwrite the change.
    pub fn with_groth16_proof(self) -> Self {
        // TODO(risc0-ethereum/#597): This needs to be kept up to date with releases of
        // risc0-ethereum.
        let mut requirements = self.requirements;
        requirements.selector = match risc0_zkvm::is_dev_mode() {
            true => Some((Selector::FakeReceipt as u32).into()),
            false => Some((Selector::Groth16V2_0 as u32).into()),
        };
        Self { requirements, ..self }
    }
}

impl Debug for RequestParams {
    /// [Debug] implementation that does not print the contents of the program.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExampleRequestParams")
            .field("program", &self.program.as_ref().map(|x| format!("[{} bytes]", x.len())))
            .field("env", &self.env)
            .field("program_url", &self.program_url)
            .field("input", &self.request_input)
            .field("cycles", &self.cycles)
            .field("journal", &self.journal)
            .field("request_id", &self.request_id)
            .field("offer", &self.offer)
            .field("requirements", &self.requirements)
            .finish()
    }
}

impl<Program, Env> From<(Program, Env)> for RequestParams
where
    Program: Into<Cow<'static, [u8]>>,
    Env: Into<GuestEnv>,
{
    fn from(value: (Program, Env)) -> Self {
        Self::default().with_program(value.0).with_env(value.1)
    }
}

/// Error indicating that a required field is missing when building a request.
///
/// This error is returned when attempting to access a field that hasn't been
/// set yet in the request parameters.
#[derive(thiserror::Error, Debug)]
#[error("field `{label}` is required but is uninitialized")]
pub struct MissingFieldError {
    /// The name of the missing field.
    pub label: Cow<'static, str>,
}

impl MissingFieldError {
    /// Creates a new error for the specified missing field.
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self { label: label.into() }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::{
        network::TransactionBuilder, node_bindings::Anvil, primitives::Address,
        providers::Provider, rpc::types::TransactionRequest,
    };
    use boundless_market_test_utils::{create_test_ctx, ECHO_ELF};
    use tracing_test::traced_test;
    use url::Url;

    use super::{
        Layer, OfferLayer, OfferLayerConfig, OfferParams, PreflightLayer, RequestBuilder,
        RequestId, RequestIdLayer, RequestIdLayerConfig, RequestIdLayerMode, RequestParams,
        RequirementsLayer, StandardRequestBuilder, StorageLayer, StorageLayerConfig,
    };

    use crate::{
        contracts::{
            boundless_market::BoundlessMarketService, Input as RequestInput, InputType, Predicate,
            Requirements,
        },
        input::GuestEnv,
        storage::{fetch_url, MockStorageProvider, StorageProvider},
        util::NotProvided,
    };
    use alloy_primitives::U256;
    use risc0_zkvm::{compute_image_id, sha::Digestible, Journal};

    #[tokio::test]
    #[traced_test]
    async fn basic() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await.unwrap();
        let storage = Arc::new(MockStorageProvider::start());
        let market = BoundlessMarketService::new(
            test_ctx.deployment.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );

        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(Some(storage))
            .offer_layer(test_ctx.customer_provider.clone())
            .request_id_layer(market)
            .build()?;

        let params = request_builder.params().with_program(ECHO_ELF).with_stdin(b"hello!");
        let request = request_builder.build(params).await?;
        println!("built request {request:#?}");
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn with_offer_layer_settings() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await.unwrap();
        let storage = Arc::new(MockStorageProvider::start());
        let market = BoundlessMarketService::new(
            test_ctx.deployment.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );

        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(Some(storage))
            .offer_layer(OfferLayer::new(
                test_ctx.customer_provider.clone(),
                OfferLayerConfig::builder().ramp_up_period(27).build()?,
            ))
            .request_id_layer(market)
            .build()?;

        let params = request_builder.params().with_program(ECHO_ELF).with_stdin(b"hello!");
        let request = request_builder.build(params).await?;
        assert_eq!(request.offer.rampUpPeriod, 27);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn without_storage_provider() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await.unwrap();
        let market = BoundlessMarketService::new(
            test_ctx.deployment.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );

        let request_builder = StandardRequestBuilder::builder()
            .storage_layer(None::<NotProvided>)
            .offer_layer(test_ctx.customer_provider.clone())
            .request_id_layer(market)
            .build()?;

        // Try building the reqeust by providing the program.
        let params = request_builder.params().with_program(ECHO_ELF).with_stdin(b"hello!");
        let err = request_builder.build(params).await.unwrap_err();
        tracing::debug!("err: {err}");

        // Try again after uploading the program first.
        let storage = Arc::new(MockStorageProvider::start());
        let program_url = storage.upload_program(ECHO_ELF).await?;
        let params = request_builder.params().with_program_url(program_url)?.with_stdin(b"hello!");
        let request = request_builder.build(params).await?;
        assert_eq!(
            request.requirements.imageId,
            risc0_zkvm::compute_image_id(ECHO_ELF)?.as_bytes()
        );
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer() -> anyhow::Result<()> {
        let storage = Arc::new(MockStorageProvider::start());
        let layer = StorageLayer::new(
            Some(storage.clone()),
            StorageLayerConfig::builder().inline_input_max_bytes(Some(1024)).build()?,
        );
        let env = GuestEnv::from_stdin(b"inline_data");
        let (program_url, request_input) = layer.process((ECHO_ELF, &env)).await?;

        // Program should be uploaded and input inline.
        assert_eq!(fetch_url(&program_url).await?, ECHO_ELF);
        assert_eq!(request_input.inputType, InputType::Inline);
        assert_eq!(request_input.data, env.encode()?);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer_no_provider() -> anyhow::Result<()> {
        let layer = StorageLayer::<NotProvided>::from(
            StorageLayerConfig::builder().inline_input_max_bytes(Some(1024)).build()?,
        );

        let env = GuestEnv::from_stdin(b"inline_data");
        let request_input = layer.process(&env).await?;

        // Program should be uploaded and input inline.
        assert_eq!(request_input.inputType, InputType::Inline);
        assert_eq!(request_input.data, env.encode()?);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer_large_input() -> anyhow::Result<()> {
        let storage = Arc::new(MockStorageProvider::start());
        let layer = StorageLayer::new(
            Some(storage.clone()),
            StorageLayerConfig::builder().inline_input_max_bytes(Some(1024)).build()?,
        );
        let env = GuestEnv::from_stdin(rand::random_iter().take(2048).collect::<Vec<u8>>());
        let (program_url, request_input) = layer.process((ECHO_ELF, &env)).await?;

        // Program and input should be uploaded and input inline.
        assert_eq!(fetch_url(&program_url).await?, ECHO_ELF);
        assert_eq!(request_input.inputType, InputType::Url);
        let fetched_input = fetch_url(String::from_utf8(request_input.data.to_vec())?).await?;
        assert_eq!(fetched_input, env.encode()?);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_storage_layer_large_input_no_provider() -> anyhow::Result<()> {
        let layer = StorageLayer::from(
            StorageLayerConfig::builder().inline_input_max_bytes(Some(1024)).build()?,
        );

        let env = GuestEnv::from_stdin(rand::random_iter().take(2048).collect::<Vec<u8>>());
        let err = layer.process(&env).await.unwrap_err();

        assert!(err
            .to_string()
            .contains("cannot upload input using StorageLayer with no storage_provider"));
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_preflight_layer() -> anyhow::Result<()> {
        let storage = MockStorageProvider::start();
        let program_url = storage.upload_program(ECHO_ELF).await?;
        let layer = PreflightLayer::default();
        let data = b"hello_zkvm".to_vec();
        let env = GuestEnv::from_stdin(data.clone());
        let input = RequestInput::inline(env.encode()?);
        let session = layer.process((&program_url, &input)).await?;

        assert_eq!(session.journal.as_ref(), data.as_slice());
        // Verify non-zero cycle count and an exit code of zero.
        let cycles: u64 = session.segments.iter().map(|s| 1 << s.po2).sum();
        assert!(cycles > 0);
        assert!(session.exit_code.is_ok());
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_requirements_layer() -> anyhow::Result<()> {
        let layer = RequirementsLayer::default();
        let program = ECHO_ELF;
        let bytes = b"journal_data".to_vec();
        let journal = Journal::new(bytes.clone());
        let req = layer.process((program, &journal, &Default::default())).await?;

        // Predicate should match the same journal
        assert!(req.predicate.eval(&journal));
        // And should not match different data
        let other = Journal::new(b"other_data".to_vec());
        assert!(!req.predicate.eval(&other));
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_id_layer_rand() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let market = BoundlessMarketService::new(
            test_ctx.deployment.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );
        let layer = RequestIdLayer::from(market.clone());
        assert_eq!(layer.config.mode, RequestIdLayerMode::Rand);
        let id = layer.process(()).await?;
        assert_eq!(id.addr, test_ctx.customer_signer.address());
        assert!(!id.smart_contract_signed);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_request_id_layer_nonce() -> anyhow::Result<()> {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let market = BoundlessMarketService::new(
            test_ctx.deployment.boundless_market_address,
            test_ctx.customer_provider.clone(),
            test_ctx.customer_signer.address(),
        );
        let layer = RequestIdLayer::new(
            market.clone(),
            RequestIdLayerConfig::builder().mode(RequestIdLayerMode::Nonce).build()?,
        );

        let id = layer.process(()).await?;
        assert_eq!(id.addr, test_ctx.customer_signer.address());
        // The customer address has sent no transactions.
        assert_eq!(id.index, 0);
        assert!(!id.smart_contract_signed);

        // Send a tx then check that the index increments.
        let tx = TransactionRequest::default()
            .with_from(test_ctx.customer_signer.address())
            .with_to(Address::ZERO)
            .with_value(U256::from(1));
        test_ctx.customer_provider.send_transaction(tx).await?.watch().await?;

        let id = layer.process(()).await?;
        assert_eq!(id.addr, test_ctx.customer_signer.address());
        // The customer address has sent one transaction.
        assert_eq!(id.index, 1);
        assert!(!id.smart_contract_signed);

        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_offer_layer_estimates() -> anyhow::Result<()> {
        // Use Anvil-backed provider for gas price
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let provider = test_ctx.customer_provider.clone();
        let layer = OfferLayer::from(provider.clone());
        // Build minimal requirements and request ID
        let image_id = compute_image_id(ECHO_ELF).unwrap();
        let predicate = Predicate::digest_match(Journal::new(b"hello".to_vec()).digest());
        let requirements = Requirements::new(image_id, predicate);
        let request_id = RequestId::new(test_ctx.customer_signer.address(), 0);

        // Zero cycles
        let offer_params = OfferParams::default();
        let offer_zero_mcycles =
            layer.process((&requirements, &request_id, Some(0u64), &offer_params)).await?;
        assert_eq!(offer_zero_mcycles.minPrice, U256::ZERO);
        // Defaults from builder
        assert_eq!(offer_zero_mcycles.rampUpPeriod, 60);
        assert_eq!(offer_zero_mcycles.lockTimeout, 600);
        assert_eq!(offer_zero_mcycles.timeout, 1200);
        // Max price should be non-negative, to account for fixed costs.
        assert!(offer_zero_mcycles.maxPrice > U256::ZERO);

        // Now create an offer for 100 Mcycles.
        let offer_more_mcycles =
            layer.process((&requirements, &request_id, Some(100u64 << 20), &offer_params)).await?;
        assert!(offer_more_mcycles.maxPrice > offer_zero_mcycles.maxPrice);

        // Check that overrides are respected.
        let min_price = U256::from(1u64);
        let max_price = U256::from(5u64);
        let offer_params = OfferParams::builder().max_price(max_price).min_price(min_price).into();
        let offer_zero_mcycles =
            layer.process((&requirements, &request_id, Some(0u64), &offer_params)).await?;
        assert_eq!(offer_zero_mcycles.maxPrice, max_price);
        assert_eq!(offer_zero_mcycles.minPrice, min_price);
        assert_eq!(offer_zero_mcycles.rampUpPeriod, 60);
        assert_eq!(offer_zero_mcycles.lockTimeout, 600);
        assert_eq!(offer_zero_mcycles.timeout, 1200);
        Ok(())
    }

    #[test]
    fn request_params_with_program_url_infallible() {
        // When passing a parsed URL, with_program_url should be infallible.
        // NOTE: The `match *e {}` incantation is a compile-time assert that this error cannot
        // occur.
        let url = Url::parse("https://fileserver.example/guest.bin").unwrap();
        RequestParams::new().with_program_url(url).inspect_err(|e| match *e {}).unwrap();
    }
}
