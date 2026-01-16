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

use std::{
    borrow::Cow,
    fmt,
    fmt::{Debug, Display},
    future::Future,
};

use alloy::{
    network::Ethereum,
    providers::{DynProvider, Provider},
};
use derive_builder::Builder;
use risc0_zkvm::{Digest, Journal};
use url::Url;

use crate::{
    contracts::{ProofRequest, RequestId, RequestInput},
    input::GuestEnv,
    request_builder::offer_layer::DEFAULT_TIMEOUT,
    selector::SelectorExt,
    storage::{StandardStorageProvider, StorageProvider},
    util::{now_timestamp, NotProvided},
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
pub struct StandardRequestBuilder<P = DynProvider, S = StandardStorageProvider> {
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
        self.program
            .as_deref()
            .ok_or(MissingFieldError::with_hint("program", "can be set using .with_program(...)"))
    }

    /// Sets the program to be executed in the zkVM.
    pub fn with_program(self, value: impl Into<Cow<'static, [u8]>>) -> Self {
        Self { program: Some(value.into()), ..self }
    }

    /// Gets the guest environment, returning an error if not set.
    ///
    /// The guest environment contains the input data for the program.
    pub fn require_env(&self) -> Result<&GuestEnv, MissingFieldError> {
        self.env.as_ref().ok_or(MissingFieldError::with_hint(
            "env",
            "can be set using .with_env(...) or .with_stdin",
        ))
    }

    /// Sets the [GuestEnv], providing the guest with input.
    ///
    /// Can be constructed with [GuestEnvBuilder][crate::input::GuestEnvBuilder].
    ///
    /// ```rust
    /// # use boundless_market::request_builder::RequestParams;
    /// # const ECHO_ELF: &[u8] = b"";
    /// use boundless_market::GuestEnv;
    ///
    /// RequestParams::new()
    ///     .with_program(ECHO_ELF)
    ///     .with_env(GuestEnv::builder()
    ///         .write_frame(b"hello!")
    ///         .write_frame(b"goodbye."));
    /// ```
    ///
    /// See also [Self::with_env] and [GuestEnvBuilder][crate::input::GuestEnvBuilder]
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
        self.program_url.as_ref().ok_or(MissingFieldError::with_hint(
            "program_url",
            "can be set using .with_program_url(...)",
        ))
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
        self.request_input.as_ref().ok_or(MissingFieldError::with_hint(
            "request_input",
            "can be set using .with_request_input(...)",
        ))
    }

    /// Sets the encoded input data for the request. This data will be decoded by the prover into a
    /// [GuestEnv] that will be used to run the guest.
    ///
    /// If not provided, the this will be constructed from the data given via
    /// [RequestParams::with_env] or [RequestParams::with_stdin]. If the input is set with both
    /// this method and one of those two, the input specified here takes precedence.
    pub fn with_request_input(self, value: impl Into<RequestInput>) -> Self {
        Self { request_input: Some(value.into()), ..self }
    }

    /// Sets the input as a URL from which provers can download the input data.
    ///
    /// This is a convenience method that creates a [RequestInput] with URL type.
    ///
    /// ```rust
    /// # use boundless_market::request_builder::RequestParams;
    /// # || -> anyhow::Result<()> {
    /// RequestParams::new()
    ///     .with_input_url("https://fileserver.example/input.bin")?;
    /// # Ok(())
    /// # }().unwrap();
    /// ```
    pub fn with_input_url<T: TryInto<Url>>(self, value: T) -> Result<Self, T::Error> {
        Ok(Self { request_input: Some(RequestInput::url(value.try_into()?)), ..self })
    }

    /// Gets the cycle count, returning an error if not set.
    ///
    /// The cycle count is used to estimate proving costs.
    pub fn require_cycles(&self) -> Result<u64, MissingFieldError> {
        self.cycles
            .ok_or(MissingFieldError::with_hint("cycles", "can be set using .with_cycles(...)"))
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
        self.journal
            .as_ref()
            .ok_or(MissingFieldError::with_hint("journal", "can be set using .with_journal(...)"))
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
        self.image_id.ok_or(MissingFieldError::with_hint(
            "image_id",
            "can be set using .with_image_id(...), and is calculated from the program",
        ))
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
        self.request_id.as_ref().ok_or(MissingFieldError::with_hint("request_id", "can be set using .with_request_id(...), and can be generated by boundless_market::Client"))
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
        let mut requirements = self.requirements;
        requirements.selector = match crate::util::is_dev_mode() {
            true => Some((SelectorExt::FakeReceipt as u32).into()),
            false => Some((SelectorExt::groth16_latest() as u32).into()),
        };
        Self { requirements, ..self }
    }

    /// Request a stand-alone Blake3 Groth16 proof for this request.
    ///
    /// This is a convinience method to set the selector on the requirements. Note that calling
    /// [RequestParams::with_requirements] after this function will overwrite the change.
    pub fn with_blake3_groth16_proof(self) -> Self {
        let mut requirements = self.requirements;
        requirements.selector = match crate::util::is_dev_mode() {
            true => Some((SelectorExt::FakeReceipt as u32).into()),
            false => Some((SelectorExt::blake3_groth16_latest() as u32).into()),
        };
        // TODO(ec2): should we automatically set the predicate type to claim digest match here?
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
#[derive(Debug)]
pub struct MissingFieldError {
    /// The name of the missing field.
    pub label: Cow<'static, str>,
    /// An optional hint as to the cause of the error, or how to resolve it.
    pub hint: Option<Cow<'static, str>>,
}

impl Display for MissingFieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.hint {
            None => write!(f, "field `{}` is required but is uninitialized", self.label),
            Some(ref hint) => {
                write!(f, "field `{}` is required but is uninitialized; {hint}", self.label)
            }
        }
    }
}

