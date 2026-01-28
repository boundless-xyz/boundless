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

//! Provider implementation for uploading and downloading from Google Cloud Storage.
//!
//! This module supports both:
//! - **Uploading**: Store programs and inputs in GCS buckets (returns `gs://` or HTTPS URLs)
//! - **Downloading**: Fetch data from `gs://` URLs (requires credentials)
//!
//! See [`StorageUploaderConfig`] for configuration options.
//!
//! # Authentication
//!
//! Credentials are resolved via the Google Cloud default credential chain (ADC):
//! - `GOOGLE_APPLICATION_CREDENTIALS` environment variable pointing to a JSON key file
//! - Well-known file locations (`~/.config/gcloud/application_default_credentials.json`)
//! - Workload Identity on GKE, metadata server on Compute Engine, etc.
//!
//! Explicit credentials can also be provided via [`StorageUploaderConfig::gcs_credentials_json`]
//! for programmatic use cases (e.g., loading from a secrets manager without writing to disk).
//!
//! # Downloading
//!
//! Downloading from `gs://` URLs requires GCS credentials (ADC). For public buckets,
//! use public HTTPS URLs (via `gcs_public_url`) which can be downloaded with the
//! HTTP downloader without credentials.
//!
//! # Public URLs
//!
//! For public buckets, you can enable `gcs_public_url` to return HTTPS URLs
//! (`https://storage.googleapis.com/{bucket}/{key}`) instead of `gs://` URLs.
//! After each upload, a HEAD request verifies the object is publicly accessible.
//! If verification fails, an error is returned.

use std::env;

use crate::storage::{
    config::StorageUploaderConfig,
    error::StorageError,
    traits::{StorageDownloader, StorageUploader},
    StorageUploaderType,
};
use alloy_primitives::bytes;
use anyhow::Context;
use async_trait::async_trait;
use google_cloud_auth::credentials::service_account;
use google_cloud_gax::retry_policy::{AlwaysRetry, NeverRetry, RetryPolicyExt};
use google_cloud_storage::client::Storage as GcsClient;
use url::Url;

const ENV_VAR_GCS_URL: &str = "GCS_URL";

/// GCS storage uploader for uploading programs and inputs.
///
/// This provider stores files in a GCS bucket and returns either:
/// - `gs://` URLs
/// - Public HTTPS URLs, for public buckets, when `public_url` is true
#[derive(Clone, Debug)]
pub struct GcsStorageUploader {
    bucket: String,
    client: GcsClient,
    public_url: bool,
}

impl GcsStorageUploader {
    /// Creates a new GCS storage uploader from configuration.
    pub async fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_uploader, StorageUploaderType::Gcs);

        let bucket =
            config.gcs_bucket.clone().ok_or_else(|| StorageError::MissingConfig("gcs_bucket"))?;

        let public_url = config.gcs_public_url.unwrap_or(false); // default: s3:// URL

        Self::new(bucket, config.gcs_url.clone(), config.gcs_credentials_json.clone(), public_url)
            .await
    }

    /// Creates a new GCS storage uploader with explicit parameters.
    ///
    /// # Arguments
    ///
    /// * `bucket` - The GCS bucket name (must already exist)
    /// * `endpoint_url` - Custom endpoint URL (optional, for emulators)
    /// * `credentials_json` - Service account JSON (optional, uses ADC if None)
    /// * `public_url` - If true, return public HTTPS URLs and verify accessibility
    pub async fn new(
        bucket: String,
        endpoint_url: Option<String>,
        credentials_json: Option<String>,
        public_url: bool,
    ) -> Result<Self, StorageError> {
        let mut builder = GcsClient::builder();

        if let Some(ref url) = endpoint_url {
            builder = builder.with_endpoint(url);
        }

        // Use explicit credentials if provided, otherwise use default chain (ADC)
        if let Some(json) = credentials_json {
            let json_value: serde_json::Value =
                serde_json::from_str(&json).context("invalid credentials json")?;
            let credentials =
                service_account::Builder::new(json_value).build().map_err(StorageError::gcs)?;
            builder = builder.with_credentials(credentials);
        }

        let client = builder.build().await.map_err(StorageError::gcs)?;

        Ok(Self { bucket, client, public_url })
    }

    /// Uploads data to GCS and returns a URL.
    ///
    /// If `public_url` is enabled, returns a public HTTPS URL after verifying
    /// the object is publicly accessible. Otherwise, returns a `gs://` URL.
    async fn upload(&self, data: bytes::Bytes, key: &str) -> Result<Url, StorageError> {
        tracing::debug!(?key, bucket = %self.bucket, public_url = %self.public_url, "uploading to GCS");

        let bucket_path = format!("projects/_/buckets/{}", self.bucket);
        self.client
            .write_object(&bucket_path, key, data)
            .send_unbuffered()
            .await
            .map_err(StorageError::gcs)?;

        let url = gcs_url(&self.bucket, key, self.public_url)?;
        if self.public_url {
            super::verify_public_url(&url).await?;
        }
        tracing::debug!(%url, "uploaded to GCS");

        Ok(url)
    }
}

