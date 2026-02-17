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
//! - **Downloading**: Fetch data from `s3://` URLs (requires credentials)
//!
//! See [`StorageUploaderConfig`] for configuration options.
//!
//! # Authentication & Region
//!
//! Region and credentials are resolved via the AWS SDK default provider chain:
//! - Environment variables (`AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`)
//! - `~/.aws/credentials` and `~/.aws/config`
//! - IAM role (on EC2, ECS, Lambda, etc.)
//!
//! # Downloading
//!
//! Downloading from `s3://` URLs requires AWS credentials. For public buckets,
//! use public HTTPS URLs (via `s3_public_url`) which can be downloaded with the
//! HTTP downloader without credentials.
//!
//! # Presigned URLs
//!
//! By default, uploads return presigned HTTPS URLs with the maximum expiration time of
//! 7 days. Note that if using temporary credentials (STS, SSO), URLs may expire earlier
//! when the underlying credentials expire.
//!
//! # Public URLs
//!
//! For public buckets, you can enable `s3_public_url` to return HTTPS URLs instead of
//! presigned URLs. This is useful when you don't want URLs to expire. After each upload,
//! a HEAD request verifies the object is publicly accessible. If verification fails,
//! an error is returned.

use crate::storage::{
    StorageDownloader, StorageError, StorageUploader, StorageUploaderConfig, StorageUploaderType,
};
use anyhow::Context;
use async_trait::async_trait;
use aws_config::{defaults, retry::RetryConfig, BehaviorVersion};
use aws_sdk_s3::{
    config::{ProvideCredentials, Region},
    presigning::PresigningConfig,
    primitives::ByteStream,
    Client as S3Client,
};
use std::{env, time::Duration};
use url::Url;

const ENV_VAR_S3_URL: &str = "S3_URL";

/// Maximum expiration time for S3 presigned URLs (7 days).
///
/// This is the maximum value allowed by S3. Note that if using temporary credentials
/// (e.g., STS, SSO), the URL will expire when the credentials expire, regardless of
/// this setting.
const PRESIGNED_URL_EXPIRY: Duration = Duration::from_secs(604800);

/// S3 storage uploader for uploading programs and inputs.
///
/// This provider stores files in an S3 bucket and returns either:
/// - `s3://` URLs (when `presigned` is false and `public_url` is false)
/// - Presigned HTTPS URLs (when `presigned` is true, default)
/// - Public HTTPS URLs (when `public_url` is true, takes precedence)
#[derive(Clone, Debug)]
pub struct S3StorageUploader {
    bucket: String,
    client: S3Client,
    presigned: bool,
    public_url: bool,
}