impl std::error::Error for MissingFieldError {}

impl MissingFieldError {
    /// Creates a new error for the specified missing field.
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self { label: label.into(), hint: None }
    }

    /// Creates a new error for the specified missing field.
    pub fn with_hint(
        label: impl Into<Cow<'static, str>>,
        hint: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self { label: label.into(), hint: Some(hint.into()) }
    }
}

/// Parameterization mode for the request builder.
///
/// Defines the proving and executor speeds used to calculate recommended timeouts.
/// Note: setting faster speeds may result in fewer provers being able to fulfill the request,
/// higher prices, and lower fulfillment guarantees.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct ParameterizationMode {
    /// Proving speed in Hz.
    proving_speed: u64,
    /// Executor speed in Hz.
    executor_speed: u64,
    /// Minimum timeout in seconds.
    min_timeout: u32,
    /// Ramp up period multiplier.
    ramp_up_period_multiplier: u32,
    /// Ramp up delay multiplier.
    ramp_up_delay_multiplier: u64,
    /// Base ramp up period in seconds.
    base_ramp_up_period: u32,
}

impl ParameterizationMode {
    /// Default proving speed in Hz.
    const DEFAULT_PROVING_SPEED_HZ: u64 = 500000; // 500 kHz

    /// Default executor speed in Hz.
    const DEFAULT_EXECUTOR_SPEED_HZ: u64 = 30000000; // 30 MHz

    /// Minimum default timeout in seconds.
    ///
    /// This is to prevent the timeout from being too short and causing the request
    /// to expire before the prover can execute and evaluate the request.
    const DEFAULT_MIN_TIMEOUT: u32 = 60;

    /// Fast proving speed in Hz.
    const FAST_PROVING_SPEED_HZ: u64 = 3000000; // 3 MHz

    /// Fast executor speed in Hz.
    const FAST_EXECUTOR_SPEED_HZ: u64 = 50000000; // 50 MHz

    /// Minimum fast timeout in seconds.
    ///
    /// This is to prevent the timeout from being too short and causing the request
    /// to expire before the prover can execute and evaluate the request.
    const FAST_MIN_TIMEOUT: u32 = 30;

    /// Default base ramp up period in seconds.
    ///
    /// This is used to ensure that the ramp up period is long enough
    /// to allow provers to execute and evaluate the request.
    const DEFAULT_BASE_RAMP_UP_PERIOD: u32 = 300; // 5 minutes

    /// Default multiplier for the ramp up period.
    ///
    /// This is used to ensure that the ramp up period is long enough
    /// to allow provers to execute and evaluate the request.
    const DEFAULT_RAMP_UP_PERIOD_MULTIPLIER: u32 = 10; // 10x the executor time

    /// Default multiplier for the ramp up delay.
    ///
    /// This is used to ensure that the ramp up start is set
    /// to allow provers to execute and evaluate the request.
    const DEFAULT_RAMP_UP_DELAY_MULTIPLIER: u64 = 2; // 2x the executor time

    /// Fast multiplier for the ramp up period.
    ///
    /// This is used to ensure that the ramp up period is long enough
    /// to allow provers to execute and evaluate the request.
    const FAST_RAMP_UP_PERIOD_MULTIPLIER: u32 = 5; // 5x the executor time

    /// Fast multiplier for the ramp up delay.
    ///
    /// This is used to ensure that the ramp up start is set
    /// to allow provers to execute and evaluate the request.
    const FAST_RAMP_UP_DELAY_MULTIPLIER: u64 = 1; // 1x the executor time

    /// Fast base ramp up period in seconds.
    ///
    /// This is used to ensure that the ramp up period is long enough
    /// to allow provers to execute and evaluate the request.
    const FAST_BASE_RAMP_UP_PERIOD: u32 = 60; // 1 minute