fn gcs_url(bucket: &str, key: &str, public: bool) -> Result<Url, StorageError> {
    let url_str = if public {
        format!("https://storage.googleapis.com/{}/{}", bucket, key)
    } else {
        format!("gs://{}/{}", bucket, key)
    };
    Url::parse(&url_str).map_err(|_| StorageError::InvalidUrl("invalid bucket/key for GCS URL"))
}

#[async_trait]
impl StorageUploader for GcsStorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.upload(bytes::Bytes::copy_from_slice(data), key).await
    }
}

/// GCS downloader for fetching data from `gs://` URLs.
///
/// This downloader requires GCS credentials (ADC via environment, config, or workload
/// identity). For public buckets, use public HTTPS URLs (via `gcs_public_url`) which
/// can be downloaded with the HTTP downloader without GCS credentials.
#[derive(Clone, Debug)]
pub struct GcsStorageDownloader {
    client: GcsClient,
}

impl GcsStorageDownloader {
    /// Creates a new GCS downloader with optional retry configuration.
    ///
    /// # Environment Variables
    ///
    /// - `GCS_URL`: Optional custom endpoint URL (for emulators like fake-gcs-server)
    pub async fn new(max_retries: Option<u8>) -> Result<Self, StorageError> {
        let endpoint_url = env::var(ENV_VAR_GCS_URL).ok();
        let client = Self::build_client(endpoint_url, max_retries).await?;

        Ok(Self { client })
    }

    /// Build a GCS client with the specified settings.
    async fn build_client(
        endpoint_url: Option<String>,
        max_retries: Option<u8>,
    ) -> Result<GcsClient, StorageError> {
        let mut builder = GcsClient::builder();

        if let Some(url) = endpoint_url {
            builder = builder.with_endpoint(url);
        }

        builder = if let Some(max_retries) = max_retries {
            builder.with_retry_policy(AlwaysRetry.with_attempt_limit(max_retries as u32))
        } else {
            builder.with_retry_policy(NeverRetry)
        };

        let client = builder
            .build()
            .await
            .context("failed to initialize GCS client")
            .map_err(StorageError::gcs)?;

        Ok(client)
    }
}

#[async_trait]
impl StorageDownloader for GcsStorageDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        if url.scheme() != "gs" {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        let bucket = url.host_str().ok_or(StorageError::InvalidUrl("missing bucket"))?;
        let key = url.path().trim_start_matches('/');
        if key.is_empty() {
            return Err(StorageError::InvalidUrl("empty key"));
        }

        tracing::debug!(%bucket, %key, "downloading from GCS");

        let bucket_name = format!("projects/_/buckets/{}", bucket);
        let mut stream =
            self.client.read_object(bucket_name, key).send().await.map_err(StorageError::gcs)?;

        // NOTE: Unlike S3, the GCS SDK's read_object doesn't return content-length in the
        // response metadata. Checking size upfront would require a separate get_object call
        // via StorageControl, adding an extra API request. Instead, we check during streaming.
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await.transpose().map_err(StorageError::gcs)? {
            if buffer.len() + chunk.len() > limit {
                return Err(StorageError::SizeLimitExceeded {
                    size: buffer.len() + chunk.len(),
                    limit,
                });
            }
            buffer.extend_from_slice(&chunk);
        }
        tracing::trace!(size = buffer.len(), %url, "downloaded from GCS");

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HttpDownloader, StorageDownloader, StorageUploader};

    async fn setup(public_url: bool) -> GcsStorageUploader {
        let bucket = env::var("GCS_BUCKET").expect("GCS_BUCKET missing");
        let endpoint_url = env::var(ENV_VAR_GCS_URL).ok();
        GcsStorageUploader::new(bucket, endpoint_url, None, public_url)
            .await
            .expect("failed to create GCS uploader")
    }

    #[tokio::test]
    #[ignore = "requires GCS_BUCKET and Application Default Credentials"]
    async fn roundtrip_gs_scheme() {
        let uploader = setup(false).await;

        let test_data = b"gcs scheme test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");
        assert_eq!(url.scheme(), "gs", "expected gs:// URL");

        let downloader = GcsStorageDownloader::new(None).await.unwrap();
        let downloaded = downloader.download_url(url).await.expect("download failed");
        assert_eq!(downloaded, test_data);
    }

    #[tokio::test]
    #[ignore = "requires GCS_BUCKET and Application Default Credentials"]
    async fn roundtrip_public_url() {
        let uploader = setup(true).await;

        let test_data = b"gcs public url test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");
        assert!(url.scheme() == "https" || url.scheme() == "http", "expected HTTP URL");

        let downloader = HttpDownloader::default();
        let downloaded = downloader.download_url(url).await.expect("download failed");
        assert_eq!(downloaded, test_data);
    }
}
