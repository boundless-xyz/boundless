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

use super::{Adapt, Layer, RequestParams};
use crate::{
    contracts::RequestInput,
    input::{CompressionOptions, GuestEnv},
    storage::StorageUploader,
    util::NotProvided,
    StandardUploader,
};
use anyhow::{bail, Context};
use derive_builder::Builder;
use url::Url;

/// Configuration for the [StorageLayer].
///
/// Controls how programs and inputs are handled during request building.
#[non_exhaustive]
#[derive(Clone, Builder)]
pub struct StorageLayerConfig {
    /// Maximum number of bytes to send as an inline input.
    ///
    /// Inputs larger than this size will be uploaded using the given storage uploader. Set to none
    /// to indicate that inputs should always be sent inline.
    #[builder(setter(into), default = "Some(2048)")]
    pub inline_input_max_bytes: Option<usize>,

    /// Compression options for encoding inputs. When `Some`, inputs are zstd-compressed (V2).
    /// When `None` (default), uncompressed V1 encoding is used.
    #[builder(default)]
    pub compression: Option<CompressionOptions>,
}

/// A layer responsible for storing programs and inputs.
///
/// This layer handles the preparation of program and input data for the proof request.
/// It can upload large programs and inputs to external storage, or include smaller
/// inputs directly in the request as inline data.
#[non_exhaustive]
#[derive(Clone)]
pub struct StorageLayer<U = StandardUploader> {
    /// [StorageUploader] used to upload programs and inputs.
    ///
    /// If not provided, the layer cannot upload files and provided inputs must be no larger than
    /// [StorageLayerConfig::inline_input_max_bytes].
    pub uploader: Option<U>,

    /// Configuration controlling storage behavior.
    pub config: StorageLayerConfig,
}

impl StorageLayerConfig {
    /// Creates a new builder for constructing a [StorageLayerConfig].
    ///
    /// This provides a way to customize storage behavior, such as
    /// the maximum size for inline inputs.
    pub fn builder() -> StorageLayerConfigBuilder {
        Default::default()
    }

    /// Encode the guest environment according to the compression config.
    fn encode_env(&self, env: &GuestEnv) -> anyhow::Result<Vec<u8>> {
        match &self.compression {
            Some(opts) => env
                .encode_compressed(opts)
                .context("failed to compress and encode guest environment"),
            None => env.encode().context("failed to encode guest environment"),
        }
    }
}

impl<S: Clone> From<Option<S>> for StorageLayer<S> {
    /// Creates a [StorageLayer] from the given [StandardUploader], using default values for all
    /// other fields.
    ///
    /// Provided value is an [Option] such that whether the storage uploader is available can be
    /// reolved at runtime (e.g. from environment variables).
    fn from(uploader: Option<S>) -> Self {
        Self { uploader, config: Default::default() }
    }
}

impl<S> From<StorageLayerConfig> for StorageLayer<S>
where
    S: StorageUploader + Default,
{
    fn from(config: StorageLayerConfig) -> Self {
        Self { uploader: Some(Default::default()), config }
    }
}

impl<S> Default for StorageLayer<S> {
    fn default() -> Self {
        Self { uploader: None, config: Default::default() }
    }
}

impl From<StorageLayerConfig> for StorageLayer<NotProvided> {
    fn from(config: StorageLayerConfig) -> Self {
        Self { uploader: None, config }
    }
}

impl Default for StorageLayerConfig {
    fn default() -> Self {
        Self::builder().build().expect("implementation error in Default for StorageLayerConfig")
    }
}

impl<S> StorageLayer<S>
where
    S: StorageUploader,
{
    /// Uploads a program binary and returns its URL.
    ///
    /// This method requires a configured storage uploader and will return an error
    /// if none is available.
    pub async fn process_program(&self, program: &[u8]) -> anyhow::Result<Url> {
        let storage_uploader = self
            .uploader
            .as_ref()
            .context("cannot upload program using StorageLayer with no storage_uploader")?;
        let program_url = storage_uploader.upload_program(program).await?;
        Ok(program_url)
    }

    /// Processes a guest environment into a [RequestInput].
    ///
    /// Small inputs (as determined by configuration) will be included inline in the request.
    /// Larger inputs will be uploaded to external storage, requiring a configured storage uploader.
    pub async fn process_env(&self, env: &GuestEnv) -> anyhow::Result<RequestInput> {
        let input_data = self.config.encode_env(env)?;
        let request_input = match self.config.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                let storage_uploader = self.uploader.as_ref().with_context( || {
                    format!("cannot upload input using StorageLayer with no storage_uploader; input length of {} bytes exceeds inline limit of {limit} bytes", input_data.len())
                })?;
                RequestInput::url(storage_uploader.upload_input(&input_data).await?)
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
}

impl<S> StorageLayer<S> {
    /// Creates a new [StorageLayer] with the given provider and configuration.
    ///
    /// The storage uploader is used to upload programs and inputs to external storage.
    pub fn new(uploader: Option<S>, config: StorageLayerConfig) -> Self {
        Self { uploader, config }
    }

    pub(crate) async fn process_env_no_provider(
        &self,
        env: &GuestEnv,
    ) -> anyhow::Result<RequestInput> {
        let input_data = self.config.encode_env(env)?;
        let request_input = match self.config.inline_input_max_bytes {
            Some(limit) if input_data.len() > limit => {
                bail!("cannot upload input using StorageLayer with no storage_uploader; input length of {} bytes exceeds inline limit of {limit} bytes", input_data.len());
            }
            _ => RequestInput::inline(input_data),
        };
        Ok(request_input)
    }
}

impl<S> Layer<(&[u8], &GuestEnv)> for StorageLayer<S>
where
    S: StorageUploader,
{
    type Output = (Url, RequestInput);
    type Error = anyhow::Error;

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
    type Output = RequestInput;
    type Error = anyhow::Error;

    async fn process(&self, env: &GuestEnv) -> Result<Self::Output, Self::Error> {
        let request_input = self.process_env_no_provider(env).await?;
        Ok(request_input)
    }
}

impl<S> Adapt<StorageLayer<S>> for RequestParams
where
    S: StorageUploader,
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
            .context("program_url must be set when storage uploader is not provided")?;
        if params.request_input.is_none() {
            let input = layer.process_env_no_provider(params.require_env()?).await?;
            params = params.with_request_input(input);
        }
        Ok(params)
    }
}
