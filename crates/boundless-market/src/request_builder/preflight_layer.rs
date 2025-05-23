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
use crate::contracts::{RequestInput, RequestInputType};
use crate::input::GuestEnv;
use crate::storage::fetch_url;
use anyhow::{bail, ensure, Context};
use risc0_zkvm::{default_executor, sha::Digestible, SessionInfo};
use url::Url;

/// A layer that performs preflight execution of the guest program.
///
/// This layer runs the program with the provided input to compute:
/// - The journal output
/// - The cycle count
/// - The image ID
///
/// Running the program in advance allows for proper pricing estimation and
/// verification configuration based on actual execution results.
///
/// Each time this layer is invoked, it created a new [Executor][risc0_zkvm::Executor] with
/// [default_executor].
#[non_exhaustive]
#[derive(Clone, Default)]
pub struct PreflightLayer {}

impl PreflightLayer {
    async fn fetch_env(&self, input: &RequestInput) -> anyhow::Result<GuestEnv> {
        let env = match input.inputType {
            RequestInputType::Inline => GuestEnv::decode(&input.data)?,
            RequestInputType::Url => {
                let input_url =
                    std::str::from_utf8(&input.data).context("Input URL is not valid UTF-8")?;
                tracing::info!("Fetching input from {}", input_url);
                GuestEnv::decode(&fetch_url(&input_url).await?)?
            }
            _ => bail!("Unsupported input type"),
        };
        Ok(env)
    }
}

impl Layer<(&Url, &RequestInput)> for PreflightLayer {
    type Output = SessionInfo;
    type Error = anyhow::Error;

    async fn process(
        &self,
        (program_url, input): (&Url, &RequestInput),
    ) -> anyhow::Result<Self::Output> {
        let program = fetch_url(program_url).await?;
        let env = self.fetch_env(input).await?;
        let session_info = default_executor().execute(env.try_into()?, &program)?;
        Ok(session_info)
    }
}

impl Adapt<PreflightLayer> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &PreflightLayer) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with PreflightLayer");

        if self.cycles.is_some() && self.journal.is_some() {
            return Ok(self);
        }

        let program_url = self.require_program_url().context("failed to preflight request")?;
        let input = self.require_request_input().context("failed to preflight request")?;

        let session_info = layer.process((program_url, input)).await?;
        let cycles = session_info.segments.iter().map(|segment| 1 << segment.po2).sum::<u64>();
        let journal = session_info.journal;

        // NOTE: SessionInfo should have ReceiptClaim provided for recent versions of risc0_zkvm.
        let preflight_image_id = session_info
            .receipt_claim
            .context("preflight execution did not provide ReceiptClaim")?
            .pre
            .digest();
        if let Some(provided_image_id) = self.image_id {
            ensure!(provided_image_id == preflight_image_id, "provided image ID does not match the value calculated in preflight: {provided_image_id} != {preflight_image_id}");
        }

        Ok(self.with_cycles(cycles).with_journal(journal).with_image_id(preflight_image_id))
    }
}
