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
use crate::{
    contracts::{RequestInput, RequestInputType},
    input::GuestEnv,
    prover_utils::local_executor::LocalExecutor,
    storage::StorageDownloader,
    NotProvided,
};
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
#[derive(Clone)]
pub struct PreflightLayer<D> {
    executor: LocalExecutor,
    /// The downloader used to fetch programs and inputs from URLs.
    downloader: Option<D>,
}

impl<D> Default for PreflightLayer<D> {
    fn default() -> Self {
        Self { executor: LocalExecutor::default(), downloader: None }
    }
}

impl<D> PreflightLayer<D>
where
    D: StorageDownloader,
{
    /// Creates a new [PreflightLayer] with the given executor and downloader.
    pub fn new(executor: LocalExecutor, downloader: Option<D>) -> Self {
        Self { executor, downloader }
    }

    /// Get a clone of the executor used by this layer.
    pub fn executor_cloned(&self) -> LocalExecutor {
        self.executor.clone()
    }

    async fn fetch_env(&self, input: &RequestInput) -> anyhow::Result<GuestEnv> {
        let env = match input.inputType {
            RequestInputType::Inline => GuestEnv::decode(&input.data)?,
            RequestInputType::Url => {
                let downloader = self
                    .downloader
                    .as_ref()
                    .context("cannot preflight URL input without downloader")?;
                let input_url =
                    std::str::from_utf8(&input.data).context("Input URL is not valid UTF-8")?;
                tracing::info!("Fetching input from {}", input_url);
                GuestEnv::decode(&downloader.download(input_url).await?)?
            }
            _ => bail!("Unsupported input type"),
        };
        Ok(env)
    }

    /// Ensures image_id is set, computing from program (inline or fetched) if needed.
    async fn ensure_image_id(&self, params: RequestParams) -> anyhow::Result<RequestParams> {
        if params.image_id.is_some() {
            return Ok(params);
        }
        let program = match params.require_program() {
            Ok(bytes) => bytes.to_vec(),
            Err(_) => {
                let url = params.require_program_url()?;
                let downloader = self
                    .downloader
                    .as_ref()
                    .context("cannot fetch program URL without downloader")?;
                downloader.download(url.as_str()).await?
            }
        };
        let image_id = risc0_zkvm::compute_image_id(&program)?;
        Ok(params.with_image_id(image_id))
    }

    /// Best-effort: fills executor cache when we have all precomputed data.
    async fn fill_executor_cache_if_ready(&self, params: &RequestParams) {
        let (Some(image_id), Some(request_input), Some(cycles), Some(journal)) = (
            params.image_id,
            params.request_input.as_ref(),
            params.cycles,
            params.journal.as_ref(),
        ) else {
            return;
        };
        let Ok(env) = self.fetch_env(request_input).await else {
            return;
        };
        tracing::debug!("Filling executor cache for {image_id} with {cycles} cycles");
        self.executor
            .insert_execution_data(&image_id.to_string(), &env.stdin, cycles, journal.bytes.clone())
            .await;
    }
}

impl<D> Adapt<PreflightLayer<D>> for RequestParams
where
    D: StorageDownloader,
{
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(
        mut self,
        layer: &PreflightLayer<D>,
    ) -> Result<Self::Output, Self::Error> {
        if self.cycles.is_some() && self.journal.is_some() {
            self = layer.ensure_image_id(self).await?;
            layer.fill_executor_cache_if_ready(&self).await;
            return Ok(self);
        }

        tracing::trace!("Processing {self:?} with PreflightLayer");

        let program_url = self.require_program_url().context("failed to preflight request")?;
        let request_input = self.require_request_input().context("failed to preflight request")?;

        // Fetch program and input
        let downloader =
            layer.downloader.as_ref().context("cannot preflight URL request without downloader")?;
        let program = downloader.download(program_url.as_str()).await?;
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

impl Adapt<PreflightLayer<NotProvided>> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(
        self,
        _: &PreflightLayer<NotProvided>,
    ) -> Result<Self::Output, Self::Error> {
        if self.cycles.is_some() && self.journal.is_some() {
            return Ok(self);
        }

        bail!("cannot preflight program without downloader")
    }
}
