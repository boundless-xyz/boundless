#![allow(missing_docs)]

use derive_builder::Builder;
use anyhow::Context;
use risc0_zkvm::{compute_image_id, Journal};
use risc0_zkvm::sha::Digestible;
use crate::contracts::{Requirements, Predicate};
use super::{Layer, Adapt, RequestParams};

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

impl Adapt<RequirementsLayer> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &RequirementsLayer) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with RequirementsLayer");

        if self.requirements.is_some() {
            return Ok(self);
        }

        let program = self.require_program()?;
        let journal = self.require_journal()?;

        let requirements = layer.process((program, journal)).await?;
        Ok(self.with_requirements(requirements))
    }
}