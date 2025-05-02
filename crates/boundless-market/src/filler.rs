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

#![allow(missing_docs)] // DO NOT MERGE: That would be too lazy
#![allow(async_fn_in_trait)] // DO NOT MERGE: Consider alternatives.

// TODO: Add debug and trace logging to the layers.
// TODO: Create a test and an example of adding a layer.

use std::{borrow::Cow, convert::Infallible, rc::Rc};

use alloy::{
    network::Ethereum,
    primitives::{
        utils::{format_units, Unit},
        U256,
    },
    providers::{DynProvider, Provider},
};
use anyhow::{bail, Context};
use derive_builder::Builder;
use risc0_zkvm::{
    compute_image_id, default_executor, sha::Digestible, Executor, Journal, SessionInfo,
};
use url::Url;

use crate::{
    contracts::{
        boundless_market::BoundlessMarketService, Input as RequestInput, InputType, Offer,
        Predicate, ProofRequest, RequestId, Requirements,
    },
    input::GuestEnv,
    now_timestamp,
    selector::{ProofType, SupportedSelectors},
    storage::{fetch_url, BuiltinStorageProvider, StorageProvider},
};

// Idea: A pipeline like construction where each output must be (convertable to) the input to the
// next stage.
// E.g. (program, input_data) -> StorageLayer -> (program_url, input) -> PreflightLayer ->
// (program_url, input, journal, cycles) -> OfferLayer ->
// (program_url, input, journal, cycles, offer) -> RequestIdLayer ->
// (program_url, input, journal, cycles, offer, id) -> Finalizer -> request
//
// In many ways, this is just how software is built: one component passing to the next. This
// modular structure is only justified if
// A) The consuming devloper will need to change out the implementation, and
// B) The layer itself is something they need to be able to define (i.e. skip or remove a layer,
// combine two layers, or break two layers apart) and we do not feel that we can dictate the
// interfaces.
//
// NOTE: There is an issue with this model in there and a lot of input types and they tend to grow
// larger as you go further down the pipeline. In some layers, they may be forced to accept a more
// complicated input because otherwise the _next_ layer won't have the data it needs.

// TODO: Should the self-ref be mut?

pub trait RequestBuilder {
    type Params;
    /// Error type that may be returned by this filler.
    type Error;

    // NOTE: Takes the self receiver so that the caller does not need to explicitly name the
    // RequestBuilder type (e.g. `<MyRequestBuilder as RequestBuilder>::default_params()`)
    fn default_params(&self) -> Self::Params
    where
        Self::Params: Default,
    {
        Default::default()
    }

    async fn build(&self, params: impl Into<Self::Params>) -> Result<ProofRequest, Self::Error>;
}

pub trait Layer<Input> {
    type Output;
    /// Error type that may be returned by this layer.
    type Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error>;
}

pub trait Adapt<L> {
    type Output;
    type Error;

    async fn process_with(self, layer: &L) -> Result<Self::Output, Self::Error>;
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

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct StorageLayer<S: StorageProvider> {
    /// Maximum number of bytes to send as an inline input.
    ///
    /// Inputs larger than this size will be uploaded using the given storage provider. Set to none
    /// to indicate that inputs should always be sent inline.
    #[builder(setter(into), default = "Some(2048)")]
    pub inline_input_max_bytes: Option<usize>,
    #[builder(setter(into))]
    pub storage_provider: S,
}

impl<S: StorageProvider> Layer<(&[u8], &GuestEnv)> for StorageLayer<S>
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = anyhow::Error;
    type Output = (Url, RequestInput);

    async fn process(
        &self,
        (program, env): (&[u8], &GuestEnv),
    ) -> Result<Self::Output, Self::Error> {
        let program_url = self.storage_provider.upload_program(program).await?;
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                RequestInput::url(self.storage_provider.upload_input(&input_data).await?)
            }
            _ => RequestInput::inline(input_data),
        };
        Ok((program_url, request_input))
    }
}

impl<S> Default for StorageLayer<S>
where
    S: StorageProvider + Default,
{
    fn default() -> Self {
        Self {
            // Default max inline input size is 2 kB.
            inline_input_max_bytes: Some(2048),
            storage_provider: S::default(),
        }
    }
}

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct PreflightLayer {
    #[builder(setter(into), default = "default_executor()")]
    executor: Rc<dyn Executor>,
}

impl PreflightLayer {
    pub fn builder() -> PreflightLayerBuilder {
        Default::default()
    }

