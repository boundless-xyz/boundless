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

use super::{Adapt, Layer, RequestParams};
use crate::{
    contracts::Input as RequestInput,
    contracts::{Offer, ProofRequest, RequestId, Requirements},
    util::now_timestamp,
};
use anyhow::{bail, Context};
use derive_builder::Builder;
use url::Url;

#[non_exhaustive]
#[derive(Debug, Clone, Builder)]
pub struct Finalizer {
    /// If true, the request's expiration time will be checked against the system clock.
    #[builder(default = true)]
    pub check_expiration: bool,
}

impl Default for Finalizer {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for Finalizer")
    }
}

impl Finalizer {
    pub fn builder() -> FinalizerBuilder {
        Default::default()
    }
}

impl Layer<(Url, RequestInput, Requirements, Offer, RequestId)> for Finalizer {
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (program_url, input, requirements, offer, request_id): (
            Url,
            RequestInput,
            Requirements,
            Offer,
            RequestId,
        ),
    ) -> Result<Self::Output, Self::Error> {
        let request = ProofRequest {
            requirements,
            id: request_id.into(),
            imageUrl: program_url.into(),
            input,
            offer,
        };

        request.validate().context("built request is invalid; check request parameters")?;
        if self.check_expiration && request.is_expired() {
            bail!(
                "request expired at {}; current time is {}",
                request.expires_at(),
                now_timestamp()
            );
        }
        if self.check_expiration && request.is_lock_expired() {
            bail!(
                "request lock expired at {}; current time is {}",
                request.lock_expires_at(),
                now_timestamp()
            );
        }
        Ok(request)
    }
}

impl Adapt<Finalizer> for RequestParams {
    type Output = ProofRequest;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &Finalizer) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with Finalizer");

        // We create local variables to hold owned values
        let program_url = self.require_program_url()?.clone();
        let input = self.require_request_input()?.clone();
        let requirements = self.require_requirements()?.clone();
        let offer = self.require_offer()?.clone();
        let request_id = self.require_request_id()?.clone();

        // As an extra consistency check. verify the journal satisfies the required predicate.
        if let Some(ref journal) = self.journal {
            if !requirements.predicate.eval(journal) {
                bail!("journal in request builder does not match requirements predicate; check request parameters");
            }
        }

        layer.process((program_url, input, requirements, offer, request_id)).await
    }
}
