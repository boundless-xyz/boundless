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

//! Provider implementation for uploading and downloading from AWS S3.
//!
//! This module supports both:
//! - **Uploading**: Store programs and inputs in S3 buckets (requires credentials)
//! - **Downloading**: Fetch data from `s3://` URLs (supports anonymous access for public buckets)
//!
//! # Environment Variables
//!
//! The following environment variables are used:
//! - `S3_BUCKET`: Required bucket name for uploads
//! - `S3_URL`: Optional custom endpoint URL (for S3-compatible services like MinIO)
//! - `S3_NO_PRESIGNED`: If set, return `s3://` URLs instead of presigned HTTPS URLs
//!
//! # Authentication & Region
//!
//! Region and credentials are resolved via the AWS SDK default provider chain:
//! - Environment variables (`AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
//! - `~/.aws/credentials` and `~/.aws/config`
//! - IAM role (on EC2, ECS, Lambda, etc.)
//! - IAM role assumption via `AWS_ROLE_ARN`
//!
//! ## Downloading
//!
//! Downloading supports both authenticated and anonymous access:
//! - If credentials are available, they are used (for private buckets)
//! - If no credentials are available, anonymous access is used (for public buckets)
//!
//! # Presigned URLs
//!
//! By default, uploads return presigned HTTPS URLs with the maximum expiration time of
//! 7 days. Note that if using temporary credentials (STS, SSO), URLs may expire earlier
//! when the underlying credentials expire. Set `S3_NO_PRESIGNED` to return `s3://` URLs
//! instead.

use std::{env, time::Duration};

use crate::storage::{
    StorageDownloader, StorageError, StorageUploader, StorageUploaderConfig, StorageUploaderType,
};
use alloy::primitives::bytes::Buf;
use async_trait::async_trait;
use aws_config::{defaults, retry::RetryConfig, BehaviorVersion, SdkConfig};
use aws_sdk_s3::{
    config::{ProvideCredentials, Region, SharedCredentialsProvider},
    presigning::PresigningConfig,
    primitives::ByteStream,
    Client as S3Client,
};
use url::Url;

const ENV_VAR_ROLE_ARN: &str = "AWS_ROLE_ARN";
const ENV_VAR_S3_BUCKET: &str = "S3_BUCKET";
const ENV_VAR_S3_URL: &str = "S3_URL";
const ENV_VAR_S3_NO_PRESIGNED: &str = "S3_NO_PRESIGNED";

/// Maximum expiration time for S3 presigned URLs (7 days).
///
/// This is the maximum value allowed by S3. Note that if using temporary credentials
/// (e.g., STS, SSO), the URL will expire when the credentials expire, regardless of
/// this setting.
const PRESIGNED_URL_EXPIRY: Duration = Duration::from_secs(604800);

/// Apply IAM role assumption if `AWS_ROLE_ARN` is set.
///
/// This is used for cross-account access and proper access control in production.
async fn apply_role_assumption(sdk_config: SdkConfig) -> SdkConfig {
    let Ok(role_arn) = env::var(ENV_VAR_ROLE_ARN) else {
        return sdk_config;
    };

    tracing::debug!(%role_arn, "Assuming IAM role for S3 access");

    let role_provider =
        aws_config::sts::AssumeRoleProvider::builder(role_arn).configure(&sdk_config).build().await;

    sdk_config
        .into_builder()
        .credentials_provider(SharedCredentialsProvider::new(role_provider))
        .build()
}

/// S3 storage uploader for uploading programs and inputs.
///
/// This provider stores files in an S3 bucket and returns either:
/// - `s3://` URLs (when `use_presigned` is false)
/// - Presigned HTTPS URLs (when `use_presigned` is true, default)
///
/// # Authentication
///
/// Uses the **AWS default credential chain** for authentication:
/// - Environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
/// - `~/.aws/credentials` and `~/.aws/config`
/// - IAM role (on EC2, ECS, Lambda, etc.)
///
/// Explicit credentials can also be provided via [`StorageUploaderConfig`] for
/// programmatic use cases (e.g., dedicated S3 credentials for sensitive inputs).
#[derive(Clone, Debug)]
pub struct S3StorageUploader {
    bucket: String,
    client: S3Client,
    use_presigned: bool,
}

