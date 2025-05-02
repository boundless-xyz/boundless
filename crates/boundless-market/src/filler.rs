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

use std::{borrow::Cow, convert::Infallible, rc::Rc};

use alloy::{
    network::Ethereum,
    providers::{DynProvider, Provider},
    primitives::U256,
};
use anyhow::{bail, Context};
use risc0_zkvm::{compute_image_id, sha::Digestible, Executor, Journal, SessionInfo};
use url::Url;

use crate::{
    contracts::{
        boundless_market::BoundlessMarketService, Input as RequestInput, InputType, Offer,
        Predicate, ProofRequest, RequestId, Requirements,
    },
    input::GuestEnv,
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
pub struct StorageLayer<S: StorageProvider> {
    /// Maximum number of bytes to send as an inline input.
    ///
    /// Inputs larger than this size will be uploaded using the given storage provider. Set to none
    /// to indicate that inputs should always be sent inline.
    pub inline_input_max_bytes: Option<usize>,
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

// TODO: Add non-default ways to build a StorageLayer.
impl<S> Default for StorageLayer<S>
where
    S: StorageProvider + Default,
{
    fn default() -> Self {
        Self {
            // Default max inline input size is 2 kB.
            inline_input_max_bytes: Some(2 * 1024),
            storage_provider: S::default(),
        }
    }
}

#[non_exhaustive]
pub struct PreflightLayer {
    executor: Rc<dyn Executor>,
}

impl PreflightLayer {
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

pub type PreflightLayerOutput = SessionInfo;

impl Layer<(&Url, &RequestInput)> for PreflightLayer {
    type Output = PreflightLayerOutput;
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
pub struct RequirementsLayer {}

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
#[derive(Copy, Clone, Debug)]
pub enum RequestIdLayerMode {
    Rand,
    Nonce,
}

#[non_exhaustive]
pub struct RequestIdLayer<P> {
    pub boundless_market: BoundlessMarketService<P>,
    pub mode: RequestIdLayerMode,
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
pub struct OfferLayer<P> {
    pub provider: P,
    // default: 0 ETH
    pub min_price_per_mcycle: U256,
    // default: 0.0001 ETH
    pub max_price_per_mcycle: U256,
    // default: 15 seconds
    pub bidding_start_delay: u64,
    // default: 120 seconds
    pub ramp_up_period: u64,
    // default: 600 seconds
    pub lock_timeout: u64,
    // default: 200_000
    pub lock_gas_estimate: u64,
    // default: 750_000
    pub fulfill_gas_estimate: u64,
    // default: 250_000
    pub groth16_verify_gas_estimate: u64,
}

impl<P> Layer<(&Requirements, &RequestId, u64)> for OfferLayer<P>
where
    P: Provider<Ethereum> + 'static + Clone,
{
    type Output = Offer;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (_requirements, _request_id, cycle_count): (&Requirements, &RequestId, u64),
    ) -> anyhow::Result<Self::Output> {
        let _min_price = self.min_price_per_mcycle * U256::from(cycle_count);
        todo!()
    }
}

pub struct Finalizer {}

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

impl<Program: Into<Cow<'static, [u8]>>> From<(Program, Vec<u8>)> for ExampleRequestParams {
    fn from(value: (Program, Vec<u8>)) -> Self {
        Self {
            program: value.0.into(),
            env: GuestEnv { stdin: value.1 },
            program_url: None,
            input: None,
            cycles: None,
            journal: None,
            request_id: None,
            offer: None,
            requirements: None,
        }
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

        let requirements = self.requirements.as_ref().ok_or(MissingFieldError::new("requirements"))?;
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