impl S3StorageUploader {
    /// Creates a new S3 storage uploader from configuration.
    pub async fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_uploader, StorageUploaderType::S3);

        let bucket =
            config.s3_bucket.clone().ok_or_else(|| StorageError::MissingConfig("s3_bucket"))?;

        let presigned = config.s3_presigned.unwrap_or(true); // default: use presigned
        let public_url = config.s3_public_url.unwrap_or(false); // default: s3:// URL

        // Use explicit credentials from config if provided (both must be set together)
        let credentials = match (&config.aws_access_key_id, &config.aws_secret_access_key) {
            (Some(access_key), Some(secret_key)) => Some((access_key.clone(), secret_key.clone())),
            (Some(_), None) => {
                return Err(StorageError::MissingConfig(
                    "aws_secret_access_key required when aws_access_key_id is set",
                ))
            }
            (None, Some(_)) => {
                return Err(StorageError::MissingConfig(
                    "aws_access_key_id required when aws_secret_access_key is set",
                ))
            }
            (None, None) => None,
        };

        Self::new(
            bucket,
            config.s3_url.clone(),
            config.aws_region.clone(),
            credentials,
            presigned,
            public_url,
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
    /// * `presigned` - Whether to return presigned HTTPS URLs (true) or `s3://` URLs (false)
    /// * `public_url` - Whether to return public HTTPS URLs (takes precedence over presigned)
    pub async fn new(
        bucket: String,
        endpoint_url: Option<String>,
        region: Option<String>,
        credentials: Option<(String, String)>,
        presigned: bool,
        public_url: bool,
    ) -> Result<Self, StorageError> {
        let mut config_loader = defaults(BehaviorVersion::latest());

        if let Some(ref region) = region {
            config_loader = config_loader.region(Region::new(region.clone()));
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

        // Validate configuration by checking bucket access
        client
            .head_bucket()
            .bucket(&bucket)
            .send()
            .await
            .with_context(|| format!("S3 bucket '{}' not accessible", &bucket))
            .map_err(StorageError::s3)?;

        Ok(Self { bucket, client, presigned, public_url })
    }

    /// Upload data to S3 and return a URL.
    ///
    /// Returns one of:
    /// - Public HTTPS URL if `public_url` is enabled (verified with HEAD request)
    /// - Presigned HTTPS URL if `presigned` is enabled (default)
    /// - `s3://` URL otherwise
    async fn upload(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        tracing::debug!(
            ?key,
            bucket = %self.bucket,
            public_url = %self.public_url,
            presigned = %self.presigned,
            "uploading to S3"
        );

        let byte_stream = ByteStream::from(data.to_vec());
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(byte_stream)
            .send()
            .await
            .map_err(StorageError::s3)?;

        let url = if self.public_url {
            // Generate presigned URL and strip signature to get public URL
            let mut url = self.presigned(key, Duration::ZERO).await?;
            url.set_query(None);

            super::verify_public_url(&url).await?;

            url
        } else if self.presigned {
            self.presigned(key, PRESIGNED_URL_EXPIRY).await?
        } else {
            let url_str = format!("s3://{}/{}", self.bucket, key);
            Url::parse(&url_str)
                .map_err(|_| StorageError::InvalidUrl("invalid bucket/key for S3 URL"))?
        };
        tracing::debug!(%url, "uploaded to S3");

        Ok(url)
    }

    async fn presigned(&self, key: &str, expires_in: Duration) -> Result<Url, StorageError> {
        let config = PresigningConfig::expires_in(expires_in).expect("valid duration");
        let request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(config)
            .await
            .map_err(StorageError::s3)?;

        Url::parse(request.uri()).map_err(|_| StorageError::s3("invalid presigned URL"))
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
/// This downloader requires AWS credentials (via environment, config, or IAM role).
/// For public S3 buckets, use public HTTPS URLs (via `s3_public_url`) or presigned
/// URLs, which can be downloaded with the HTTP downloader without S3 credentials.
#[derive(Clone, Debug)]
pub struct S3StorageDownloader {
    client: S3Client,
}

impl S3StorageDownloader {
    /// Creates a new S3 downloader with optional retry configuration.
    ///
    /// # Environment Variables
    ///
    /// - `S3_URL`: Optional custom endpoint URL (for S3-compatible services like MinIO)
    pub async fn new(max_retries: Option<u8>) -> Result<Self, StorageError> {
        let endpoint_url = env::var(ENV_VAR_S3_URL).ok();
        let client = Self::build_client(endpoint_url, max_retries).await?;

        Ok(Self { client })
    }

    async fn build_client(
        endpoint_url: Option<String>,
        max_retries: Option<u8>,
    ) -> Result<S3Client, StorageError> {
        let retry_config = match max_retries {
            Some(n) => RetryConfig::standard().with_max_attempts(n as u32 + 1),
            None => RetryConfig::disabled(),
        };

        let sdk_config =
            defaults(BehaviorVersion::latest()).retry_config(retry_config).load().await;

        // Validate credentials are available
        let credentials_provider = sdk_config
            .credentials_provider()
            .ok_or_else(|| StorageError::s3("AWS credentials missing"))?;
        credentials_provider.provide_credentials().await.map_err(StorageError::s3)?;

        let mut builder = aws_sdk_s3::config::Builder::from(&sdk_config);
        if let Some(url) = endpoint_url {
            builder = builder.endpoint_url(url).force_path_style(true);
        }

        Ok(S3Client::from_conf(builder.build()))
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

        tracing::debug!(%url, "downloading from S3");

        let resp =
            self.client.get_object().bucket(bucket).key(key).send().await.map_err(|sdk_err| {
                tracing::debug!(error = %sdk_err, "S3 GetObject failed");
                StorageError::s3(sdk_err)
            })?;

        // Check content length if available for early rejection
        let content_length = resp.content_length.unwrap_or(0).max(0) as usize;
        if content_length > limit {
            return Err(StorageError::SizeLimitExceeded { size: content_length, limit });
        }

        // Stream the response body with size checking
        let mut buffer = Vec::with_capacity(content_length.min(limit));
        let mut stream = resp.body;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StorageError::s3)?;
            if buffer.len() + chunk.len() > limit {
                return Err(StorageError::SizeLimitExceeded {
                    size: buffer.len() + chunk.len(),
                    limit,
                });
            }
            buffer.extend_from_slice(&chunk);
        }
        tracing::trace!(size = buffer.len(), %url, "downloaded from S3");

        Ok(buffer)
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        // No size limit configured; download full content
        self.download_url_with_limit(url, usize::MAX).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HttpDownloader, StorageDownloader, StorageUploader};

    #[tokio::test]
    async fn invalid_credentials() {
        S3StorageUploader::new(
            String::default(),
            None,
            Some("us-east-1".into()),
            Some((String::default(), String::default())),
            false,
            false,
        )
        .await
        .expect_err("should fail");
    }

    #[tokio::test]
    #[ignore = "requires S3_BUCKET and AWS credentials (env or ~/.aws/credentials)"]
    async fn roundtrip_presigned() {
        let uploader = setup(true, true).await;

        let test_data = b"s3 presigned test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");
        assert!(url.scheme() == "https" || url.scheme() == "http", "expected presigned URL");

        let downloader = HttpDownloader::default();
        let downloaded = downloader.download_url(url).await.expect("download failed");
        assert_eq!(downloaded, test_data);
    }

    #[tokio::test]
    #[ignore = "requires S3_BUCKET and AWS credentials (env or ~/.aws/credentials)"]
    async fn roundtrip_s3_scheme() {
        let uploader = setup(false, false).await;

        let test_data = b"s3 scheme test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");
        assert_eq!(url.scheme(), "s3", "expected s3:// URL");

        let downloader = S3StorageDownloader::new(None).await.unwrap();
        let downloaded = downloader.download_url(url).await.expect("download failed");
        assert_eq!(downloaded, test_data);
    }

    #[tokio::test]
    #[ignore = "requires S3_BUCKET with public read access and AWS credentials"]
    async fn roundtrip_public_url() {
        let uploader = setup(false, true).await;

        let test_data = b"s3 public url test data";
        let url = uploader.upload_input(test_data).await.expect("upload failed");
        assert!(url.scheme() == "https" || url.scheme() == "http", "expected HTTP URL");

        let downloader = HttpDownloader::default();
        let downloaded = downloader.download_url(url).await.expect("download failed");
        assert_eq!(downloaded, test_data);
    }

    async fn setup(presigned: bool, public_url: bool) -> S3StorageUploader {
        let bucket = env::var("S3_BUCKET").expect("S3_BUCKET missing");
        let endpoint_url = env::var(ENV_VAR_S3_URL).ok();
        S3StorageUploader::new(bucket, endpoint_url, None, None, presigned, public_url)
            .await
            .expect("failed to create S3 uploader")
    }

    mod mock {
        use super::*;
        use crate::test_helpers::S3Mock;

        #[tokio::test]
        async fn roundtrip_s3_scheme() {
            let s3 = S3Mock::start().await.unwrap();
            temp_env::async_with_vars(s3.env_vars(), async move {
                let uploader = setup(&s3, false, false).await;

                let test_data = b"s3 scheme test data";
                let url = uploader.upload_input(test_data).await.expect("upload failed");
                assert_eq!(url.scheme(), "s3", "expected s3:// URL");

                let downloader = S3StorageDownloader::new(None).await.unwrap();
                let downloaded = downloader.download_url(url).await.expect("download failed");
                assert_eq!(downloaded, test_data);
            })
            .await;
        }

        #[tokio::test]
        async fn roundtrip_presigned() {
            let s3 = S3Mock::start().await.unwrap();
            temp_env::async_with_vars(s3.env_vars(), async move {
                let uploader = setup(&s3, true, false).await;

                let test_data = b"s3 presigned test data";
                let url = uploader.upload_input(test_data).await.expect("upload failed");
                assert!(url.scheme() == "https" || url.scheme() == "http", "expected HTTP URL");

                let downloader = HttpDownloader::default();
                let downloaded = downloader.download_url(url).await.expect("download failed");
                assert_eq!(downloaded, test_data);
            })
            .await;
        }

        #[tokio::test]
        async fn download_rejects_oversized_response() {
            let s3 = S3Mock::start().await.unwrap();
            temp_env::async_with_vars(s3.env_vars(), async move {
                let uploader = setup(&s3, false, false).await;

                let test_data = [0x41u8; 100];
                let url = uploader.upload_input(&test_data).await.expect("upload failed");

                let downloader = S3StorageDownloader::new(None).await.unwrap();
                let result = downloader.download_url_with_limit(url, 10).await;
                assert!(matches!(result, Err(StorageError::SizeLimitExceeded { .. })));
            })
            .await;
        }

        async fn setup(s3: &S3Mock, presigned: bool, public_url: bool) -> S3StorageUploader {
            S3StorageUploader::new(
                S3Mock::BUCKET.to_string(),
                Some(s3.endpoint().to_string()),
                Some(S3Mock::REGION.to_string()),
                Some((S3Mock::ACCESS_KEY.to_string(), S3Mock::SECRET_KEY.to_string())),
                presigned,
                public_url,
            )
            .await
            .expect("failed to create S3 uploader")
        }
    }
}