impl S3StorageUploader {
    /// Creates a new S3 storage uploader from environment variables.
    ///
    /// Required environment variables:
    /// - `S3_BUCKET`: Bucket name
    ///
    /// Optional environment variables:
    /// - `S3_URL`: Custom S3 endpoint URL (for MinIO, LocalStack, etc.)
    /// - `S3_NO_PRESIGNED`: If set, return `s3://` URLs instead of presigned URLs
    ///
    /// Region and credentials are resolved via the AWS SDK default provider chain:
    /// - Region: `AWS_REGION`, `~/.aws/config`, or instance metadata
    /// - Credentials: `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, `~/.aws/credentials`, or IAM role
    pub async fn from_env() -> Result<Self, StorageError> {
        let bucket = env::var(ENV_VAR_S3_BUCKET)?;
        let endpoint_url = env::var(ENV_VAR_S3_URL).ok();
        let use_presigned = env::var_os(ENV_VAR_S3_NO_PRESIGNED).is_none();

        Self::new(bucket, endpoint_url, None, None, use_presigned).await
    }

    /// Creates a new S3 storage uploader from configuration.
    pub async fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_uploader, StorageUploaderType::S3);

        let bucket = config
            .s3_bucket
            .clone()
            .ok_or_else(|| StorageError::MissingConfig("s3_bucket".to_string()))?;

        let use_presigned = config.s3_use_presigned.unwrap_or(true);

        // Use explicit credentials from config if provided
        let credentials = match (&config.s3_access_key, &config.s3_secret_key) {
            (Some(access_key), Some(secret_key)) => Some((access_key.clone(), secret_key.clone())),
            _ => None,
        };

        Self::new(
            bucket,
            config.s3_url.clone(),
            config.aws_region.clone(),
            credentials,
            use_presigned,
        )
        .await
    }

    /// Creates a new S3 storage uploader with explicit parameters.
    ///
    /// # Arguments
    ///
    /// * `bucket` - The S3 bucket name
    /// * `endpoint_url` - Custom endpoint URL (optional, for S3-compatible services)
    /// * `region` - AWS region (optional, resolved from default chain if None)
    /// * `credentials` - Explicit (access_key, secret_key) tuple (optional, uses default chain if None)
    /// * `use_presigned` - Whether to return presigned HTTPS URLs (true) or `s3://` URLs (false)
    pub async fn new(
        bucket: String,
        endpoint_url: Option<String>,
        region: Option<String>,
        credentials: Option<(String, String)>,
        use_presigned: bool,
    ) -> Result<Self, StorageError> {
        let mut config_loader = defaults(BehaviorVersion::latest());

        if let Some(region) = region {
            config_loader = config_loader.region(Region::new(region));
        }

        // Use explicit credentials if provided, otherwise use default chain
        if let Some((access_key, secret_key)) = credentials {
            let creds = aws_sdk_s3::config::Credentials::new(
                access_key,
                secret_key,
                None,
                None,
                "boundless-storage",
            );
            config_loader = config_loader.credentials_provider(creds);
        }

        let sdk_config = config_loader.load().await;

        let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&sdk_config);

        if let Some(url) = endpoint_url {
            s3_config_builder = s3_config_builder.endpoint_url(url).force_path_style(true);
        }

        let client = S3Client::from_conf(s3_config_builder.build());

        Ok(Self { bucket, client, use_presigned })
    }

    /// Upload data to S3 and return a URL.
    async fn upload(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        let byte_stream = ByteStream::from(data.to_vec());

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(byte_stream)
            .send()
            .await
            .map_err(StorageError::s3)?;

        if !self.use_presigned {
            let base = format!("s3://{}/", self.bucket);
            let mut url = Url::parse(&base)
                .map_err(|_| StorageError::InvalidUrl("invalid bucket name for S3 URL"))?;
            url.set_path(key);
            return Ok(url);
        }

        // Generate presigned URL
        let presigned_request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(PRESIGNED_URL_EXPIRY)?)
            .await
            .map_err(StorageError::s3)?;

        Ok(Url::parse(presigned_request.uri())?)
    }
}

#[async_trait]
impl StorageUploader for S3StorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.upload(data, key).await
    }
}

/// S3 downloader for fetching data from `s3://` URLs.
///
/// This downloader supports both authenticated and anonymous access:
/// - If AWS credentials are available (via environment, config, or IAM role), they are used.
/// - If no credentials are available, anonymous access is used for public buckets.
///
/// For public S3 buckets, the data is typically accessed via presigned HTTPS URLs
/// (generated by the uploader) rather than `s3://` URLs, so the HTTP downloader
/// handles those cases.
#[derive(Clone, Debug)]
pub struct S3StorageDownloader {
    client: S3Client,
}

