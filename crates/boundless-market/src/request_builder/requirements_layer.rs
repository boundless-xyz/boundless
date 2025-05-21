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

use super::{Adapt, Layer, MissingFieldError, RequestParams};
use crate::contracts::{Callback, Predicate, Requirements};
use alloy::primitives::{aliases::U96, Address, FixedBytes, B256};
use anyhow::{ensure, Context};
use clap::Args;
use derive_builder::Builder;
use risc0_zkvm::{compute_image_id, Journal};
use risc0_zkvm::{sha::Digestible, Digest};

const DEFAULT_CALLBACK_GAS_LIMT: u64 = 100000u64;

#[non_exhaustive]
#[derive(Clone, Builder, Default)]
pub struct RequirementsLayer {}

#[non_exhaustive]
#[derive(Clone, Debug, Default, Builder, Args)]
/// A partial [Requirements], with all the fields as optional. Used in the [RequirementsLayer] to
/// provide explicit settings.
///
/// Does not include the predicate, which is created by [RequirementsLayer].
pub struct RequirementParams {
    #[clap(skip)]
    #[builder(setter(strip_option, into), default)]
    pub predicate: Option<Predicate>,
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub image_id: Option<B256>,
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub callback_address: Option<Address>,
    #[clap(long)]
    #[builder(setter(strip_option), default)]
    pub callback_gas_limit: Option<u64>,
    #[clap(long)]
    #[builder(setter(strip_option, into), default)]
    pub selector: Option<FixedBytes<4>>,
}

impl From<Requirements> for RequirementParams {
    fn from(value: Requirements) -> Self {
        Self {
            predicate: Some(value.predicate),
            image_id: Some(value.imageId),
            selector: Some(value.selector),
            callback_address: Some(value.callback.addr),
            callback_gas_limit: Some(value.callback.gasLimit.to()),
        }
    }
}

impl TryFrom<RequirementParams> for Requirements {
    type Error = MissingFieldError;

    fn try_from(value: RequirementParams) -> Result<Self, Self::Error> {
        Ok(Self {
            predicate: value.predicate.ok_or(MissingFieldError::new("predicate"))?,
            imageId: value.image_id.ok_or(MissingFieldError::new("image_id"))?,
            selector: value.selector.unwrap_or_default(),
            callback: Callback {
                addr: value.callback_address.unwrap_or_default(),
                gasLimit: U96::from(value.callback_gas_limit.unwrap_or_default()),
            },
        })
    }
}

impl From<RequirementParamsBuilder> for RequirementParams {
    fn from(value: RequirementParamsBuilder) -> Self {
        // Builder should be infallible.
        value.build().expect("implementation error in RequirementParams")
    }
}

// Allows for a nicer builder pattern in RequestParams.
impl From<&mut RequirementParamsBuilder> for RequirementParams {
    fn from(value: &mut RequirementParamsBuilder) -> Self {
        value.clone().into()
    }
}

impl RequirementParams {
    pub fn builder() -> RequirementParamsBuilder {
        Default::default()
    }
}

impl RequirementsLayer {
    pub fn builder() -> RequirementsLayerBuilder {
        Default::default()
    }
}

impl Layer<(&[u8], &Journal, &RequirementParams)> for RequirementsLayer {
    type Output = Requirements;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (program, journal, params): (&[u8], &Journal, &RequirementParams),
    ) -> Result<Self::Output, Self::Error> {
        let image_id =
            compute_image_id(program).context("failed to compute image ID for program")?;
        self.process((image_id, journal, params)).await
    }
}

impl Layer<(Digest, &Journal, &RequirementParams)> for RequirementsLayer {
    type Output = Requirements;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (image_id, journal, params): (Digest, &Journal, &RequirementParams),
    ) -> Result<Self::Output, Self::Error> {
        let predicate =
            params.predicate.clone().unwrap_or_else(|| Predicate::digest_match(journal.digest()));
        if let Some(params_image_id) = params.image_id {
            ensure!(
                image_id == Digest::from(<[u8; 32]>::from(params_image_id)),
                "mismatch between specified and computed image ID"
            )
        }
        let callback = params
            .callback_address
            .map(|addr| Callback {
                addr,
                gasLimit: U96::from(params.callback_gas_limit.unwrap_or(DEFAULT_CALLBACK_GAS_LIMT)),
            })
            .unwrap_or_default();
        let selector = params.selector.unwrap_or_default();

        Ok(Requirements {
            imageId: <[u8; 32]>::from(image_id).into(),
            predicate,
            callback,
            selector,
        })
    }
}

impl Adapt<RequirementsLayer> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &RequirementsLayer) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with RequirementsLayer");

        // If the two required paramters of image ID and predicate are already set, skip this
        // layer.
        if self.requirements.predicate.is_some() && self.requirements.image_id.is_some() {
            return Ok(self);
        }

        let journal = self.require_journal()?;
        let requirements = if let Some(image_id) = self.image_id {
            layer.process((image_id, journal, &self.requirements)).await?
        } else {
            let program = self.require_program()?;
            layer.process((program, journal, &self.requirements)).await?
        };

        Ok(self.with_requirements(requirements))
    }
}
