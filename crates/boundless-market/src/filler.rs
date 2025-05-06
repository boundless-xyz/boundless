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

impl<S: StorageProvider> StorageLayer<S>
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn process_program(&self, program: &[u8]) -> anyhow::Result<Url> {
        let program_url = self.storage_provider.upload_program(program).await?;
        Ok(program_url)
    }

    pub async fn process_env(&self, env: &GuestEnv) -> anyhow::Result<RequestInput> {
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                RequestInput::url(self.storage_provider.upload_input(&input_data).await?)
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
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
        let program_url = self.process_program(program).await?;
        let request_input = self.process_env(env).await?;
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

// TODO: If using the preflight layer, how to avoid a second preflight on submit request?
// TODO: Provide a layer impl that works without downloading the program and input.
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

// NOTE: We don't use derive_builder here because we need to be able to access the values on the
// incrementally built parameters.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ExampleRequestParams {
    pub program: Option<Cow<'static, [u8]>>,
    pub env: Option<GuestEnv>,
    pub program_url: Option<Url>,
    pub input: Option<RequestInput>,
    pub cycles: Option<u64>,
    pub journal: Option<Journal>,
    pub request_id: Option<RequestId>,
    pub offer: Option<Offer>,
    pub requirements: Option<Requirements>,
}

impl ExampleRequestParams {
    pub fn require_program(&self) -> Result<&[u8], MissingFieldError> {
        self.program.as_deref().ok_or(MissingFieldError::new("program"))
    }

    pub fn with_program(self, value: impl Into<Cow<'static, [u8]>>) -> Self {
        Self { program: Some(value.into()), ..self }
    }

    pub fn require_env(&self) -> Result<&GuestEnv, MissingFieldError> {
        self.env.as_ref().ok_or(MissingFieldError::new("env"))
    }

    pub fn with_env(self, value: impl Into<GuestEnv>) -> Self {
        Self { env: Some(value.into()), ..self }
    }

    pub fn require_program_url(&self) -> Result<&Url, MissingFieldError> {
        self.program_url.as_ref().ok_or(MissingFieldError::new("program_url"))
    }

    pub fn with_program_url(self, value: impl Into<Url>) -> Self {
        Self { program_url: Some(value.into()), ..self }
    }

    pub fn require_input(&self) -> Result<&RequestInput, MissingFieldError> {
        self.input.as_ref().ok_or(MissingFieldError::new("input"))
    }

    pub fn with_input(self, value: impl Into<RequestInput>) -> Self {
        Self { input: Some(value.into()), ..self }
    }

    pub fn require_cycles(&self) -> Result<u64, MissingFieldError> {
        self.cycles.ok_or(MissingFieldError::new("cycles"))
    }

    pub fn with_cycles(self, value: u64) -> Self {
        Self { cycles: Some(value), ..self }
    }

    pub fn require_journal(&self) -> Result<&Journal, MissingFieldError> {
        self.journal.as_ref().ok_or(MissingFieldError::new("journal"))
    }

    pub fn with_journal(self, value: impl Into<Journal>) -> Self {
        Self { journal: Some(value.into()), ..self }
    }

    pub fn require_request_id(&self) -> Result<&RequestId, MissingFieldError> {
        self.request_id.as_ref().ok_or(MissingFieldError::new("request_id"))
    }

    pub fn with_request_id(self, value: impl Into<RequestId>) -> Self {
        Self { request_id: Some(value.into()), ..self }
    }

    pub fn require_offer(&self) -> Result<&Offer, MissingFieldError> {
        self.offer.as_ref().ok_or(MissingFieldError::new("offer"))
    }

    pub fn with_offer(self, value: impl Into<Offer>) -> Self {
        Self { offer: Some(value.into()), ..self }
    }

    pub fn require_requirements(&self) -> Result<&Requirements, MissingFieldError> {
        self.requirements.as_ref().ok_or(MissingFieldError::new("requirements"))
    }

    pub fn with_requirements(self, value: impl Into<Requirements>) -> Self {
        Self { requirements: Some(value.into()), ..self }
    }
}

impl<Program, Env> From<(Program, Env)> for ExampleRequestParams
where
    Program: Into<Cow<'static, [u8]>>,
    Env: Into<GuestEnv>,
{
    fn from(value: (Program, Env)) -> Self {
        Self::default().with_program(value.0).with_env(value.1)
    }
}

#[derive(thiserror::Error, Debug)]
#[error("field `{label}` is required but is uninitialized")]
pub struct MissingFieldError {
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
        let mut params = self;
        if params.program_url.is_none() {
            let program_url = layer.process_program(params.require_program()?).await?;
            params = params.with_program_url(program_url);
        }
        if params.input.is_none() {
            let input = layer.process_env(params.require_env()?).await?;
            params = params.with_input(input);
        }
        Ok(params)
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

        let program_url = self.require_program_url()?;
        let input = self.require_input()?;

        let session_info = layer.process((program_url, input)).await?;
        let cycles = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
        let journal = session_info.journal;
        Ok(self.with_cycles(cycles).with_journal(journal))
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

        let program = self.require_program()?;
        let journal = self.require_journal()?;

        let requirements = layer.process((program, journal)).await?;
        Ok(self.with_requirements(requirements))
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
        Ok(self.with_request_id(request_id))
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

        let requirements = self.require_requirements()?;
        let request_id = self.require_request_id()?;
        let cycles = self.require_cycles()?;

        let offer = layer.process((requirements, request_id, cycles)).await?;
        Ok(self.with_offer(offer))
    }
}

impl Adapt<Finalizer> for ExampleRequestParams {
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &Finalizer) -> Result<Self::Output, Self::Error> {
        // We create local variables to hold owned values
        let program_url = self.require_program_url()?.clone();
        let input = self.require_input()?.clone();
        let requirements = self.require_requirements()?.clone();
        let offer = self.require_offer()?.clone();
        let request_id = self.require_request_id()?.clone();

        layer
            .process((program_url, input, requirements, offer, request_id))
            .await
            .map_err(Into::into)
    }
}

impl RequestBuilder for Example {
    type Params = ExampleRequestParams;
    type Error = anyhow::Error;

    async fn build(&self, params: impl Into<Self::Params>) -> Result<ProofRequest, Self::Error> {
        self.process(params.into()).await
    }
}

#[allow(dead_code)]
async fn example(example: Example) -> anyhow::Result<()> {
    example.build((b"", vec![])).await?;
    Ok(())
}

/*
TODO: Write a test using the code from above!
#[cfg(test)]
mod tests {
    use boundless_market_test_utils::create_test_ctx;
    use alloy::node_bindings::Anvil;

    #[tokio::test]
    async fn basic() {
        let anvil = Anvil::new().spawn();
        let test_ctx = create_test_ctx(&anvil).await.unwrap();
        let storage = MockStorageProvider::start();

        //let offer_layer = OfferLayer::builder().provider(
        test_
    }
}
*/
