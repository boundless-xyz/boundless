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
use crate::contracts::Input as RequestInput;
use crate::input::GuestEnv;
use crate::storage::StorageProvider;
use anyhow::Context;
use derive_builder::Builder;
use std::fmt;
use url::Url;

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct StorageLayer<S> {
    /// Maximum number of bytes to send as an inline input.
    ///
    /// Inputs larger than this size will be uploaded using the given storage provider. Set to none
    /// to indicate that inputs should always be sent inline.
    #[builder(setter(into), default = "Some(2048)")]
    pub inline_input_max_bytes: Option<usize>,
    pub storage_provider: S,
}

impl StorageLayer<()> {
    pub fn builder<S: Clone>() -> StorageLayerBuilder<S> {
        Default::default()
    }
}

impl<S> fmt::Debug for StorageLayer<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StorageLayer").finish()
    }
}

impl<S: Clone> From<S> for StorageLayer<S> {
    /// Creates a [StorageLayer] from the given [StorageProvider], using default values for all
    /// other fields.
    fn from(value: S) -> Self {
        StorageLayer::builder()
            .storage_provider(value)
            .build()
            .expect("implementation error in From<S> for StorageLayer")
    }
}

impl<S: StorageProvider> StorageLayer<S>
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn process_program(&self, program: &[u8]) -> anyhow::Result<Url> {
        let program_url = self.storage_provider.upload_program(program).await?;
        Ok(program_url)
    }

    pub async fn process_env(&self, env: &GuestEnv) -> anyhow::Result<RequestInput> {
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                RequestInput::url(self.storage_provider.upload_input(&input_data).await?)
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
}

impl<S: StorageProvider> Layer<(&[u8], &GuestEnv)> for StorageLayer<S>
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    type Error = anyhow::Error;
    type Output = (Url, RequestInput);

    async fn process(
        &self,
        (program, env): (&[u8], &GuestEnv),
    ) -> Result<Self::Output, Self::Error> {
        let program_url = self.process_program(program).await?;
        let request_input = self.process_env(env).await?;
        Ok((program_url, request_input))
    }
}

impl<S> Default for StorageLayer<S>
where
    S: StorageProvider + Default,
{
    fn default() -> Self {
        Self { inline_input_max_bytes: Some(2048), storage_provider: S::default() }
    }
}

impl<S: StorageProvider> Adapt<StorageLayer<S>> for RequestParams
where
    S::Error: std::error::Error + Send + Sync + 'static,
{
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &StorageLayer<S>) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with StorageLayer");

        let mut params = self;
        if params.program_url.is_none() {
            let program_url = layer.process_program(params.require_program()?).await?;
            params = params.with_program_url(program_url);
        }
        if params.request_input.is_none() {
            let input = layer.process_env(params.require_env()?).await?;
            params = params.with_request_input(input);
        }
        Ok(params)
    }
}