impl S3StorageDownloader {
    /// Creates a new S3 downloader with optional retry configuration.
    pub async fn new(max_retries: Option<u8>) -> Self {
        let endpoint_url = env::var(ENV_VAR_S3_URL).ok();
        let client = Self::build_client(endpoint_url, max_retries).await;

        Self { client }
    }

    async fn build_client(endpoint_url: Option<String>, max_retries: Option<u8>) -> S3Client {
        let retry_config = if let Some(max_retries) = max_retries {
            RetryConfig::standard().with_max_attempts(max_retries as u32 + 1)
        } else {
            RetryConfig::disabled()
        };

        let config_loader = defaults(BehaviorVersion::latest()).retry_config(retry_config);

        // Check if credentials are available
        let sdk_config_check = defaults(BehaviorVersion::latest()).load().await;
        let has_credentials = if let Some(provider) = sdk_config_check.credentials_provider() {
            provider.provide_credentials().await.is_ok()
        } else {
            false
        };

        let sdk_config = if has_credentials {
            tracing::debug!("Using AWS credentials for S3 downloads");
            let sdk_config = config_loader.load().await;
            // Apply IAM role assumption if configured
            apply_role_assumption(sdk_config).await
        } else {
            tracing::debug!("No AWS credentials found, using anonymous access for S3 downloads");
            config_loader.no_credentials().load().await
        };

        let mut s3_config_builder = aws_sdk_s3::config::Builder::from(&sdk_config);

        if let Some(url) = endpoint_url {
            s3_config_builder = s3_config_builder.endpoint_url(url).force_path_style(true);
        }

        S3Client::from_conf(s3_config_builder.build())
    }
}

#[async_trait]
impl StorageDownloader for S3StorageDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        if url.scheme() != "s3" {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        let bucket = url.host_str().ok_or(StorageError::InvalidUrl("missing bucket"))?;
        let key = url.path().trim_start_matches('/');
        if key.is_empty() {
            return Err(StorageError::InvalidUrl("empty key"));
        }

        let resp =
            self.client.get_object().bucket(bucket).key(key).send().await.map_err(|sdk_err| {
                tracing::debug!(error = %sdk_err, "S3 GetObject failed");
                StorageError::s3(sdk_err)
            })?;

        // Check size from content length
        let capacity = resp.content_length.unwrap_or_default() as usize;
        if capacity > limit {
            return Err(StorageError::SizeLimitExceeded { size: capacity, limit });
        }

        // Stream the response
        let mut buffer = Vec::with_capacity(capacity);
        let mut stream = resp.body;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StorageError::s3)?;
            buffer.extend_from_slice(chunk.chunk());
            if buffer.len() > limit {
                return Err(StorageError::SizeLimitExceeded { size: buffer.len(), limit });
            }
        }

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HttpDownloader, StorageDownloader, StorageUploader};

    #[tokio::test]
    #[ignore = "requires S3_BUCKET and AWS credentials (env or ~/.aws/credentials)"]
    async fn test_s3_roundtrip_presigned() {
        temp_env::async_with_vars([(ENV_VAR_S3_NO_PRESIGNED, None::<&str>)], async {
            let uploader =
                S3StorageUploader::from_env().await.expect("failed to create S3 uploader");

            let test_data = b"s3 presigned test data";
            let url = uploader.upload_input(test_data).await.expect("upload failed");

            assert!(url.scheme() == "https" || url.scheme() == "http", "expected presigned URL");

            let downloader = HttpDownloader::default();
            let downloaded = downloader.download_url(url).await.expect("download failed");

            assert_eq!(downloaded, test_data);
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "requires S3_BUCKET and AWS credentials (env or ~/.aws/credentials)"]
    async fn test_s3_roundtrip_s3_scheme() {
        temp_env::async_with_vars([(ENV_VAR_S3_NO_PRESIGNED, Some("1"))], async {
            let uploader =
                S3StorageUploader::from_env().await.expect("failed to create S3 uploader");

            let test_data = b"s3 scheme test data";
            let url = uploader.upload_input(test_data).await.expect("upload failed");

            assert_eq!(url.scheme(), "s3", "expected s3:// URL");

            let downloader = S3StorageDownloader::new(None).await;
            let downloaded = downloader.download_url(url).await.expect("download failed");

            assert_eq!(downloaded, test_data);
        })
        .await;
    }
}