    /// Creates a parameterization mode for fulfillment.
    ///
    /// This mode is more conservative and ensures more provers can fulfill the request.
    ///
    /// Sets the ramp up period as 10x the executor time assuming the executor speed is 30 MHz.
    /// Sets the lock timeout as the sum of the ramp up period and the proving and executor times
    /// assuming the proving speed is 500 kHz and the executor speed is 30 MHz and capped at 4 hours.
    /// Sets the timeout as 2 times the lock timeout and capped at 8 hours.
    pub fn fulfillment() -> Self {
        Self {
            proving_speed: Self::DEFAULT_PROVING_SPEED_HZ,
            executor_speed: Self::DEFAULT_EXECUTOR_SPEED_HZ,
            min_timeout: Self::DEFAULT_MIN_TIMEOUT,
            ramp_up_period_multiplier: Self::DEFAULT_RAMP_UP_PERIOD_MULTIPLIER,
            ramp_up_delay_multiplier: Self::DEFAULT_RAMP_UP_DELAY_MULTIPLIER,
            base_ramp_up_period: Self::DEFAULT_BASE_RAMP_UP_PERIOD,
        }
    }

    /// Creates a parameterization mode for low latency.
    ///
    /// This mode is more aggressive and allows for faster fulfillment,
    /// at the cost of higher prices and lower fulfillment guarantees.
    ///
    /// Sets the ramp up period as 5x the executor time assuming the executor speed is 50 MHz.
    /// Sets the lock timeout as the sum of the ramp up period and the proving and executor times
    /// assuming the proving speed is 3 MHz and the executor speed is 50 MHz and capped at 4 hours.
    /// Sets the timeout as 2 times the lock timeout and capped at 8 hours.
    pub fn latency() -> Self {
        Self {
            proving_speed: Self::FAST_PROVING_SPEED_HZ,
            executor_speed: Self::FAST_EXECUTOR_SPEED_HZ,
            min_timeout: Self::FAST_MIN_TIMEOUT,
            ramp_up_period_multiplier: Self::FAST_RAMP_UP_PERIOD_MULTIPLIER,
            ramp_up_delay_multiplier: Self::FAST_RAMP_UP_DELAY_MULTIPLIER,
            base_ramp_up_period: Self::FAST_BASE_RAMP_UP_PERIOD,
        }
    }

    /// Calculates the recommended ramp up start based on the cycle count and speeds.
    ///
    /// The ramp up start is calculated as the current timestamp plus the required executor time multiplied by the ramp up delay multiplier.
    fn recommended_ramp_up_start(&self, cycle_count: Option<u64>) -> u64 {
        cycle_count
            .filter(|&count| count > 0)
            .map(|cycle_count| {
                now_timestamp()
                    + (self.executor_time(Some(cycle_count)) as u64 * self.ramp_up_delay_multiplier)
            })
            .unwrap_or(now_timestamp() + 15) // 15 seconds default
    }

    /// Calculates the recommended ramp up period based on the cycle count and speeds.
    ///
    /// The ramp up period is calculated as the base ramp up period plus the required executor time multiplied by the ramp up period multiplier.
    /// The ramp up period is capped at 2 hours to prevent the ramp up period from being too long.
    fn recommended_ramp_up_period(&self, cycle_count: Option<u64>) -> u32 {
        const MAX_RAMP_UP_PERIOD: u32 = 7200; // 2 hours
        cycle_count
            .filter(|&count| count > 0)
            .map(|cycle_count| {
                // MIN(BASE_RAMP_UP_PERIOD + (executor_time * ramp_up_period_multiplier), MAX_RAMP_UP_PERIOD)
                let ramp_up_period =
                    self.executor_time(Some(cycle_count)) * self.ramp_up_period_multiplier;
                let base_ramp_up_period = self.base_ramp_up_period;
                let max_ramp_up_period = MAX_RAMP_UP_PERIOD;
                base_ramp_up_period.saturating_add(ramp_up_period).min(max_ramp_up_period)
            })
            .unwrap_or(self.base_ramp_up_period)
    }

    /// Calculates the recommended ramp up delay based on the cycle count and speeds.
    /// Calculates the recommended timeout based on the cycle count and speeds.
    ///
    /// The timeout is calculated as the sum of:
    /// - Time required for proving: `cycle_count / proving_speed`
    /// - Time required for execution: `cycle_count / executor_speed`
    ///
    /// # Notes
    /// The timeout is guaranteed to be at least [self.min_timeout] seconds.
    fn recommended_timeout(&self, cycle_count: Option<u64>) -> u32 {
        cycle_count
            .filter(|&count| count > 0)
            .map(|cycle_count| {
                let required_proving_time = self.proving_time(Some(cycle_count));
                let required_executor_time = self.executor_time(Some(cycle_count));
                let timeout = required_proving_time.saturating_add(required_executor_time);
                timeout.max(self.min_timeout)
            })
            .unwrap_or(DEFAULT_TIMEOUT)
    }

