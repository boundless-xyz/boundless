// Copyright 2026 Boundless Foundation, Inc.
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

use super::{Adapt, RequestParams};
use crate::contracts::{RequestInput, RequestInputType};
use crate::input::GuestEnv;
use crate::prover_utils::local_executor::LocalExecutor;
use crate::storage::fetch_url;
use anyhow::{bail, ensure, Context};

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
/// Uses a LocalExecutor for execution, which deduplicates executions by
/// content-addressing (same program + input = same result returned from cache).
#[non_exhaustive]
#[derive(Clone, Default)]
pub struct PreflightLayer {
    executor: LocalExecutor,
}

impl PreflightLayer {
    /// Create a new preflight layer with a shared executor.
    ///
    /// The executor can be shared with other components (like pricing checks)
    /// to avoid redundant executions.
    pub fn with_executor(executor: LocalExecutor) -> Self {
        Self { executor }
    }

    /// Get a clone of the executor used by this layer.
    ///
    /// This can be used to share the executor with other components.
    pub fn executor(&self) -> LocalExecutor {
        self.executor.clone()
    }

    async fn fetch_env(&self, input: &RequestInput) -> anyhow::Result<GuestEnv> {
        let env = match input.inputType {
            RequestInputType::Inline => GuestEnv::decode(&input.data)?,
            RequestInputType::Url => {
                let input_url =
                    std::str::from_utf8(&input.data).context("Input URL is not valid UTF-8")?;
                tracing::info!("Fetching input from {}", input_url);
                GuestEnv::decode(&fetch_url(input_url).await?)?
            }
            _ => bail!("Unsupported input type"),
        };
        Ok(env)
    }
}

impl Adapt<PreflightLayer> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &PreflightLayer) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with PreflightLayer");

        if self.cycles.is_some() && self.journal.is_some() {
            if let (Some(image_id), Some(request_input), Some(cycles), Some(journal)) =
                (self.image_id, self.request_input.as_ref(), self.cycles, self.journal.as_ref())
            {
                if let Ok(env) = layer.fetch_env(request_input).await {
                    // If we have all values to skip re-executing in future steps, fill the
                    // executor cache to avoid redundant executions.
                    tracing::debug!("Filling executor cache for {image_id} with {cycles} cycles");
                    layer
                        .executor
                        .insert_execution_data(
                            &image_id.to_string(),
                            &env.stdin,
                            cycles,
                            journal.bytes.clone(),
                        )
                        .await;
                }
            }

            return Ok(self);
        }

        let program_url = self.require_program_url().context("failed to preflight request")?;
        let request_input = self.require_request_input().context("failed to preflight request")?;

        // Fetch program and input
        let program = fetch_url(program_url).await?;
        let env = layer.fetch_env(request_input).await?;
        // Use env.stdin directly - this matches what the pricing logic uses for hashing
        let input_bytes = env.stdin;

        // Compute image_id from the program
        let image_id = risc0_zkvm::compute_image_id(&program)?;
        let image_id_str = image_id.to_string();

        // Execute using LocalExecutor (with deduplication)
        let (stats, journal) = layer
            .executor
            .execute_program(&image_id_str, &program, &input_bytes)
            .await
            .map_err(|e| anyhow::anyhow!("preflight execution failed: {}", e))?;

        let cycles = stats.total_cycles;
        let journal = risc0_zkvm::Journal::new(journal);

        // Verify image_id if one was provided
        if let Some(provided_image_id) = self.image_id {
            ensure!(
                provided_image_id == image_id,
                "provided image ID does not match computed value: {provided_image_id} != {image_id}"
            );
        }

        Ok(self.with_cycles(cycles).with_journal(journal).with_image_id(image_id))
    }
}