    async fn fetch_env(&self, input: &RequestInput) -> anyhow::Result<GuestEnv> {
        let env = match input.inputType {
            InputType::Inline => GuestEnv::decode(&input.data)?,
            InputType::Url => {
                let input_url =
                    std::str::from_utf8(&input.data).context("Input URL is not valid UTF-8")?;
                tracing::info!("Fetching input from {}", input_url);
                GuestEnv::decode(&fetch_url(&input_url).await?)?
            }
            _ => bail!("Unsupported input type"),
        };
        Ok(env)
    }
}

impl Default for PreflightLayer {
    fn default() -> Self {
        Self { executor: default_executor() }
    }
}

impl Layer<(&Url, &RequestInput)> for PreflightLayer {
    type Output = SessionInfo;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (program_url, input): (&Url, &RequestInput),
    ) -> anyhow::Result<Self::Output> {
        let program = fetch_url(program_url).await?;
        let env = self.fetch_env(input).await?;
        let session_info = self.executor.execute(env.try_into()?, &program)?;
        Ok(session_info)
    }
}

#[non_exhaustive]
#[derive(Clone, Builder, Default)]
pub struct RequirementsLayer {}

impl RequirementsLayer {
    pub fn builder() -> RequirementsLayerBuilder {
        Default::default()
    }
}

impl Layer<(&[u8], &Journal)> for RequirementsLayer {
    type Output = Requirements;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (program, journal): (&[u8], &Journal),
    ) -> Result<Self::Output, Self::Error> {
        let image_id =
            compute_image_id(program).context("failed to compute image ID for program")?;
        Ok(Requirements::new(image_id, Predicate::digest_match(journal.digest())))
    }
}

#[non_exhaustive]
#[derive(Copy, Clone, Debug, Default)]
pub enum RequestIdLayerMode {
    #[default]
    Rand,
    Nonce,
}

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct RequestIdLayer<P> {
    pub boundless_market: BoundlessMarketService<P>,
    #[builder(default)]
    pub mode: RequestIdLayerMode,
}

impl<P> RequestIdLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub fn builder() -> RequestIdLayerBuilder<P> {
        Default::default()
    }
}

impl<P> Layer<()> for RequestIdLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = RequestId;
    type Error = anyhow::Error;

    async fn process(&self, (): ()) -> anyhow::Result<Self::Output> {
        let id_u256 = match self.mode {
            RequestIdLayerMode::Nonce => self.boundless_market.request_id_from_nonce().await?,
            RequestIdLayerMode::Rand => self.boundless_market.request_id_from_rand().await?,
        };
        Ok(id_u256.try_into().expect("generated request ID should always be valid"))
    }
}

#[non_exhaustive]
#[derive(Builder)]
pub struct OfferLayer<P> {
    pub provider: P,
    // default: 0 ETH
    #[builder(setter(into), default = "U256::ZERO")]
    pub min_price_per_mcycle: U256,
    // default: 0.0001 ETH
    #[builder(setter(into), default = "U256::from(100) * Unit::TWEI.wei_const()")]
    pub max_price_per_mcycle: U256,
    // default: 15 seconds
    #[builder(default = "15")]
    pub bidding_start_delay: u64,
    // default: 120 seconds
    #[builder(default = "120")]
    pub ramp_up_period: u32,
    // default: 600 seconds
    #[builder(default = "600")]
    pub lock_timeout: u32,
    // default: 1200 seconds
    #[builder(default = "1200")]
    pub timeout: u32,
    // default: 0.1 HP
    #[builder(setter(into), default = "U256::from(100) * Unit::PWEI.wei_const()")]
    pub lock_stake: U256,
    // default: 200_000
    #[builder(default = "200_000")]
    pub lock_gas_estimate: u64,
    // default: 750_000
    #[builder(default = "750_000")]
    pub fulfill_gas_estimate: u64,
    // default: 250_000
    #[builder(default = "250_000")]
    pub groth16_verify_gas_estimate: u64,
    // default: 100_000
    #[builder(default = "100_000")]
    pub smart_contract_sig_verify_gas_estimate: u64,
    #[builder(setter(into), default)]
    pub supported_selectors: SupportedSelectors,
}