    /// Calculates the required proving time based on the cycle count and speeds.
    ///
    /// The proving time is calculated as the cycle count divided by the proving speed.
    fn proving_time(&self, cycle_count: Option<u64>) -> u32 {
        cycle_count
            .filter(|&count| count > 0)
            .map(|cycle_count| cycle_count.div_ceil(self.proving_speed) as u32)
            .unwrap_or(0)
    }
    /// Calculates the required executor time based on the cycle count and speeds.
    ///
    /// The executor time is calculated as the cycle count divided by the executor speed.
    fn executor_time(&self, cycle_count: Option<u64>) -> u32 {
        cycle_count
            .filter(|&count| count > 0)
            .map(|cycle_count| cycle_count.div_ceil(self.executor_speed) as u32)
            .unwrap_or(0)
    }
}

impl Default for ParameterizationMode {
    fn default() -> Self {
        Self {
            proving_speed: Self::DEFAULT_PROVING_SPEED_HZ,
            executor_speed: Self::DEFAULT_EXECUTOR_SPEED_HZ,
            min_timeout: Self::DEFAULT_MIN_TIMEOUT,
            ramp_up_period_multiplier: Self::DEFAULT_RAMP_UP_PERIOD_MULTIPLIER,
            ramp_up_delay_multiplier: Self::DEFAULT_RAMP_UP_DELAY_MULTIPLIER,
            base_ramp_up_period: Self::DEFAULT_BASE_RAMP_UP_PERIOD,
        }
    }
}

#[cfg(test)]
mod parameterization_mode_tests {
    use crate::request_builder::offer_layer::DEFAULT_TIMEOUT;

    use super::ParameterizationMode;

    #[test]
    fn test_default_creation() {
        let mode = ParameterizationMode::default();
        assert_eq!(mode.proving_speed, ParameterizationMode::DEFAULT_PROVING_SPEED_HZ);
        assert_eq!(mode.executor_speed, ParameterizationMode::DEFAULT_EXECUTOR_SPEED_HZ);
        assert_eq!(mode.min_timeout, ParameterizationMode::DEFAULT_MIN_TIMEOUT);
        assert_eq!(
            mode.ramp_up_period_multiplier,
            ParameterizationMode::DEFAULT_RAMP_UP_PERIOD_MULTIPLIER
        );
    }

    #[test]
    fn test_fulfillment_creation() {
        let mode = ParameterizationMode::fulfillment();
        assert_eq!(mode.proving_speed, ParameterizationMode::DEFAULT_PROVING_SPEED_HZ);
        assert_eq!(mode.executor_speed, ParameterizationMode::DEFAULT_EXECUTOR_SPEED_HZ);
        assert_eq!(mode.min_timeout, ParameterizationMode::DEFAULT_MIN_TIMEOUT);
        assert_eq!(
            mode.ramp_up_period_multiplier,
            ParameterizationMode::DEFAULT_RAMP_UP_PERIOD_MULTIPLIER
        );
    }

    #[test]
    fn test_latency_creation() {
        let mode = ParameterizationMode::latency();
        assert_eq!(mode.proving_speed, ParameterizationMode::FAST_PROVING_SPEED_HZ);
        assert_eq!(mode.executor_speed, ParameterizationMode::FAST_EXECUTOR_SPEED_HZ);
        assert_eq!(mode.min_timeout, ParameterizationMode::FAST_MIN_TIMEOUT);
        assert_eq!(
            mode.ramp_up_period_multiplier,
            ParameterizationMode::FAST_RAMP_UP_PERIOD_MULTIPLIER
        );
    }

    #[test]
    fn test_recommended_timeout_default() {
        let mode = ParameterizationMode::default();

        // Test with a small cycle count
        let cycle_count = 1_000_000; // 1M cycles
        let timeout = mode.recommended_timeout(Some(cycle_count));

        // Expected: (1_000_000 / (500 * 1000)) + (1_000_000 / (30000 * 1000))
        // = (1_000_000 / 500_000) + (1_000_000 / 30_000_000)
        // = 2 + 1 = 3 seconds, but should be at least MIN_TIMEOUT (30)
        assert_eq!(timeout, mode.min_timeout);

        // Test with a larger cycle count that exceeds MIN_TIMEOUT
        let cycle_count = 50_000_000; // 50M cycles
        let timeout = mode.recommended_timeout(Some(cycle_count));

        // Expected: (50_000_000 / 500_000) + (50_000_000 / 30_000_000)
        // = 100 + 2 = 102 seconds
        assert_eq!(timeout, 102);
    }

