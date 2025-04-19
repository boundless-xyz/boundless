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

#![allow(missing_docs)]
#![allow(async_fn_in_trait)] // DO NOT MERGE: Consider alternatives.

use std::borrow::Cow;

use risc0_zkvm::{Journal, ReceiptClaim};
use url::Url;

use crate::contracts::{Input, Offer, ProofRequest, RequestId};

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
    type Error: core::error::Error;

    async fn build(&self, input: Input) -> Result<ProofRequest, Self::Error>;
}

pub trait Layer {
    type Input;
    type Output;
    /// Error type that may be returned by this filler.
    type Error;

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;
}

pub trait Adapt<L: Layer + ?Sized> {
    type Output;
    type Passthrough;

    fn preprocess(self) -> (Self::Passthrough, L::Input);

    fn postprocess(buck: Self::Passthrough, data: L::Output) -> Self::Output;
}


impl<L> Adapt<L> for L::Input
where
    L: Layer + ?Sized,
{
    type Passthrough = ();
    type Output = L::Output;

    fn preprocess(self) -> (Self::Passthrough, <L as Layer>::Input) {
        ((), self) 
    }

    fn postprocess((): Self::Passthrough, data: <L as Layer>::Output) -> Self::Output {
        data
    }
}

impl<A, B> Layer for (A, B)
where
    A: Layer,
    B: Layer,
    A::Output: Adapt<B>,
    A::Error: Into<B::Error>,
{
    type Input = A::Input;
    type Output = <A::Output as Adapt<B>>::Output;
    type Error = B::Error;

    async fn process(&self, input: Self::Input) -> Result<Self::Output, Self::Error> {
        let output_a = self.0.process(input).await.map_err(|e| e.into())?;
        let (passthrough, input_b) = output_a.preprocess();
        let output_b = self.1.process(input_b).await?;
        Ok(A::Output::postprocess(passthrough, output_b))
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

pub struct StorageLayer {}

impl Layer for StorageLayer {
    type Input = ProgramAndInput;
    type Error = anyhow::Error;
    type Output = (Url, Input);

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
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

impl Adapt<RequestIdLayer> for <PreflightLayer as Layer>::Output {
    type Output = <OfferLayer as Layer>::Input;
    type Passthrough = Self;

    fn preprocess(self) -> (Self::Passthrough, <RequestIdLayer as Layer>::Input) {
        (self, ())
    }

    fn postprocess(buck: Self::Passthrough, id: RequestId) -> Self::Output {
        let (url, input, preflight_info) = buck;
        (url, input, preflight_info, id)
    }
}

#[allow(unused)]
type Example = ((((StorageLayer, PreflightLayer), RequestIdLayer), OfferLayer), Finalizer);

#[allow(dead_code)]
trait AssertLayer<Input, Output>: Layer<Input = Input, Output = Output> {}

impl AssertLayer<ProgramAndInput, ProofRequest> for Example {}
