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
//! - **Uploading**: Store programs and inputs in GCS buckets (returns `gs://` URLs)
//! - **Downloading**: Fetch data from `gs://` URLs
//!
//! # Authentication
//!
//! Authentication is handled via Application Default Credentials (ADC):
//! - `GOOGLE_APPLICATION_CREDENTIALS` environment variable pointing to a service account key
//! - Workload Identity on GKE
//! - Default credentials on Compute Engine, Cloud Run, Cloud Functions, etc.
//! - `gcloud auth application-default login` for local development
//!
//! # Endpoint URL
//!
//! The endpoint URL is optional and only needed for testing with emulators
//! like fake-gcs-server. In production, the default GCS endpoint is used.
//!
//! # Public Buckets
//!
//! For downloading from public buckets, anonymous credentials are used.
//! This allows accessing publicly readable objects without authentication.
//!
//! # Presigned URLs
//!
//! TODO: Presigned URL support is not yet available in the google-cloud-storage
//! Rust SDK. This feature is being actively developed. For now, uploaded objects
//! should be made publicly readable, or downloaders should have appropriate
//! GCS permissions.

use std::env::{self, VarError};

use crate::storage::{
    config::StorageUploaderConfig,
    error::StorageError,
    traits::{StorageDownloader, StorageUploader},
    StorageUploaderType,
};
use alloy_primitives::bytes;
use async_trait::async_trait;
use google_cloud_auth::credentials::anonymous;
use google_cloud_gax::retry_policy::{AlwaysRetry, NeverRetry, RetryPolicyExt};
use google_cloud_storage::client::Storage;
use url::Url;

/// GCS storage uploader for uploading programs and inputs.
///
/// This provider stores files in a GCS bucket and returns `gs://` URLs.
///
/// # Authentication
///
/// Uses Application Default Credentials (ADC). No explicit credentials needed
/// if running on GCP infrastructure with an appropriate service account, or if
/// credentials are configured via `GOOGLE_APPLICATION_CREDENTIALS` or
/// `gcloud auth application-default login`.
///
/// # Bucket Requirements
///
/// The bucket must already exist and be accessible with the configured credentials.
/// For public access to uploaded objects, configure the bucket's IAM policy to
/// grant `roles/storage.objectViewer` to `allUsers`.
#[derive(Clone, Debug)]
pub struct GcsStorageUploader {
    bucket: String,
    client: Storage,
}

impl GcsStorageUploader {
    /// Creates a new GCS storage uploader from environment variables.
    ///
    /// Required environment variables:
    /// - `GCS_BUCKET`: The name of the GCS bucket
    ///
    /// Optional environment variables:
    /// - `GCS_URL`: Custom endpoint URL (for emulators like fake-gcs-server)
    /// - `GOOGLE_APPLICATION_CREDENTIALS`: Path to service account JSON key file
    pub async fn from_env() -> Result<Self, StorageError> {
        let bucket = env::var("GCS_BUCKET")?;
        let endpoint_url = match env::var("GCS_URL") {
            Ok(url) => Some(url),
            Err(VarError::NotPresent) => None,
            Err(e) => return Err(e.into()),
        };

        Self::new(bucket, endpoint_url).await
    }

    /// Creates a new GCS storage uploader from configuration.
    pub async fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_uploader, StorageUploaderType::Gcs);

        let bucket = config
            .gcs_bucket
            .clone()
            .ok_or_else(|| StorageError::MissingConfig("gcs_bucket".to_string()))?;

        Self::new(bucket, config.gcs_url.clone()).await
    }

    /// Creates a new GCS storage uploader with explicit parameters.
    ///
    /// # Arguments
    ///
    /// * `bucket` - The GCS bucket name (must already exist)
    /// * `endpoint_url` - Custom endpoint URL (optional, for emulators)
    pub async fn new(bucket: String, endpoint_url: Option<String>) -> Result<Self, StorageError> {
        let mut builder = Storage::builder();

        if let Some(ref url) = endpoint_url {
            builder = builder.with_endpoint(url);
        }

        let client = builder.build().await.map_err(|e| StorageError::Other(e.into()))?;

        Ok(Self { bucket, client })
    }

    /// Uploads data to GCS and returns a `gs://` URL.
    async fn upload(&self, data: bytes::Bytes, key: &str) -> Result<Url, StorageError> {
        tracing::debug!(?key, bucket = %self.bucket, "uploading to GCS");

        let bucket_path = format!("projects/_/buckets/{}", self.bucket);
        self.client.write_object(&bucket_path, key, data).send_unbuffered().await?;

        let base = format!("gs://{}/", self.bucket);
        let mut url = Url::parse(&base)
            .map_err(|_| StorageError::InvalidUrl("invalid bucket name for GCS URL"))?;
        url.set_path(key);
        tracing::debug!(?url, "uploaded to GCS");
        Ok(url)
    }
}

