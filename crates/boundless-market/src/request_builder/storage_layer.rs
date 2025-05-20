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
    input::GuestEnv,
    storage::{StandardStorageProvider, StorageProvider},
    util::NotProvided,
};
use anyhow::{bail, Context};
use derive_builder::Builder;
use url::Url;

#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct StorageLayerConfig {
    /// Maximum number of bytes to send as an inline input.
    ///
    /// Inputs larger than this size will be uploaded using the given storage provider. Set to none
    /// to indicate that inputs should always be sent inline.
    #[builder(setter(into), default = "Some(2048)")]
    pub inline_input_max_bytes: Option<usize>,
}

#[non_exhaustive]
#[derive(Clone)]
pub struct StorageLayer<S = StandardStorageProvider> {
    /// [StorageProvider] used to upload programs and inputs.
    ///
    /// If not provided, the layer cannot upload files and provided inputs must be no larger than
    /// [StorageLayerConfig::inline_input_max_bytes].
    pub storage_provider: Option<S>,
    pub config: StorageLayerConfig,
}

impl StorageLayerConfig {
    pub fn builder() -> StorageLayerConfigBuilder {
        Default::default()
    }
}

impl<S: Clone> From<Option<S>> for StorageLayer<S> {
    /// Creates a [StorageLayer] from the given [StorageProvider], using default values for all
    /// other fields.
    ///
    /// Provided value is an [Option] such that whether the storage provider is available can be
    /// reolved at runtime (e.g. from environment variables).
    fn from(storage_provider: Option<S>) -> Self {
        StorageLayer { storage_provider, config: Default::default() }
    }
}

impl<S> From<StorageLayerConfig> for StorageLayer<S>
where
    S: StorageProvider + Default,
{
    fn from(config: StorageLayerConfig) -> Self {
        Self { storage_provider: Some(Default::default()), config }
    }
}

impl<S> Default for StorageLayer<S>
where
    S: StorageProvider + Default,
{
    fn default() -> Self {
        StorageLayer { storage_provider: Some(Default::default()), config: Default::default() }
    }
}

impl Default for StorageLayer<NotProvided> {
    fn default() -> Self {
        StorageLayer { storage_provider: None, config: Default::default() }
    }
}

impl From<StorageLayerConfig> for StorageLayer<NotProvided> {
    fn from(config: StorageLayerConfig) -> Self {
        Self { storage_provider: None, config }
    }
}

impl Default for StorageLayerConfig {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for StorageLayerConfig")
    }
}

impl<S> StorageLayer<S>
where
    S: StorageProvider,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    pub async fn process_program(&self, program: &[u8]) -> anyhow::Result<Url> {
        let storage_provider = self
            .storage_provider
            .as_ref()
            .context("cannot upload program using StorageLayer with no storage_provider")?;
        let program_url = storage_provider.upload_program(program).await?;
        Ok(program_url)
    }

    pub async fn process_env(&self, env: &GuestEnv) -> anyhow::Result<RequestInput> {
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.config.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                let storage_provider = self.storage_provider.as_ref().with_context( || {
                    format!("cannot upload input using StorageLayer with no storage_provider; input length of {} bytes exceeds inline limit of {limit} bytes", input_data.len())
                })?;
                RequestInput::url(storage_provider.upload_input(&input_data).await?)
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
}

impl<S> StorageLayer<S> {
    pub fn new(storage_provider: Option<S>, config: StorageLayerConfig) -> Self {
        Self { storage_provider, config }
    }

    pub(crate) async fn process_env_no_provider(
        &self,
        env: &GuestEnv,
    ) -> anyhow::Result<RequestInput> {
        let input_data = env.encode().context("failed to encode guest environment")?;
        let request_input = match self.config.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                bail!("cannot upload input using StorageLayer with no storage_provider; input length of {} bytes exceeds inline limit of {limit} bytes", input_data.len());
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
}

impl<S> Layer<(&[u8], &GuestEnv)> for StorageLayer<S>
where
    S: StorageProvider,
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

impl Layer<&GuestEnv> for StorageLayer<NotProvided> {
    type Error = anyhow::Error;
    type Output = RequestInput;

    async fn process(&self, env: &GuestEnv) -> Result<Self::Output, Self::Error> {
        let request_input = self.process_env_no_provider(env).await?;
        Ok(request_input)
    }
}

impl<S> Adapt<StorageLayer<S>> for RequestParams
where
    S: StorageProvider,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(self, layer: &StorageLayer<S>) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with StorageLayer");

        let mut params = self;
        if params.program_url.is_none() {
            let program_url = layer.process_program(params.require_program()?).await?;
            params = params.with_program_url(program_url)?;
        }
        if params.request_input.is_none() {
            let input = layer.process_env(params.require_env()?).await?;
            params = params.with_request_input(input);
        }
        Ok(params)
    }
}

impl Adapt<StorageLayer<NotProvided>> for RequestParams {
    type Output = RequestParams;
    type Error = anyhow::Error;

    async fn process_with(
        self,
        layer: &StorageLayer<NotProvided>,
    ) -> Result<Self::Output, Self::Error> {
        tracing::trace!("Processing {self:?} with StorageLayer");

        let mut params = self;
        params
            .require_program_url()
            .context("program_url must be set when storage provider is not provided")?;
        if params.request_input.is_none() {
            let input = layer.process_env_no_provider(params.require_env()?).await?;
            params = params.with_request_input(input);
        }
        Ok(params)
    }
}