    #[test]
    fn test_recommended_timeout_latency() {
        let mode = ParameterizationMode::latency();

        // Test with a small cycle count
        let cycle_count = 1_000_000; // 1M cycles
        let timeout = mode.recommended_timeout(Some(cycle_count as u64));

        // Expected: (1_000_000 / (3000 * 1000)) + (1_000_000 / (50000 * 1000))
        // = (1_000_000 / 3_000_000) + (1_000_000 / 50_000_000)
        // = 1 + 1 = 2 seconds, but should be at least MIN_TIMEOUT (30)
        assert_eq!(timeout, mode.min_timeout);

        // Test with a larger cycle count
        let cycle_count = 50_000_000; // 50M cycles
        let timeout = mode.recommended_timeout(Some(cycle_count as u64));

        // Expected: (50_000_000 / 3_000_000) + (50_000_000 / 50_000_000)
        // = 17 + 1 = 18 seconds, but should be at least MIN_TIMEOUT (30)
        assert_eq!(timeout, mode.min_timeout);

        // Test with a very large cycle count that exceeds MIN_TIMEOUT
        let cycle_count = 200_000_000; // 200M cycles
        let timeout = mode.recommended_timeout(Some(cycle_count as u64));

        // Expected: (200_000_000 / 3_000_000) + (200_000_000 / 50_000_000)
        // = 67 + 4 = 71 seconds
        assert_eq!(timeout, 71);
    }

    #[test]
    fn test_recommended_timeout_minimum() {
        let mode = ParameterizationMode::default();

        // Test with zero cycles - should return DEFAULT_TIMEOUT
        let timeout = mode.recommended_timeout(Some(0));
        assert_eq!(timeout, DEFAULT_TIMEOUT);

        // Test with very small cycle count - should return MIN_TIMEOUT
        let timeout = mode.recommended_timeout(Some(100));
        assert_eq!(timeout, mode.min_timeout);
    }

    #[test]
    fn test_recommended_ramp_up_period_default() {
        let mode = ParameterizationMode::default();

        // Test with 1M cycles
        let cycle_count = 1_000_000;
        let ramp_up = mode.recommended_ramp_up_period(Some(cycle_count as u64));

        // Expected: (1_000_000 / (30000 * 1000)) * 10 + 300
        // = (1_000_000 / 30_000_000) * 10 + 300
        // = 1 * 10 + 300 = 310 seconds
        assert_eq!(ramp_up, 310);

        // Test with 50M cycles
        let cycle_count = 50_000_000;
        let ramp_up = mode.recommended_ramp_up_period(Some(cycle_count as u64));

        // Expected: (50_000_000 / 30_000_000) * 10 + 300
        // = 2 * 10 + 300 = 320 seconds
        assert_eq!(ramp_up, 320);
    }

    #[test]
    fn test_recommended_ramp_up_period_fast() {
        let mode = ParameterizationMode::latency();

        // Test with 1M cycles
        let cycle_count = 1_000_000;
        let ramp_up = mode.recommended_ramp_up_period(Some(cycle_count as u64));

        // Expected: (1_000_000 / (50000 * 1000)) * 5 + 60
        // = (1_000_000 / 50_000_000) * 5 + 60
        // = 1 * 5 + 60 = 65 seconds
        assert_eq!(ramp_up, 65);

        // Test with 50M cycles
        let cycle_count = 50_000_000;
        let ramp_up = mode.recommended_ramp_up_period(Some(cycle_count as u64));

        // Expected: (50_000_000 / 50_000_000) * 5 + 60
        // = 1 * 5 + 60 = 65 seconds
        assert_eq!(ramp_up, 65);
    }

    #[test]
    fn test_recommended_ramp_up_period_zero_cycles() {
        let mode = ParameterizationMode::default();

        // Test with zero cycles
        let ramp_up = mode.recommended_ramp_up_period(Some(0));
        assert_eq!(ramp_up, ParameterizationMode::DEFAULT_BASE_RAMP_UP_PERIOD);
    }

    #[test]
    fn test_latency_vs_fulfillment_timeout_comparison() {
        let latency_mode = ParameterizationMode::latency();
        let fulfillment_mode = ParameterizationMode::fulfillment();

        let cycle_count = 100_000_000; // 100M cycles

        let latency_timeout = latency_mode.recommended_timeout(Some(cycle_count as u64));
        let fulfillment_timeout = fulfillment_mode.recommended_timeout(Some(cycle_count as u64));

        // Latency mode should generally result in shorter timeouts (when above MIN_TIMEOUT)
        // For this cycle count, both should be above MIN_TIMEOUT
        if latency_timeout > latency_mode.min_timeout
            && fulfillment_timeout > fulfillment_mode.min_timeout
        {
            assert!(
                latency_timeout < fulfillment_timeout,
                "Latency mode should have shorter timeout: latency={}, fulfillment={}",
                latency_timeout,
                fulfillment_timeout
            );
        }
    }