#[async_trait]
impl StorageUploader for GcsStorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.upload(bytes::Bytes::copy_from_slice(data), key).await
    }
}

/// GCS downloader for fetching data from `gs://` URLs.
///
/// This downloader attempts to use Application Default Credentials (ADC) first,
/// falling back to anonymous credentials if ADC is not available. This allows:
/// - Private bucket access when ADC is configured
/// - Public bucket access when no credentials are available
///
/// # Authentication Priority
///
/// 1. Application Default Credentials (ADC) - for private buckets
/// 2. Anonymous credentials - for public buckets (fallback)
///
/// # Public Bucket Access
///
/// To make a GCS bucket publicly readable:
/// 1. Go to the bucket in the GCP Console
/// 2. Go to Permissions tab
/// 3. Grant `roles/storage.objectViewer` to `allUsers`
#[derive(Clone, Debug)]
pub struct GcsStorageDownloader {
    client: Storage,
}

impl GcsStorageDownloader {
    /// Creates a new GCS downloader with optional retry configuration.
    pub async fn new(max_retries: Option<u8>) -> Self {
        let endpoint_url = env::var("GCS_URL").ok();

        // Try with Application Default Credentials first
        if let Ok(client) = Self::build_client(endpoint_url.clone(), max_retries, false).await {
            tracing::debug!("GCS downloader using Application Default Credentials");
            return Self { client };
        }

        // Fall back to anonymous credentials for public bucket access
        tracing::debug!("GCS downloader falling back to anonymous credentials");
        let client = Self::build_client(endpoint_url, max_retries, true)
            .await
            .expect("anonymous GCS client should always succeed");
        Self { client }
    }

    /// Build a GCS client with the specified settings.
    async fn build_client(
        endpoint_url: Option<String>,
        max_retries: Option<u8>,
        anonymous: bool,
    ) -> Result<Storage, StorageError> {
        let mut builder = Storage::builder();

        if let Some(url) = endpoint_url {
            builder = builder.with_endpoint(url);
        }

        builder = if let Some(max_retries) = max_retries {
            builder.with_retry_policy(AlwaysRetry.with_attempt_limit(max_retries as u32))
        } else {
            builder.with_retry_policy(NeverRetry)
        };

        if anonymous {
            builder = builder.with_credentials(anonymous::Builder::new().build());
        }

        builder.build().await.map_err(|e| StorageError::Other(e.into()))
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

        let bucket_name = format!("projects/_/buckets/{}", bucket);
        let mut stream = self.client.read_object(bucket_name, key).send().await?;

        // NOTE: Unlike S3, the GCS SDK's read_object doesn't return content-length in the
        // response metadata. Checking size upfront would require a separate get_object call
        // via StorageControl, adding an extra API request. Instead, we check during streaming.
        let mut buffer = Vec::new();
        while let Some(chunk) = stream.next().await.transpose()? {
            buffer.extend_from_slice(&chunk);
            if buffer.len() > limit {
                return Err(StorageError::SizeLimitExceeded { size: 0, limit });
            }
        }

        Ok(buffer)
    }
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{StorageDownloader, StorageDownloaderConfig, StorageUploader};

    #[tokio::test]
    #[ignore = "requires GCS_BUCKET and Application Default Credentials"]
    async fn test_gcs_roundtrip() {
        let uploader = GcsStorageUploader::from_env().await.expect("failed to create GCS uploader");

        let test_data = b"gcs integration test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");

        assert_eq!(url.scheme(), "gs", "expected gs:// URL");

        let downloader = GcsStorageDownloader::from_config(&StorageDownloaderConfig::default())
            .await
            .expect("failed to create GCS downloader");
        let downloaded = downloader.download_url(&url).await.expect("download failed");

        assert_eq!(downloaded, test_data);
    }
}
*/
