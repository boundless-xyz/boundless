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

use std::{borrow::Cow, rc::Rc};

use anyhow::{bail, Context};
use risc0_zkvm::{Executor, Journal, ReceiptClaim};
use url::Url;

use crate::{
    contracts::{Input, InputType, Offer, ProofRequest, RequestId},
    input::GuestEnv,
    storage::{BuiltinStorageProvider, StorageProvider, fetch_url},
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

pub trait RequestBuilder<Input> {
    /// Error type that may be returned by this filler.
    type Error;

    async fn build(&self, input: Input) -> Result<ProofRequest, Self::Error>;
}

impl<I, L> RequestBuilder<I> for L
where
    L: Layer<I, Output = ProofRequest>,
{
    type Error = L::Error;

    async fn build(&self, input: I) -> Result<ProofRequest, Self::Error> {
        self.process(input.into()).await
    }
}

pub trait Layer<Input> {
    type Output;
    /// Error type that may be returned by this layer.
    type Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error>;
}

/// Define a layer as a stack of two layers. Output of layer A is piped into layer B, with pre and
/// post-processing defined by the [Adapt] trait when required.
impl<A, B, Input> Layer<Input> for (A, B)
where
    A: Layer<Input>,
    B: Layer<A::Output>,
    A::Error: Into<B::Error>,
{
    type Output = B::Output;
    type Error = B::Error;

    async fn process(&self, input: Input) -> Result<Self::Output, Self::Error> {
        let output_a = self.0.process(input).await.map_err(|e| e.into())?;
        self.1.process(output_a).await
    }
}

// TODO: Either define this as an enum that can also take a URL that is pre-uploaded, or use the
// Adapt trait to accept URL inputs such that it bypasses the StorageLayer.
pub struct ProgramAndEnv {
    pub program: Cow<'static, [u8]>,
    pub env: GuestEnv,
}

impl<Program, Stdin> From<(Program, Stdin)> for ProgramAndEnv
where
    Program: Into<Cow<'static, [u8]>>,
    Stdin: Into<Vec<u8>>,
{
    fn from(value: (Program, Stdin)) -> Self {
        Self { program: value.0.into(), env: GuestEnv { stdin: value.1.into() } }
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

impl<In, S: StorageProvider> Layer<In> for StorageLayer<S>
where
    S::Error: std::error::Error + Send + Sync + 'static,
    In: Into<ProgramAndEnv>,
{
    type Error = anyhow::Error;
    type Output = (Url, Input);

    async fn process(&self, input: In) -> Result<Self::Output, Self::Error> {
        let ProgramAndEnv { program, env } = input.into();
        let program_url = self.storage_provider.upload_program(&program).await?;
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                Input::url(self.storage_provider.upload_input(&input_data).await?)
            }
            _ => Input::inline(input_data),
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

#[non_exhaustive]
pub struct PreflightInfo {
    pub cycles: u64,
    pub journal: Journal,
    pub receipt_claim: ReceiptClaim,
}

impl PreflightLayer {
    async fn fetch_env(&self, input: Input) -> anyhow::Result<GuestEnv> {
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

    async fn execute(&self, program: &[u8], env: GuestEnv) -> anyhow::Result<PreflightInfo> {
        let _session_info = self.executor.execute(env.try_into()?, program)?;
        todo!("create a PreflightInfo from session_info")
    }
}

pub type PreflightLayerOutput = (Url, Input, PreflightInfo);

impl Layer<(Url, Input)> for PreflightLayer {
    type Output = PreflightLayerOutput;
    type Error = anyhow::Error;

    async fn process(&self, (program_url, input): (Url, Input)) -> anyhow::Result<Self::Output> {
        let program = fetch_url(program_url).await?;
        let env = self.fetch_env(input).await?;
        let _info = self.execute(&program, env);
        todo!()
    }
}

pub struct RequestIdLayer {}

impl Layer<()> for RequestIdLayer {
    type Output = RequestId;
    type Error = anyhow::Error;

    async fn process(&self, _input: ()) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

pub struct OfferLayer {}

pub type OfferLayerInput = (Url, Input, PreflightInfo, RequestId);

impl Layer<OfferLayerInput> for OfferLayer {
    type Output = (Url, Input, PreflightInfo, Offer, RequestId);
    type Error = anyhow::Error;

    async fn process(&self, _input: OfferLayerInput) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

pub struct Finalizer {}

pub type FinalizerInput = (Url, Input, PreflightInfo, Offer, RequestId);

impl Layer<FinalizerInput> for Finalizer {
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(&self, _input: FinalizerInput) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

impl Layer<PreflightLayerOutput> for RequestIdLayer {
    type Output = OfferLayerInput;
    type Error = anyhow::Error;

    async fn process(&self, input: PreflightLayerOutput) -> Result<Self::Output, Self::Error> {
        let request_id = self.process(()).await?;
        let (url, input, preflight_info) = input;
        Ok((url, input, preflight_info, request_id))
    }
}

#[allow(unused)]
type Example = (
    (((StorageLayer<BuiltinStorageProvider>, PreflightLayer), RequestIdLayer), OfferLayer),
    Finalizer,
);

#[allow(dead_code)]
trait AssertLayer<Input, Output>: Layer<Input, Output = Output> {}
#[allow(dead_code)]
trait AssertRequestBuilder<Input>: RequestBuilder<Input> {}

impl AssertLayer<ProgramAndEnv, ProofRequest> for Example {}
impl AssertRequestBuilder<ProgramAndEnv> for Example {}
impl AssertRequestBuilder<(&'static [u8], Vec<u8>)> for Example {}
