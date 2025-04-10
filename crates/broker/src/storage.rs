// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::config::ConfigLock;
use alloy::primitives::bytes::Buf;
use async_trait::async_trait;
use aws_config::retry::RetryConfig;
use aws_sdk_s3::{
    config::{ProvideCredentials, SharedCredentialsProvider},
    Client as S3Client,
};
use futures::StreamExt;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use std::{
    env,
    error::Error as StdError,
    fmt::{Display, Formatter},
    path::PathBuf,
    sync::Arc,
};

const ENV_VAR_ROLE_ARN: &str = "AWS_ROLE_ARN";

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum StorageErr {
    #[error("unsupported URI scheme: {0}")]
    UnsupportedScheme(String),

    #[error("failed to parse URL")]
    UriParse(#[from] url::ParseError),

    #[error("invalid URL: {0}")]
    InvalidURL(&'static str),

    #[error("resource size exceeds maximum allowed size ({0} bytes)")]
    SizeLimitExceeded(usize),

    #[error("file error")]
    File(#[from] std::io::Error),

    #[error("HTTP error")]
    Http(#[source] Box<dyn StdError + Send + Sync + 'static>),

    #[error("AWS S3 error")]
    S3(#[source] Box<dyn StdError + Send + Sync + 'static>),
}

pub(crate) async fn create_uri_handler(
    uri_str: &str,
    config: &ConfigLock,
) -> Result<Arc<dyn Handler>, StorageErr> {
    let uri = url::Url::parse(uri_str)?;

    match uri.scheme() {
        "file" => {
            if !risc0_zkvm::is_dev_mode() {
                return Err(StorageErr::UnsupportedScheme("file".to_string()));
            }
            let max_size = {
                let config = &config.lock_all().expect("lock failed").market;
                config.max_file_size
            };
            let handler = FileHandler { path: uri.path().into(), max_size };

            Ok(Arc::new(handler))
        }
        "http" | "https" => {
            let (max_size, max_retries, cache_dir) = {
                let config = &config.lock_all().expect("lock failed").market;
                (config.max_file_size, config.max_fetch_retries, config.cache_dir.clone())
            };
            let handler = HttpHandler::new(uri, max_size, cache_dir, max_retries).await?;

            Ok(Arc::new(handler))
        }
        "s3" => {
            let (max_size, max_retries, endpoint_url) = {
                let config = &config.lock_all().expect("lock failed").market;
                (config.max_file_size, config.max_fetch_retries, config.s3_endpoint_url.clone())
            };
            let handler = S3Handler::new(uri, max_size, max_retries, endpoint_url).await?;

            Ok(Arc::new(handler))
        }
        scheme => Err(StorageErr::UnsupportedScheme(scheme.to_string())),
    }
}

#[async_trait]
pub(crate) trait Handler: Display + Send + Sync {
    async fn fetch(&self) -> Result<Vec<u8>, StorageErr>;
}

struct FileHandler {
    path: PathBuf,
    max_size: usize,
}

impl Display for FileHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        url::Url::from_file_path(&self.path).unwrap().fmt(f)
    }
}

#[async_trait]
impl Handler for FileHandler {
    async fn fetch(&self) -> Result<Vec<u8>, StorageErr> {
        let metadata = tokio::fs::metadata(&self.path).await?;
        let size = metadata.len() as usize;
        if size > self.max_size {
            return Err(StorageErr::SizeLimitExceeded(size));
        }

        Ok(tokio::fs::read(&self.path).await?)
    }
}

pub struct HttpHandler {
    url: url::Url,
    client: ClientWithMiddleware,
    max_size: usize,
}

impl HttpHandler {
    async fn new(
        url: url::Url,
        max_size: usize,
        cache_dir: Option<PathBuf>,
        max_retries: Option<u8>,
    ) -> Result<Self, StorageErr> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(StorageErr::InvalidURL("invalid HTTP scheme"));
        }
        if !url.has_host() {
            return Err(StorageErr::InvalidURL("missing host"));
        }

        let mut builder = ClientBuilder::new(reqwest::Client::new());

        if let Some(cache_dir) = cache_dir {
            tokio::fs::create_dir_all(&cache_dir).await?;
            let manager = CACacheManager { path: cache_dir };
            let cache_middleware = Cache(HttpCache {
                mode: CacheMode::ForceCache,
                manager,
                options: HttpCacheOptions::default(),
            });

            builder = builder.with(cache_middleware)
        }
        if let Some(max_retries) = max_retries {
            let retry_policy =
                ExponentialBackoff::builder().build_with_max_retries(max_retries as u32);
            let retry_middleware = RetryTransientMiddleware::new_with_policy(retry_policy);

            builder = builder.with(retry_middleware)
        }

        Ok(HttpHandler { url, client: builder.build(), max_size })
    }
}

impl Display for HttpHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.url.fmt(f)
    }
}

