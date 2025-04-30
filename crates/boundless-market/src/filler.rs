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

use anyhow::Context;
use risc0_zkvm::{Executor, Journal, ReceiptClaim};
use url::Url;

use crate::{
    contracts::{Input, Offer, ProofRequest, RequestId},
    input::GuestEnv,
    storage::{BuiltinStorageProvider, StorageProvider},
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
    L: Layer<Output = ProofRequest>,
    I: Into<L::Input>,
{
    type Error = L::Error;

    async fn build(&self, input: I) -> Result<ProofRequest, Self::Error> {
        self.process(input.into()).await
    }
}

pub trait Layer {
    type Input;
    type Output;
    /// Error type that may be returned by this filler.
    type Error;

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

// DO NOT MERGE: Consider the alternative factoring of this that works by removing the passthrough
// struct and instead giving a reference to preprocess that it uses to prepare the input, then
// passing the input again to postprocess.
pub trait Adapt<Input>: Layer {
    type Postprocessed;
    type Passthrough;

    fn preprocess(&self, input: Input) -> (Self::Passthrough, Self::Input);

    fn postprocess(&self, pass: Self::Passthrough, out: Self::Output) -> Self::Postprocessed;
}

impl<L> Adapt<L::Input> for L
where
    L: Layer + ?Sized,
{
    type Passthrough = ();
    type Postprocessed = L::Output;

    fn preprocess(&self, input: Self::Input) -> (Self::Passthrough, Self::Input) {
        ((), input)
    }

    fn postprocess(&self, (): Self::Passthrough, out: Self::Output) -> Self::Postprocessed {
        out
    }
}

/// Define a layer as a stack of two layers. Output of layer A is piped into layer B, with pre and
/// post-processing defined by the [Adapt] trait when required.
impl<A, B> Layer for (A, B)
where
    A: Layer,
    B: Layer,
    B: Adapt<A::Output>,
    A::Error: Into<B::Error>,
{
    type Input = A::Input;
    type Output = <B as Adapt<A::Output>>::Postprocessed;
    type Error = B::Error;

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let output_a = self.0.process(input).await.map_err(|e| e.into())?;
        let (passthrough, input_b) = self.1.preprocess(output_a);
        let output_b = self.1.process(input_b).await?;
        Ok(self.1.postprocess(passthrough, output_b))
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

impl<S: StorageProvider> Layer for StorageLayer<S>
where S::Error: std::error::Error + Send + Sync + 'static,
{
    type Input = ProgramAndEnv;
    type Error = anyhow::Error;
    type Output = (Url, Input);

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let program_url = self.storage_provider.upload_program(&input.program).await?;
        let input_data = input.env.encode().context("failed to encode guest environment")?;
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

impl Layer for PreflightLayer {
    // TODO: Defining the Layer::Input as a Url for the program codifies that fetching the program
    // and input from the Internet is part of the preflight process. This is recommended because it
    // ensure the data is actually available, but there are certainly cases where this is not
    // desirable. In these cases, the caller could not use PreflightLayer as written. Should this
    // be rewritten to take the program and environment and use an Adapt impl or simmilar to handle
    // the fetch?
    type Input = (Url, Input);
    type Output = (Url, Input, PreflightInfo);
    type Error = anyhow::Error;

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

pub struct RequestIdLayer {}

impl Layer for RequestIdLayer {
    type Input = ();
    type Output = RequestId;
    type Error = anyhow::Error;

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

pub struct OfferLayer {}

impl Layer for OfferLayer {
    type Input = (Url, Input, PreflightInfo, RequestId);
    type Output = (Url, Input, PreflightInfo, Offer, RequestId);
    type Error = anyhow::Error;

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

pub struct Finalizer {}

impl Layer for Finalizer {
    type Input = (Url, Input, PreflightInfo, Offer, RequestId);
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
    }
}

impl Adapt<<PreflightLayer as Layer>::Output> for RequestIdLayer {
    type Postprocessed = <OfferLayer as Layer>::Input;
    type Passthrough = <PreflightLayer as Layer>::Output;

    fn preprocess(&self, input: <PreflightLayer as Layer>::Output) -> (Self::Passthrough, ()) {
        (input, ())
    }

    fn postprocess(&self, pass: Self::Passthrough, id: RequestId) -> Self::Postprocessed {
        let (url, input, preflight_info) = pass;
        (url, input, preflight_info, id)
    }
}

#[allow(unused)]
type Example = (
    (((StorageLayer<BuiltinStorageProvider>, PreflightLayer), RequestIdLayer), OfferLayer),
    Finalizer,
);

#[allow(dead_code)]
trait AssertLayer<Input, Output>: Layer<Input = Input, Output = Output> {}
#[allow(dead_code)]
trait AssertRequestBuilder<Input>: RequestBuilder<Input> {}

impl AssertLayer<ProgramAndEnv, ProofRequest> for Example {}
impl AssertRequestBuilder<ProgramAndEnv> for Example {}
impl AssertRequestBuilder<(&'static [u8], Vec<u8>)> for Example {}