    #[test]
    fn test_recommended_ramp_up_start_default() {
        let mode = ParameterizationMode::default();
        let now = crate::util::now_timestamp();

        // Test with zero cycles - should return now + 15
        let ramp_up_start = mode.recommended_ramp_up_start(Some(0));
        assert!(ramp_up_start >= now + 15);
        assert!(ramp_up_start <= now + 16); // Allow 1 second tolerance

        // Test with None cycles - should return now + 15
        let ramp_up_start = mode.recommended_ramp_up_start(None);
        assert!(ramp_up_start >= now + 15);
        assert!(ramp_up_start <= now + 16); // Allow 1 second tolerance

        // Test with 1M cycles
        let cycle_count = 1_000_000;
        let executor_time = mode.executor_time(Some(cycle_count));
        let expected_delay = executor_time as u64 * mode.ramp_up_delay_multiplier;
        let ramp_up_start = mode.recommended_ramp_up_start(Some(cycle_count));
        assert!(ramp_up_start >= now + expected_delay);
        assert!(ramp_up_start <= now + expected_delay + 1); // Allow 1 second tolerance

        // Test with 50M cycles
        let cycle_count = 50_000_000;
        let executor_time = mode.executor_time(Some(cycle_count));
        let expected_delay = executor_time as u64 * mode.ramp_up_delay_multiplier;
        let ramp_up_start = mode.recommended_ramp_up_start(Some(cycle_count));
        assert!(ramp_up_start >= now + expected_delay);
        assert!(ramp_up_start <= now + expected_delay + 1); // Allow 1 second tolerance
    }

    #[test]
    fn test_recommended_ramp_up_start_latency() {
        let mode = ParameterizationMode::latency();
        let now = crate::util::now_timestamp();

        // Test with zero cycles - should return now + 15
        let ramp_up_start = mode.recommended_ramp_up_start(Some(0));
        assert!(ramp_up_start >= now + 15);
        assert!(ramp_up_start <= now + 16); // Allow 1 second tolerance

        // Test with 1M cycles
        let cycle_count = 1_000_000;
        let executor_time = mode.executor_time(Some(cycle_count));
        let expected_delay = executor_time as u64 * mode.ramp_up_delay_multiplier;
        let ramp_up_start = mode.recommended_ramp_up_start(Some(cycle_count));
        assert!(ramp_up_start >= now + expected_delay);
        assert!(ramp_up_start <= now + expected_delay + 1); // Allow 1 second tolerance

        // Test with 50M cycles
        let cycle_count = 50_000_000;
        let executor_time = mode.executor_time(Some(cycle_count));
        let expected_delay = executor_time as u64 * mode.ramp_up_delay_multiplier;
        let ramp_up_start = mode.recommended_ramp_up_start(Some(cycle_count));
        assert!(ramp_up_start >= now + expected_delay);
        assert!(ramp_up_start <= now + expected_delay + 1); // Allow 1 second tolerance
    }

