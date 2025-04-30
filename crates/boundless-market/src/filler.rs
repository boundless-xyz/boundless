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

use std::borrow::Cow;

use risc0_zkvm::{Journal, ReceiptClaim};
use url::Url;

use crate::{
    contracts::{Input, Offer, ProofRequest, RequestId},
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

pub struct ProgramAndInput {
    pub program: Cow<'static, [u8]>,
    pub input: Cow<'static, [u8]>,
}

impl<Program, Input> Into<ProgramAndInput> for (Program, Input)
where
    Program: Into<Cow<'static, [u8]>>,
    Input: Into<Cow<'static, [u8]>>,
{
    fn into(self) -> ProgramAndInput {
        ProgramAndInput { program: self.0.into(), input: self.1.into() }
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

impl<S: StorageProvider> Layer for StorageLayer<S> {
    type Input = ProgramAndInput;
    type Error = S::Error;
    type Output = (Url, Input);

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let program_url = self.storage_provider.upload_program(&input.program).await?;
        let request_input = match self.inline_input_max_bytes {
            Some(limit) if input.input.len() > limit => {
                Input::url(self.storage_provider.upload_input(&input.input).await?)
            }
            _ => Input::inline(input.input.to_vec()),
        };
        Ok((program_url, request_input))
    }
}

pub struct PreflightLayer {}

pub struct PreflightInfo {
    pub cycles: u64,
    pub journal: Journal,
    pub receipt_claim: ReceiptClaim,
}

impl Layer for PreflightLayer {
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

impl AssertLayer<ProgramAndInput, ProofRequest> for Example {}
impl AssertRequestBuilder<ProgramAndInput> for Example {}
impl AssertRequestBuilder<(&'static [u8], Vec<u8>)> for Example {}