impl<P> OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    pub fn builder() -> OfferLayerBuilder<P> {
        Default::default()
    }

    fn estimate_gas_usage(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
    ) -> anyhow::Result<u64> {
        let mut gas_usage_estimate = self.lock_gas_estimate + self.fulfill_gas_estimate;
        if request_id.smart_contract_signed {
            gas_usage_estimate += self.smart_contract_sig_verify_gas_estimate;
        }
        // Add gas for orders that make use of the callbacks feature.
        if let Some(callback) = requirements.callback.as_option() {
            gas_usage_estimate +=
                u64::try_from(callback.gasLimit).context("callback gas limit too large for u64")?;
        }

        let proof_type = self
            .supported_selectors
            .proof_type(requirements.selector)
            .context("cannot estimate gas usage for request with unsupported selector")?;
        if let ProofType::Groth16 = proof_type {
            gas_usage_estimate += self.groth16_verify_gas_estimate;
        };
        Ok(gas_usage_estimate)
    }

    fn estimate_gas_cost(
        &self,
        requirements: &Requirements,
        request_id: &RequestId,
        gas_price: u128,
    ) -> anyhow::Result<U256> {
        let gas_usage_estimate = self.estimate_gas_usage(requirements, request_id)?;

        // Add to the max price an estimated upper bound on the gas costs.
        // Add a 10% buffer to the gas costs to account for flucuations after submission.
        let gas_cost_estimate = (gas_price + (gas_price / 10)) * (gas_usage_estimate as u128);
        Ok(U256::from(gas_cost_estimate))
    }
}

impl<P> Layer<(&Requirements, &RequestId, u64)> for OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Offer;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (requirements, request_id, cycle_count): (&Requirements, &RequestId, u64),
    ) -> anyhow::Result<Self::Output> {
        let mcycle_count = cycle_count >> 20;
        let min_price = self.min_price_per_mcycle * U256::from(mcycle_count);
        let max_price_mcycle = self.max_price_per_mcycle * U256::from(mcycle_count);

        // TODO: Use EIP-1559 parameters to select a better max price.
        let gas_price: u128 = self.provider.get_gas_price().await?;
        let gas_cost_estimate = self.estimate_gas_cost(requirements, request_id, gas_price)?;
        let max_price = max_price_mcycle + gas_cost_estimate;
        tracing::debug!(
            "Setting a max price of {} ether: {} mcycle_price + {} gas_cost_estimate",
            format_units(max_price, "ether")?,
            format_units(max_price_mcycle, "ether")?,
            format_units(gas_cost_estimate, "ether")?,
        );

        Ok(Offer {
            minPrice: min_price,
            maxPrice: max_price,
            biddingStart: now_timestamp() + self.bidding_start_delay,
            rampUpPeriod: self.ramp_up_period,
            lockTimeout: self.lock_timeout,
            timeout: self.timeout,
            lockStake: self.lock_stake,
        })
    }
}

#[non_exhaustive]
#[derive(Clone, Builder, Default)]
pub struct Finalizer {}

impl Finalizer {
    pub fn builder() -> FinalizerBuilder {
        Default::default()
    }
}

pub type FinalizerInput = (Url, RequestInput, Requirements, Offer, RequestId);

impl Layer<FinalizerInput> for Finalizer {
    type Output = ProofRequest;
    type Error = Infallible;

    async fn process(
        &self,
        (program_url, input, requirements, offer, request_id): FinalizerInput,
    ) -> Result<Self::Output, Self::Error> {
        Ok(ProofRequest {
            requirements,
            id: request_id.into(),
            imageUrl: program_url.into(),
            input,
            offer,
        })
    }
}

#[allow(unused)]
type Example = (
    (
        (
            ((StorageLayer<BuiltinStorageProvider>, PreflightLayer), RequirementsLayer),
            RequestIdLayer<DynProvider>,
        ),
        OfferLayer<DynProvider>,
    ),
    Finalizer,
);

#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ExampleRequestParams {
    pub program: Cow<'static, [u8]>,
    pub env: GuestEnv,
    pub program_url: Option<Url>,
    pub input: Option<RequestInput>,
    pub cycles: Option<u64>,
    pub journal: Option<Journal>,
    pub request_id: Option<RequestId>,
    pub offer: Option<Offer>,
    pub requirements: Option<Requirements>,
}

impl ExampleRequestParams {
    pub fn new(program: impl Into<Cow<'static, [u8]>>, env: impl Into<GuestEnv>) -> Self {
        Self {
            program: program.into(),
            env: env.into(),
            program_url: None,
            input: None,
            cycles: None,
            journal: None,
            request_id: None,
            offer: None,
            requirements: None,
        }
    }

    pub fn with_program_url(self, value: impl Into<Url>) -> Self {
        Self {
            program_url: Some(value.into()),
            ..self
        }
    }
}