    #[test]
    fn test_recommended_ramp_up_start_fulfillment_vs_latency() {
        let fulfillment_mode = ParameterizationMode::fulfillment();
        let latency_mode = ParameterizationMode::latency();
        let now = crate::util::now_timestamp();

        let cycle_count = 100_000_000; // 100M cycles

        let fulfillment_start = fulfillment_mode.recommended_ramp_up_start(Some(cycle_count));
        let latency_start = latency_mode.recommended_ramp_up_start(Some(cycle_count));

        // Both should be in the future
        assert!(fulfillment_start > now);
        assert!(latency_start > now);

        // Fulfillment mode uses a larger delay multiplier (2x) vs latency (1x),
        // and fulfillment has slower executor speed, so fulfillment should have a later start
        let fulfillment_executor_time = fulfillment_mode.executor_time(Some(cycle_count));
        let latency_executor_time = latency_mode.executor_time(Some(cycle_count));
        let fulfillment_delay =
            fulfillment_executor_time as u64 * fulfillment_mode.ramp_up_delay_multiplier;
        let latency_delay = latency_executor_time as u64 * latency_mode.ramp_up_delay_multiplier;

        // Fulfillment should have a longer delay (2x multiplier vs 1x, and slower executor)
        assert!(
            fulfillment_delay > latency_delay,
            "Fulfillment mode should have longer ramp up delay: fulfillment={}, latency={}",
            fulfillment_delay,
            latency_delay
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use alloy::{
        network::TransactionBuilder,
        node_bindings::Anvil,
        primitives::Address,
        providers::{DynProvider, Provider},
        rpc::types::TransactionRequest,
    };
    use boundless_test_utils::{guests::ECHO_ELF, market::create_test_ctx};
    use tracing_test::traced_test;
    use url::Url;

    use super::{
        Layer, OfferLayer, OfferLayerConfig, OfferParams, ParameterizationMode, PreflightLayer,
        RequestBuilder, RequestId, RequestIdLayer, RequestIdLayerConfig, RequestIdLayerMode,
        RequestParams, RequirementsLayer, StandardRequestBuilder, StorageLayer, StorageLayerConfig,
    };

    use crate::{
        contracts::{
            boundless_market::BoundlessMarketService, FulfillmentData, Predicate, RequestInput,
            RequestInputType, Requirements,
        },
        input::GuestEnv,
        request_builder::offer_layer::DEFAULT_TIMEOUT,
        storage::{fetch_url, MockStorageProvider, StorageProvider},
        util::NotProvided,
        StandardStorageProvider,
    };
    use alloy_primitives::{utils::parse_ether, U256};
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
    async fn offer_layer_lock_collateral_default() -> anyhow::Result<()> {
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
                OfferLayerConfig::builder().build()?,
            ))
            .request_id_layer(market)
            .build()?;

        let params = request_builder.params().with_program(ECHO_ELF).with_stdin(b"hello!");
        let request = request_builder.build(params).await?;
        assert_eq!(request.offer.lockCollateral, parse_ether("0").unwrap());
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
                OfferLayerConfig::builder()
                    .ramp_up_period(27)
                    .lock_collateral(parse_ether("10").unwrap())
                    .build()?,
            ))
            .request_id_layer(market)
            .build()?;

        let params = request_builder.params().with_program(ECHO_ELF).with_stdin(b"hello!");
        let request = request_builder.build(params).await?;
        assert_eq!(request.offer.rampUpPeriod, 27);
        assert_eq!(request.offer.lockCollateral, parse_ether("10").unwrap());
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
        let predicate = Predicate::try_from(request.requirements.predicate.clone())?;
        assert_eq!(predicate.image_id().unwrap(), risc0_zkvm::compute_image_id(ECHO_ELF)?);
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
        assert_eq!(request_input.inputType, RequestInputType::Inline);
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
        assert_eq!(request_input.inputType, RequestInputType::Inline);
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
        assert_eq!(request_input.inputType, RequestInputType::Url);
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
        let predicate = Predicate::try_from(req.predicate.clone())?;
        let fulfillment_data = FulfillmentData::from_image_id_and_journal(
            predicate.image_id().unwrap(),
            journal.bytes.clone(),
        );
        // Predicate should match the same journal
        assert!(predicate.eval(&fulfillment_data).is_some());
        // And should not match different data
        let other = Journal::new(b"other_data".to_vec());
        let fulfillment_data = FulfillmentData::from_image_id_and_journal(
            predicate.image_id().unwrap(),
            other.bytes.clone(),
        );
        assert!(predicate.eval(&fulfillment_data).is_none());
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
        let predicate = Predicate::digest_match(image_id, Journal::new(b"hello".to_vec()).digest());
        let requirements = Requirements::new(predicate);
        let request_id = RequestId::new(test_ctx.customer_signer.address(), 0);

        // Zero cycles
        let offer_params = OfferParams::default();
        let now = crate::util::now_timestamp();
        let offer_zero_mcycles =
            layer.process((&requirements, &request_id, Some(0u64), &offer_params)).await?;
        assert_eq!(offer_zero_mcycles.minPrice, U256::ZERO);
        // Defaults from builder
        assert_eq!(
            offer_zero_mcycles.rampUpPeriod,
            ParameterizationMode::DEFAULT_BASE_RAMP_UP_PERIOD
        );
        assert_eq!(
            offer_zero_mcycles.lockTimeout,
            DEFAULT_TIMEOUT + ParameterizationMode::DEFAULT_BASE_RAMP_UP_PERIOD
        );
        assert_eq!(
            offer_zero_mcycles.timeout,
            (DEFAULT_TIMEOUT + ParameterizationMode::DEFAULT_BASE_RAMP_UP_PERIOD) * 2
                - ParameterizationMode::DEFAULT_BASE_RAMP_UP_PERIOD
        );
        // Default ramp up start should be now + 15 seconds
        assert!(offer_zero_mcycles.rampUpStart >= now + 15);
        assert!(offer_zero_mcycles.rampUpStart <= now + 16); // Allow 1 second tolerance
                                                             // Max price should be non-negative, to account for fixed costs.
        assert!(offer_zero_mcycles.maxPrice > U256::ZERO);

        // Now create an offer for 100 Mcycles.
        let offer_more_mcycles =
            layer.process((&requirements, &request_id, Some(100u64 << 20), &offer_params)).await?;
        assert!(offer_more_mcycles.maxPrice > offer_zero_mcycles.maxPrice);

        // Check that overrides are respected.
        let min_price = U256::from(1u64);
        let max_price = U256::from(5u64);
        let bidding_start = now + 100;
        let offer_params = OfferParams::builder()
            .max_price(max_price)
            .min_price(min_price)
            .bidding_start(bidding_start)
            .ramp_up_period(20)
            .lock_timeout(50)
            .timeout(80)
            .into();
        let offer_zero_mcycles =
            layer.process((&requirements, &request_id, Some(0u64), &offer_params)).await?;
        assert_eq!(offer_zero_mcycles.maxPrice, max_price);
        assert_eq!(offer_zero_mcycles.minPrice, min_price);
        assert_eq!(offer_zero_mcycles.rampUpPeriod, 20);
        assert_eq!(offer_zero_mcycles.lockTimeout, 50);
        assert_eq!(offer_zero_mcycles.timeout, 80);
        assert_eq!(offer_zero_mcycles.rampUpStart, bidding_start);
        Ok(())
    }

    #[tokio::test]
    #[traced_test]
    async fn test_offer_layer_with_parameterization_mode() -> anyhow::Result<()> {
        // Use Anvil-backed provider for gas price
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await?;
        let provider = test_ctx.customer_provider.clone();

        // Build minimal requirements and request ID
        let image_id = compute_image_id(ECHO_ELF).unwrap();
        let predicate = Predicate::digest_match(image_id, Journal::new(b"hello".to_vec()).digest());
        let requirements = Requirements::new(predicate);
        let request_id = RequestId::new(test_ctx.customer_signer.address(), 0);

        // Test with fulfillment mode
        let fulfillment_mode = ParameterizationMode::fulfillment();
        let layer = OfferLayer::new(
            provider.clone(),
            OfferLayerConfig::builder().parameterization_mode(fulfillment_mode).build()?,
        );
        let now = crate::util::now_timestamp();
        let cycle_count = 100_000_000; // 100M cycles
        let offer_params = OfferParams::default();
        let offer =
            layer.process((&requirements, &request_id, Some(cycle_count), &offer_params)).await?;

        // Check that ramp up start is calculated based on parameterization mode
        let expected_executor_time = fulfillment_mode.executor_time(Some(cycle_count));
        let expected_delay =
            expected_executor_time as u64 * fulfillment_mode.ramp_up_delay_multiplier;
        assert!(offer.rampUpStart >= now + expected_delay);
        assert!(offer.rampUpStart <= now + expected_delay + 1); // Allow 1 second tolerance

        // Check that ramp up period is calculated based on parameterization mode
        let expected_ramp_up_period =
            fulfillment_mode.recommended_ramp_up_period(Some(cycle_count));
        assert_eq!(offer.rampUpPeriod, expected_ramp_up_period);

        // Test with latency mode
        let latency_mode = ParameterizationMode::latency();
        let layer = OfferLayer::new(
            provider.clone(),
            OfferLayerConfig::builder().parameterization_mode(latency_mode).build()?,
        );
        let offer_latency =
            layer.process((&requirements, &request_id, Some(cycle_count), &offer_params)).await?;

        // Latency mode should have a shorter ramp up start delay
        let latency_executor_time = latency_mode.executor_time(Some(cycle_count));
        let latency_delay = latency_executor_time as u64 * latency_mode.ramp_up_delay_multiplier;
        assert!(offer_latency.rampUpStart >= now + latency_delay);
        assert!(offer_latency.rampUpStart <= now + latency_delay + 1); // Allow 1 second tolerance

        // Latency mode should have a shorter ramp up period
        let expected_latency_ramp_up_period =
            latency_mode.recommended_ramp_up_period(Some(cycle_count));
        assert_eq!(offer_latency.rampUpPeriod, expected_latency_ramp_up_period);

        // Fulfillment mode should have a later ramp up start than latency mode
        assert!(
            offer.rampUpStart > offer_latency.rampUpStart,
            "Fulfillment mode should have later ramp up start: fulfillment={}, latency={}",
            offer.rampUpStart,
            offer_latency.rampUpStart
        );

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

    #[test]
    fn request_params_with_input_url_infallible() {
        // When passing a parsed URL, with_input_url should be infallible.
        // NOTE: The `match *e {}` incantation is a compile-time assert that this error cannot
        // occur.
        let url = Url::parse("https://fileserver.example/input.bin").unwrap();
        RequestParams::new().with_input_url(url).inspect_err(|e| match *e {}).unwrap();
    }

    #[test]
    fn test_with_input_url() {
        // Test with string URL
        let params =
            RequestParams::new().with_input_url("https://fileserver.example/input.bin").unwrap();

        let input = params.request_input.unwrap();
        assert_eq!(input.inputType, RequestInputType::Url);
        assert_eq!(input.data.as_ref(), "https://fileserver.example/input.bin".as_bytes());

        // Test with parsed URL
        let url = Url::parse("https://fileserver.example/input2.bin").unwrap();
        let params = RequestParams::new().with_input_url(url).unwrap();

        let input = params.request_input.unwrap();
        assert_eq!(input.inputType, RequestInputType::Url);
        assert_eq!(input.data.as_ref(), "https://fileserver.example/input2.bin".as_bytes());
    }

    #[allow(dead_code)]
    trait AssertSend: Send {}

    // The StandardRequestBuilder must be Send such that a Client can be sent between threads.
    impl AssertSend for StandardRequestBuilder<DynProvider, StandardStorageProvider> {}
}
