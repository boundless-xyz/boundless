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

use std::{borrow::Cow, fmt::Debug, marker::PhantomData};

use url::Url;
use risc0_zkvm::{Journal, ReceiptClaim};

use crate::contracts::{ProofRequestBuilder, ProofRequest, Input, Offer, RequestId};

/// When building a [ProofRequest], a filler provides values for unpopulated fields.
///
/// TODO: Link to examples.
pub trait ProofRequestFiller {
    /// Error type that may be returned by this filler.
    type Error: core::error::Error;

    /// Accept a partially filled [ProofRequest] as a [ProofRequestBuilder] and populate the fields
    /// supported by this filler.
    async fn fill(&self, req: ProofRequestBuilder) -> Result<ProofRequestBuilder, Self::Error>;

    /// Creating a joined filler that will first fill the fields using `self`, then pipe the result
    /// to the next filler.
    fn join_with<F>(self, other: F) -> Join<Self, F>
    where
        F: ProofRequestFiller,
        Self: Sized,
        F::Error: Into<Self::Error>,
    {
        Join::new(self, other)
    }
}

/// A joined filler that sequentially combines a left and a right filler.
#[derive(Clone, Copy, Debug, Default)]
pub struct Join<L, R> {
    left: L,
    right: R,
}

impl<L, R> Join<L, R>
where
    L: ProofRequestFiller,
    R: ProofRequestFiller,
    R::Error: Into<L::Error>,
{
    /// Create a joined filler that sequentially combines a left and a right filler.
    pub const fn new(left: L, right: R) -> Self {
        Self { left, right }
    }
}

impl<L, R> ProofRequestFiller for Join<L, R>
where
    L: ProofRequestFiller,
    R: ProofRequestFiller,
    L::Error: Into<R::Error>,
{
    type Error = R::Error;

    async fn fill(&self, req: ProofRequestBuilder) -> Result<ProofRequestBuilder, Self::Error> {
        let req = self.left.fill(req).await.map_err(|e| e.into())?;
        self.right.fill(req).await
    }
}

/// A filler that can populate the [Requirements][crate::contracts::Requirements] of a request
/// given then image URL and [Input][crate::contracts::Input].
pub struct PreflightFiller {}

/// A filler that can populate the [Offer][crate::contracts::Offer] on a request given the image
/// URL, [Input][crate::contracts::Input], and [Requirements][crate::contracts::Requirements].
pub struct OfferFiller {}

/// TODO
pub struct RequestIdFiller {}

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

pub struct ProgramAndInput<'a, 'b> {
    pub program: Cow<'a, [u8]>,
    pub input: Cow<'b, [u8]>,
}

impl<'a, 'b> Into<ProgramAndInput<'a, 'b>> for (&'a [u8], &'b [u8]) {
    fn into(self) -> ProgramAndInput<'a, 'b> {
        ProgramAndInput {
            program: Cow::Borrowed(self.0),
            input: Cow::Borrowed(self.1),
        }
    }
}

pub struct StorageLayer {}

impl<'a, 'b> Layer for StorageLayer
{
    type Input = ProgramAndInput<'a, 'b>;
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
    type Output = (Url, Input, PreflightInfo, RequestId);
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

pub trait Merge<T> {
    type Output;

    fn merge(self, data: T) -> Self::Output;
}

pub struct Pipe<Input, A, B> {
    pub a: A,
    pub b: B,
    phantom_input: PhantomData<Input>,
}

impl<A, B> Layer for Pipe<Input, A, B>
where A: Layer,
      B: Layer,
      Input: Clone,
      <Input as Merge<A::Output>>: Into<B::Input>,
{
    type Input = Input;
    type Output = <<Input as Merge<A::Output>> as Merge<B::Output>>::Output;

    async fn process(&self, _input: Self::Input) -> anyhow::Result<Self::Output> {
        todo!()
    }
}
