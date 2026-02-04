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

//! URI handling for fetching data from HTTP, S3, and file URLs.

use alloy::primitives::bytes::Buf;
#[cfg(feature = "s3")]
use aws_config::retry::RetryConfig;
#[cfg(feature = "s3")]
use aws_sdk_s3::{
    config::{ProvideCredentials, SharedCredentialsProvider},
    error::ProvideErrorMetadata,
    Client as S3Client,
};
use futures_util::StreamExt;
use std::env;
use thiserror::Error;

use super::config::MarketConfig;

#[cfg(feature = "s3")]
const ENV_VAR_ROLE_ARN: &str = "AWS_ROLE_ARN";

/// Returns `true` if the dev mode environment variable is enabled.
fn is_dev_mode() -> bool {
    env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

/// Returns `true` if the `ALLOW_LOCAL_FILE_STORAGE` environment variable is enabled.
fn allow_local_file_storage() -> bool {
    env::var("ALLOW_LOCAL_FILE_STORAGE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

/// Returns `true` if file:// URLs are allowed based on environment variables.
fn allow_file_urls() -> bool {
    is_dev_mode() || allow_local_file_storage()
}

/// Errors that can occur during URI fetching.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum StorageError {
    /// Unsupported URI scheme.
    #[error("unsupported URI scheme: {0}")]
    UnsupportedScheme(String),

    /// Failed to parse URL.
    #[error("failed to parse URL: {0}")]
    UriParse(#[from] url::ParseError),

    /// Invalid URL.
    #[error("invalid URL: {0}")]
    InvalidUrl(&'static str),

    /// Resource size exceeds maximum allowed.
    #[error("resource size exceeds maximum allowed size ({0} bytes)")]
    SizeLimitExceeded(usize),

    /// File I/O error.
    #[error("file error: {0}")]
    File(#[from] std::io::Error),

    /// HTTP error.
    #[error("HTTP error: {0}")]
    Http(String),

    /// AWS S3 error.
    #[cfg(feature = "s3")]
    #[error("AWS S3 error: {0}")]
    S3(String),
}

/// Fetch data from a URI with default config. Supports HTTP/HTTPS and S3 schemes.
///
/// For more control over fetching behavior (size limits, retries, caching, file:// support),
/// use [`fetch_uri_with_config`] instead.
#[allow(unused)]
pub async fn fetch_uri(uri: &str) -> Result<Vec<u8>, StorageError> {
    fetch_uri_with_config(uri, &MarketConfig::default()).await
}

/// Fetch data from a URI with the given market configuration.
///
/// Supports:
/// - `http://` and `https://` URLs
/// - `s3://bucket/key` URLs (requires AWS credentials)
/// - `file://` URLs (only if `RISC0_DEV_MODE` or `ALLOW_LOCAL_FILE_STORAGE` env var is set)
pub async fn fetch_uri_with_config(
    uri: &str,
    config: &MarketConfig,
) -> Result<Vec<u8>, StorageError> {
    let parsed = url::Url::parse(uri)?;

    match parsed.scheme() {
        "file" => {
            if !allow_file_urls() {
                return Err(StorageError::UnsupportedScheme(
                    "file (not allowed in this context)".to_string(),
                ));
            }
            fetch_file(parsed.path(), Some(config.max_file_size)).await
        }
        "http" | "https" => fetch_http(parsed, config).await,
        #[cfg(feature = "s3")]
        "s3" => fetch_s3(parsed, config).await,
        scheme => Err(StorageError::UnsupportedScheme(scheme.to_string())),
    }
}

/// Fetch data from a local file.
async fn fetch_file(path: &str, max_size: Option<usize>) -> Result<Vec<u8>, StorageError> {
    let metadata = tokio::fs::metadata(path).await?;
    let size = metadata.len() as usize;

    if let Some(max) = max_size {
        if size > max {
            return Err(StorageError::SizeLimitExceeded(size));
        }
    }

    Ok(tokio::fs::read(path).await?)
}

/// Fetch data from an HTTP/HTTPS URL.
async fn fetch_http(url: url::Url, config: &MarketConfig) -> Result<Vec<u8>, StorageError> {
    use reqwest_middleware::ClientBuilder;
    use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};

    if !url.has_host() {
        return Err(StorageError::InvalidUrl("missing host"));
    }

    let mut builder = ClientBuilder::new(reqwest::Client::new());

    // Add retry middleware if configured
    if let Some(max_retries) = config.max_fetch_retries {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(max_retries as u32);
        let retry_middleware = RetryTransientMiddleware::new_with_policy(retry_policy);
        builder = builder.with(retry_middleware);
    }

    let client = builder.build();
    let response =
        client.get(url.clone()).send().await.map_err(|e| StorageError::Http(e.to_string()))?;
    let response = response.error_for_status().map_err(|e| StorageError::Http(e.to_string()))?;

    let max_size = config.max_file_size;

    // Check content-length header first
    let capacity = response.content_length().unwrap_or_default() as usize;
    if capacity > max_size {
        return Err(StorageError::SizeLimitExceeded(capacity));
    }

    // Stream the response and check size incrementally
    let mut buffer = Vec::with_capacity(capacity);
    let mut stream = response.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| StorageError::Http(e.to_string()))?;
        buffer.extend_from_slice(chunk.chunk());
        if buffer.len() > max_size {
            return Err(StorageError::SizeLimitExceeded(buffer.len()));
        }
    }

    Ok(buffer)
}

/// Fetch data from an S3 URL.
///
/// Authenticates using the default AWS credential chain (environment variables,
/// `~/.aws/credentials`, `~/.aws/config`, etc.).
///
/// If the `AWS_ROLE_ARN` environment variable is set, it will attempt to assume that
/// IAM role before accessing S3.
#[cfg(feature = "s3")]
async fn fetch_s3(url: url::Url, config: &MarketConfig) -> Result<Vec<u8>, StorageError> {
    let retry_config = if let Some(max_retries) = config.max_fetch_retries {
        RetryConfig::standard().with_max_attempts(max_retries as u32 + 1)
    } else {
        RetryConfig::disabled()
    };

    let mut aws_config = aws_config::from_env().retry_config(retry_config).load().await;

    // Verify credentials are available
    if let Some(provider) = aws_config.credentials_provider() {
        if let Err(e) = provider.provide_credentials().await {
            tracing::debug!(error=%e, "Could not load AWS credentials for S3");
            return Err(StorageError::UnsupportedScheme(format!(
                "s3 (no credentials available: {})",
                e
            )));
        }
    } else {
        return Err(StorageError::UnsupportedScheme("s3 (no credentials provider)".to_string()));
    }

    // Handle role assumption if AWS_ROLE_ARN is set
    if let Ok(role_arn) = env::var(ENV_VAR_ROLE_ARN) {
        let role_provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
            .configure(&aws_config)
            .build()
            .await;
        aws_config = aws_config
            .into_builder()
            .credentials_provider(SharedCredentialsProvider::new(role_provider))
            .build();
    }

    let bucket = url.host_str().ok_or(StorageError::InvalidUrl("missing bucket"))?;
    let key = url.path().trim_start_matches('/');
    if key.is_empty() {
        return Err(StorageError::InvalidUrl("empty key"));
    }

    let client = S3Client::new(&aws_config);
    let resp = client.get_object().bucket(bucket).key(key).send().await.map_err(|e| {
        let code = e.code().unwrap_or("unknown");
        tracing::debug!(error = %e, code = ?code, "S3 GetObject failed");
        StorageError::S3(format!("{}: {}", code, e))
    })?;

    let max_size = config.max_file_size;

    // Check content-length first
    let capacity = resp.content_length.unwrap_or_default() as usize;
    if capacity > max_size {
        return Err(StorageError::SizeLimitExceeded(capacity));
    }

    // Stream and check size incrementally
    let mut buffer = Vec::with_capacity(capacity);
    let mut stream = resp.body;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| StorageError::S3(e.to_string()))?;
        buffer.extend_from_slice(chunk.chunk());
        if buffer.len() > max_size {
            return Err(StorageError::SizeLimitExceeded(buffer.len()));
        }
    }

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_unsupported_scheme() {
        let result = fetch_uri("ftp://example.com/file").await;
        assert!(matches!(result, Err(StorageError::UnsupportedScheme(_))));
    }

    #[tokio::test]
    async fn test_invalid_url() {
        let result = fetch_uri("not a url").await;
        assert!(matches!(result, Err(StorageError::UriParse(_))));
    }

    #[tokio::test]
    async fn test_file_url_disabled_by_default() {
        // Skip test if dev mode or local file storage is enabled
        if allow_file_urls() {
            return;
        }
        let result = fetch_uri("file:///tmp/test").await;
        assert!(matches!(result, Err(StorageError::UnsupportedScheme(_))));
    }
}
