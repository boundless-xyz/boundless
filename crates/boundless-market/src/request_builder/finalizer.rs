#![allow(missing_docs)]

use derive_builder::Builder;
use anyhow::{Context, bail};
use url::Url;
use crate::contracts::{ProofRequest, Offer, RequestId, Requirements};
use crate::contracts::Input as RequestInput;
use super::{Layer, Adapt, RequestParams};

#[non_exhaustive]
#[derive(Debug, Clone, Builder, Default)]
pub struct Finalizer {}

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
        (program_url, input, requirements, offer, request_id): (Url, RequestInput, Requirements, Offer, RequestId),
    ) -> Result<Self::Output, Self::Error> {
        let request = ProofRequest {
            requirements,
            id: request_id.into(),
            imageUrl: program_url.into(),
            input,
            offer,
        };

        request.validate().context("built request is invalid; check request parameters")?;
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

        layer
            .process((program_url, input, requirements, offer, request_id))
            .await
            .map_err(Into::into)
    }
}