impl<Program, Env> From<(Program, Env)> for ExampleRequestParams
where
    Program: Into<Cow<'static, [u8]>>,
    Env: Into<GuestEnv>,
{
    fn from(value: (Program, Env)) -> Self {
        Self::new(value.0, value.1)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("field `{label}` is required but is uninitialized")]
struct MissingFieldError {
    pub label: Cow<'static, str>,
}

impl MissingFieldError {
    pub fn new(label: impl Into<Cow<'static, str>>) -> Self {
        Self { label: label.into() }
    }
}

impl<S: StorageProvider> Adapt<StorageLayer<S>> for ExampleRequestParams
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    type Output = Self;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &StorageLayer<S>) -> Result<Self::Output, Self::Error> {
        // If program_url and input fields are already set, do nothing.
        // DO NOT MERGE: What if only one is set?
        if self.program_url.is_some() && self.input.is_some() {
            return Ok(self);
        }

        let (program_url, input) = layer.process((&self.program, &self.env)).await?;
        Ok(Self { program_url: Some(program_url), input: Some(input), ..self })
    }
}

impl Adapt<PreflightLayer> for ExampleRequestParams {
    type Output = Self;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &PreflightLayer) -> Result<Self::Output, Self::Error> {
        // If cycles and journal are already set, do nothing.
        // DO NOT MERGE: What if only one is set?
        if self.cycles.is_some() && self.journal.is_some() {
            return Ok(self);
        }

        let program_url = self.program_url.as_ref().ok_or(MissingFieldError::new("program_url"))?;
        let input = self.input.as_ref().ok_or(MissingFieldError::new("input"))?;

        let session_info = layer.process((program_url, input)).await?;
        let cycles = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
        let journal = session_info.journal;
        Ok(Self { cycles: Some(cycles), journal: Some(journal), ..self })
    }
}

impl Adapt<RequirementsLayer> for ExampleRequestParams {
    type Output = Self;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &RequirementsLayer) -> Result<Self::Output, Self::Error> {
        // If the requirements field is already set, do nothing.
        if self.requirements.is_some() {
            return Ok(self);
        }

        let journal = self.journal.as_ref().ok_or(MissingFieldError::new("journal"))?;

        let requirements = layer.process((&self.program, journal)).await?;
        Ok(Self { requirements: Some(requirements), ..self })
    }
}

impl<P> Adapt<RequestIdLayer<P>> for ExampleRequestParams
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Self;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &RequestIdLayer<P>) -> Result<Self::Output, Self::Error> {
        // If the request_id field is already populated, do nothing.
        if self.request_id.is_some() {
            return Ok(self);
        }

        let request_id = layer.process(()).await?;
        Ok(Self { request_id: Some(request_id), ..self })
    }
}

impl<P> Adapt<OfferLayer<P>> for ExampleRequestParams
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Self;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &OfferLayer<P>) -> Result<Self::Output, Self::Error> {
        // If the offer field is already populated, do nothing.
        if self.offer.is_some() {
            return Ok(self);
        }

        let requirements =
            self.requirements.as_ref().ok_or(MissingFieldError::new("requirements"))?;
        let request_id = self.request_id.as_ref().ok_or(MissingFieldError::new("request_id"))?;
        let cycles = self.cycles.ok_or(MissingFieldError::new("cycles"))?;

        let offer = layer.process((requirements, request_id, cycles)).await?;
        Ok(Self { offer: Some(offer), ..self })
    }
}

impl Adapt<Finalizer> for ExampleRequestParams {
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &Finalizer) -> Result<Self::Output, Self::Error> {
        let program_url = self.program_url.ok_or(MissingFieldError::new("program_url"))?;
        let input = self.input.ok_or(MissingFieldError::new("input"))?;
        let requirements = self.requirements.ok_or(MissingFieldError::new("requirements"))?;
        let offer = self.offer.ok_or(MissingFieldError::new("offer"))?;
        let request_id = self.request_id.ok_or(MissingFieldError::new("request_id"))?;

        layer
            .process((program_url, input, requirements, offer, request_id))
            .await
            .map_err(Into::into)
    }
}

#[allow(dead_code)]
trait AssertLayer<Input, Output>: Layer<Input, Output = Output> {}

impl RequestBuilder for Example {
    type Params = ExampleRequestParams;
    type Error = anyhow::Error;

    async fn build(&self, params: impl Into<Self::Params>) -> Result<ProofRequest, Self::Error> {
        self.process(params.into()).await
    }
}

//impl AssertLayer<(&[u8], &GuestEnv), ProofRequest> for Example {}

#[allow(dead_code)]
async fn example(example: Example) -> anyhow::Result<()> {
    example.build((b"", vec![])).await?;
    Ok(())
}
