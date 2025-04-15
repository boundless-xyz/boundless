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

use crate::contracts::ProofRequestBuilder;

/// When building a [ProofRequest], a filler provides values for unpopulated fields.
///
/// TODO: Link to examples.
#[allow(async_fn_in_trait)]
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
        Self::Error: Into<F::Error>,
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
    L::Error: Into<R::Error>,
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
