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

//! Provider implementation for uploading programs and inputs to IPFS via Pinata.

use std::{env::VarError, fmt::Debug, result::Result::Ok};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use reqwest::{
    multipart::{Form, Part},
    Url,
};
use sha2::{Digest as _, Sha256};

use super::{StorageProvider, StorageProviderConfig};

/// Storage provider that uploads inputs and inputs to IPFS via Pinata.
#[derive(Clone, Debug)]
pub struct PinataStorageProvider {
    client: reqwest::Client,
    pinata_jwt: String,
    pinata_api_url: Url,
    ipfs_gateway_url: Url,
}

#[derive(thiserror::Error, Debug)]
/// Error type for the Pinata storage provider.
pub enum PinataStorageProviderError {
    /// Error type for reqwest errors.
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),

    /// Error type for URL parsing errors.
    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Error type for environment variable errors.
    #[error("environment variable error: {0}")]
    EnvVar(#[from] VarError),

    /// Error type for missing configuration parameters.
    #[error("missing config parameter: {0}")]
    Config(String),

    /// Error type for other errors.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

const DEFAULT_PINATA_API_URL: &str = "https://uploads.pinata.cloud";
const DEFAULT_GATEWAY_URL: &str = "https://gateway.pinata.cloud";

impl PinataStorageProvider {
    /// Creates a new Pinata storage provider from the environment variables.
    pub fn from_env() -> Result<Self, PinataStorageProviderError> {
        let jwt = std::env::var("PINATA_JWT")
            .context("failed to fetch environment variable 'PINATA_JWT'")?;
        if jwt.is_empty() {
            return Err(anyhow!("pinata api key must be non-empty").into());
        }

        let api_url_str = match std::env::var("PINATA_API_URL") {
            Ok(string) => string,
            Err(VarError::NotPresent) => DEFAULT_PINATA_API_URL.to_string(),
            Err(e) => return Err(e.into()),
        };
        if api_url_str.is_empty() {
            return Err(anyhow!("pinata api url must be non-empty").into());
        }

        let api_url = Url::parse(&api_url_str)?;

        let gateway_url_str = match std::env::var("IPFS_GATEWAY_URL") {
            Ok(string) => string,
            Err(VarError::NotPresent) => DEFAULT_GATEWAY_URL.to_string(),
            Err(e) => return Err(e.into()),
        };
        let gateway_url = Url::parse(&gateway_url_str)?;

        let client = reqwest::Client::new();

        Ok(Self { pinata_jwt: jwt, pinata_api_url: api_url, ipfs_gateway_url: gateway_url, client })
    }

    /// Creates a new Pinata storage provider from the given parts.
    pub async fn from_parts(
        jwt: String,
        api_url: String,
        gateway_url: String,
    ) -> Result<Self, PinataStorageProviderError> {
        let api_url = Url::parse(&api_url)?;
        let gateway_url = Url::parse(&gateway_url)?;
        let client = reqwest::Client::new();

        Ok(Self { pinata_jwt: jwt, pinata_api_url: api_url, ipfs_gateway_url: gateway_url, client })
    }

    /// Creates a new Pinata storage provider from the given configuration.
    pub fn from_config(config: &StorageProviderConfig) -> Result<Self, PinataStorageProviderError> {
        Ok(PinataStorageProvider {
            pinata_jwt: config
                .pinata_jwt
                .clone()
                .ok_or_else(|| PinataStorageProviderError::Config("pinata_jwt".to_string()))?,
            pinata_api_url: config
                .pinata_api_url
                .clone()
                .unwrap_or(Url::parse(DEFAULT_PINATA_API_URL)?),
            ipfs_gateway_url: config
                .ipfs_gateway_url
                .clone()
                .unwrap_or(Url::parse(DEFAULT_GATEWAY_URL)?),
            client: reqwest::Client::new(),
        })
    }

    async fn upload(
        &self,
        data: impl AsRef<[u8]>,
        filename: impl Into<String>,
    ) -> Result<Url, PinataStorageProviderError> {
        // https://docs.pinata.cloud/api-reference/endpoint/upload-a-file
        let url = self.pinata_api_url.join("/v3/files")?;
        let form = Form::new()
            .part(
                "file",
                Part::bytes(data.as_ref().to_vec())
                    .mime_str("application/octet-stream")?
                    .file_name(filename.into()),
            )
            .part("network", Part::text("public"));

        let request = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.pinata_jwt))
            .multipart(form)
            .build()?;

        tracing::debug!("Sending upload HTTP request: {}", request.url());
        tracing::trace!("{:#?}", request);

        let response = self.client.execute(request).await?;

        tracing::debug!("Received HTTP response: status {}", response.status());
        tracing::trace!("{:#?}", response);

        let response = response.error_for_status()?;

        let json_value: serde_json::Value = response.json().await?;
        let ipfs_hash = json_value
            .as_object()
            .ok_or(anyhow!("response from Pinata is not a JSON object"))?
            .get("data")
            .ok_or(anyhow!("response from Pinata does not contain data"))?
            .get("cid")
            .ok_or(anyhow!("response from Pinata does not contain data.cid"))?
            .as_str()
            .ok_or(anyhow!("response from Pinata contains an invalid IPFS hash"))?;

        let data_url = self.ipfs_gateway_url.join(&format!("ipfs/{ipfs_hash}"))?;
        Ok(data_url)
    }
}

#[async_trait]
impl StorageProvider for PinataStorageProvider {
    type Error = PinataStorageProviderError;

    async fn upload_program(&self, program: &[u8]) -> Result<Url, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(program)?;
        let filename = format!("{}.bin", image_id);
        self.upload(program, filename).await
    }

    async fn upload_input(&self, input: &[u8]) -> Result<Url, Self::Error> {
        let digest = Sha256::digest(input);
        let filename = format!("{}.input", hex::encode(digest.as_slice()));
        self.upload(input, filename).await
    }
}