#[async_trait]
impl Handler for HttpHandler {
    async fn fetch(&self) -> Result<Vec<u8>, StorageErr> {
        let response = self
            .client
            .get(self.url.clone())
            .send()
            .await
            .map_err(|err| StorageErr::Http(err.into()))?;
        let response = response.error_for_status().map_err(|err| StorageErr::Http(err.into()))?;

        // If a maximum size is set and the content_length exceeds it, return early.
        let capacity_hint = response.content_length().unwrap_or_default() as usize;
        if capacity_hint > self.max_size {
            return Err(StorageErr::SizeLimitExceeded(capacity_hint));
        }

        let mut buffer = Vec::with_capacity(capacity_hint);
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| StorageErr::Http(err.into()))?;
            buffer.extend_from_slice(chunk.chunk());
            if buffer.len() > self.max_size {
                return Err(StorageErr::SizeLimitExceeded(buffer.len()));
            }
        }

        Ok(buffer)
    }
}

pub struct S3Handler {
    bucket: String,
    key: String,
    client: S3Client,
    max_size: usize,
}

impl S3Handler {
    async fn new(
        url: url::Url,
        max_size: usize,
        max_retries: Option<u8>,
        endpoint_url: Option<String>,
    ) -> Result<Self, StorageErr> {
        let retry_config = if let Some(max_retries) = max_retries {
            RetryConfig::standard().with_max_attempts(max_retries as u32 + 1)
        } else {
            RetryConfig::disabled()
        };

        let mut s3_conf = aws_config::from_env().retry_config(retry_config).load().await;
        if match s3_conf.credentials_provider() {
            None => true,
            Some(credentials) => credentials.provide_credentials().await.is_err(),
        } {
            return Err(StorageErr::UnsupportedScheme("s3".to_string()));
        }

        if let Ok(role_arn) = env::var(ENV_VAR_ROLE_ARN) {
            // Create the AssumeRoleProvider using the base_config for its STS client needs
            let role_provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
                .configure(&s3_conf) // Use the base config to configure the provider
                .build()
                .await;
            s3_conf = s3_conf
                .into_builder()
                .credentials_provider(SharedCredentialsProvider::new(role_provider))
                .build();
        }

        if let Some(endpoint_url) = endpoint_url {
            s3_conf = s3_conf.into_builder().endpoint_url(endpoint_url).build();
        }

        let bucket = url.host_str().ok_or(StorageErr::InvalidURL("missing bucket"))?;
        let key = url.path().trim_start_matches('/');
        if key.is_empty() {
            return Err(StorageErr::InvalidURL("empty key"));
        }

        Ok(S3Handler {
            bucket: bucket.to_string(),
            key: key.to_string(),
            client: S3Client::new(&s3_conf),
            max_size,
        })
    }
}

impl Display for S3Handler {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "s3://{}/{}", self.bucket, self.key)
    }
}

#[async_trait]
impl Handler for S3Handler {
    async fn fetch(&self) -> Result<Vec<u8>, StorageErr> {
        let resp = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(&self.key)
            .send()
            .await
            .map_err(|e| StorageErr::S3(e.into()))?;

        let capacity_hint = resp.content_length.unwrap_or_default() as usize;
        if capacity_hint > self.max_size {
            return Err(StorageErr::SizeLimitExceeded(capacity_hint));
        }

        let mut buffer = Vec::with_capacity(capacity_hint);
        let mut stream = resp.body;

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| StorageErr::S3(e.into()))?;
            buffer.extend_from_slice(chunk.chunk());
            if buffer.len() > self.max_size {
                return Err(StorageErr::SizeLimitExceeded(buffer.len()));
            }
        }

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use std::sync::atomic::{AtomicU8, Ordering};

    #[tokio::test]
    async fn http_fetch() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(&resp_data);
        });

        let url = url::Url::parse(&server.url("/image")).unwrap();
        let handler = HttpHandler::new(url, 1_000_000, None, None).await.unwrap();

        let data = handler.fetch().await.unwrap();
        assert_eq!(data, resp_data);
        get_mock.assert();
    }

    #[tokio::test]
    async fn http_fetch_retry() {
        const RETRIES: u8 = 2;
        static CALL_COUNT: AtomicU8 = AtomicU8::new(0);
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];

        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET)
                .path("/image")
                .matches(|_| CALL_COUNT.fetch_add(1, Ordering::SeqCst) < RETRIES);
            then.status(503) // Service Unavailable - retryable
                .body("Service temporarily down");
        });
        let success_mock = server.mock(|when, then| {
            when.method(GET)
                .path("/image")
                .matches(|_| CALL_COUNT.fetch_add(1, Ordering::SeqCst) >= RETRIES);
            then.status(200).body(&resp_data);
        });

        let url = url::Url::parse(&server.url("/image")).unwrap();
        let handler = HttpHandler::new(url, 1_000_000, None, Some(RETRIES)).await.unwrap();

        handler.fetch().await.unwrap();
        success_mock.assert();
    }

    #[tokio::test]
    async fn http_max_size() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(&resp_data);
        });

        let url = url::Url::parse(&server.url("/image")).unwrap();
        let handler = HttpHandler::new(url, 1, None, None).await.unwrap();

        let result = handler.fetch().await;
        get_mock.assert();
        assert!(matches!(result, Err(StorageErr::SizeLimitExceeded(_))));
    }
}
