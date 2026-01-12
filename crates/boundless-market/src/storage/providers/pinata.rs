// Copyright 2025 Boundless Foundation, Inc.
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
//!
//! This is an **upload-only** provider. Downloads from the resulting HTTPS URLs
//! should be handled by the HTTP downloader.

use std::env::{self, VarError};

use crate::storage::{StorageError, StorageUploader, StorageUploaderConfig, StorageUploaderType};
use anyhow::anyhow;
use async_trait::async_trait;
use reqwest::{
    multipart::{Form, Part},
    Client,
};
use url::Url;

const DEFAULT_PINATA_API_URL: &str = "https://uploads.pinata.cloud";
const DEFAULT_GATEWAY_URL: &str = "https://gateway.pinata.cloud";

/// Pinata storage provider for uploading to IPFS.
///
/// This provider uploads files to IPFS via Pinata's API and returns
/// HTTPS gateway URLs for accessing the uploaded content.
///
/// # Note
///
/// This is an upload-only provider. The returned URLs are HTTPS URLs
/// that should be downloaded using the HTTP downloader.
#[derive(Clone, Debug)]
pub struct PinataStorageUploader {
    client: Client,
    pinata_jwt: String,
    pinata_api_url: Url,
    ipfs_gateway_url: Url,
}

impl PinataStorageUploader {
    /// Creates a new Pinata storage provider from environment variables.
    ///
    /// Required environment variables:
    /// - `PINATA_JWT`: Pinata API JWT token
    ///
    /// Optional environment variables:
    /// - `PINATA_API_URL`: Pinata API URL (default: https://uploads.pinata.cloud)
    /// - `IPFS_GATEWAY_URL`: IPFS gateway URL (default: https://gateway.pinata.cloud)
    pub fn from_env() -> Result<Self, StorageError> {
        let jwt = env::var("PINATA_JWT")?;
        if jwt.is_empty() {
            return Err(StorageError::Other(anyhow!("PINATA_JWT must be non-empty")));
        }

        let api_url_str = match env::var("PINATA_API_URL") {
            Ok(url) => url,
            Err(VarError::NotPresent) => DEFAULT_PINATA_API_URL.to_string(),
            Err(e) => return Err(e.into()),
        };
        if api_url_str.is_empty() {
            return Err(StorageError::Other(anyhow!("PINATA_API_URL must be non-empty")));
        }
        let api_url = Url::parse(&api_url_str)?;

        let gateway_url_str = match env::var("IPFS_GATEWAY_URL") {
            Ok(url) => url,
            Err(VarError::NotPresent) => DEFAULT_GATEWAY_URL.to_string(),
            Err(e) => return Err(e.into()),
        };
        let gateway_url = Url::parse(&gateway_url_str)?;

        Ok(Self {
            client: Client::new(),
            pinata_jwt: jwt,
            pinata_api_url: api_url,
            ipfs_gateway_url: gateway_url,
        })
    }

    /// Creates a new Pinata storage provider from configuration.
    pub fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_provider, StorageUploaderType::Pinata);

        let jwt = config
            .pinata_jwt
            .clone()
            .ok_or_else(|| StorageError::MissingConfig("pinata_jwt".to_string()))?;

        let api_url = config
            .pinata_api_url
            .clone()
            .unwrap_or_else(|| Url::parse(DEFAULT_PINATA_API_URL).unwrap());

        let gateway_url = config
            .ipfs_gateway_url
            .clone()
            .unwrap_or_else(|| Url::parse(DEFAULT_GATEWAY_URL).unwrap());

        Ok(Self {
            client: Client::new(),
            pinata_jwt: jwt,
            pinata_api_url: api_url,
            ipfs_gateway_url: gateway_url,
        })
    }

    /// Creates a new Pinata storage provider with explicit parameters.
    pub fn new(jwt: String, api_url: Url, gateway_url: Url) -> Self {
        Self {
            client: Client::new(),
            pinata_jwt: jwt,
            pinata_api_url: api_url,
            ipfs_gateway_url: gateway_url,
        }
    }

    /// Upload data to Pinata and return an IPFS gateway URL.
    async fn upload(&self, data: &[u8], filename: &str) -> Result<Url, StorageError> {
        let url = self.pinata_api_url.join("/v3/files")?;

        let form = Form::new()
            .part(
                "file",
                Part::bytes(data.to_vec())
                    .mime_str("application/octet-stream")
                    .map_err(|e| StorageError::http(e))?
                    .file_name(filename.to_string()),
            )
            .part("network", Part::text("public"));

        let request = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.pinata_jwt))
            .multipart(form)
            .build()
            .map_err(|e| StorageError::http(e))?;

        tracing::debug!("Sending Pinata upload request: {}", request.url());

        let response = self.client.execute(request).await.map_err(|e| StorageError::http(e))?;

        tracing::debug!("Pinata response status: {}", response.status());

        let response = response.error_for_status().map_err(|e| StorageError::http(e))?;

        let json_value: serde_json::Value =
            response.json().await.map_err(|e| StorageError::http(e))?;

        let ipfs_hash = json_value
            .as_object()
            .ok_or_else(|| StorageError::Other(anyhow!("response is not a JSON object")))?
            .get("data")
            .ok_or_else(|| StorageError::Other(anyhow!("response missing 'data' field")))?
            .get("cid")
            .ok_or_else(|| StorageError::Other(anyhow!("response missing 'data.cid' field")))?
            .as_str()
            .ok_or_else(|| StorageError::Other(anyhow!("invalid IPFS hash type")))?;

        let data_url = self.ipfs_gateway_url.join(&format!("ipfs/{ipfs_hash}"))?;
        Ok(data_url)
    }
}

#[async_trait]
impl StorageUploader for PinataStorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.upload(data, key).await
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HttpDownloader, StorageDownloader, StorageUploader};

    #[tokio::test]
    #[ignore = "requires PINATA_JWT credentials"]
    async fn test_pinata_roundtrip() {
        let uploader = PinataStorageUploader::from_env().expect("failed to create Pinata uploader");

        let test_data = b"pinata integration test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");

        assert!(
            url.scheme() == "https" || url.scheme() == "http",
            "expected HTTPS gateway URL, got {}",
            url.scheme()
        );

        let downloader = HttpDownloader::default();
        let downloaded = downloader.download_url(&url).await.expect("download failed");

        assert_eq!(downloaded, test_data);
    }
}
*/
