// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{config::ConfigLock, errors::CodedError};
use alloy::primitives::bytes::Buf;
use anyhow::{Context, Result};
use async_trait::async_trait;
use aws_config::retry::RetryConfig;
use aws_sdk_s3::{
    config::{ProvideCredentials, SharedCredentialsProvider},
    error::ProvideErrorMetadata,
    Client as S3Client,
};
use futures::StreamExt;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use risc0_zkvm::Digest;
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
    #[error("{code} unsupported URI scheme: {0}", code = self.code())]
    UnsupportedScheme(String),

    #[error("{code} failed to parse URL", code = self.code())]
    UriParse(#[from] url::ParseError),

    #[error("{code} invalid URL: {0}", code = self.code())]
    InvalidURL(&'static str),

    #[error("{code} resource size exceeds maximum allowed size ({0} bytes)", code = self.code())]
    SizeLimitExceeded(usize),

    #[error("{code} file error", code = self.code())]
    File(#[from] std::io::Error),

    #[error("{code} HTTP error", code = self.code())]
    Http(#[source] Box<dyn StdError + Send + Sync + 'static>),

    #[error("{code} AWS S3 error", code = self.code())]
    S3(#[source] Box<dyn StdError + Send + Sync + 'static>),
}

impl CodedError for StorageErr {
    fn code(&self) -> &str {
        match self {
            StorageErr::Http(_) => "[B-STR-002]",
            _ => "[B-STR-500]",
        }
    }
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
            let (max_size, max_retries) = {
                let config = &config.lock_all().expect("lock failed").market;
                (config.max_file_size, config.max_fetch_retries)
            };
            let handler = S3Handler::new(uri, max_size, max_retries).await?;

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
        let capacity = response.content_length().unwrap_or_default() as usize;
        if capacity > self.max_size {
            return Err(StorageErr::SizeLimitExceeded(capacity));
        }

        let mut buffer = Vec::with_capacity(capacity);
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

/// Handles fetching data specified by `s3://` URIs using the AWS SDK.
///
/// This handler authenticates using the default AWS credential chain (environment variables,
/// `~/.aws/credentials`, `~/.aws/config`, etc.), loaded via `aws_config::from_env()`.
///
/// If the `AWS_ROLE_ARN` environment variable is set and non-empty, it will attempt to assume that
/// IAM role using the initially resolved credentials before interacting with S3. This is crucial
/// for accessing resources requiring specific permissions, such as objects encrypted with SSE-KMS
/// where the assumed role needs `kms:Decrypt` permission granted by the key owner.
///
/// It enforces a maximum download size and utilizes AWS SDK's retry mechanisms, configured based
/// on the `max_retries` parameter provided during construction.
///
/// **Note:** Successful initialization requires that valid AWS credentials can be resolved from
/// the environment, otherwise `S3Handler::new` will return an [StorageErr::UnsupportedScheme].
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
    ) -> Result<Self, StorageErr> {
        let retry_config = if let Some(max_retries) = max_retries {
            RetryConfig::standard().with_max_attempts(max_retries as u32 + 1)
        } else {
            RetryConfig::disabled()
        };

        let mut config = aws_config::from_env().retry_config(retry_config).load().await;

        if let Some(provider) = config.credentials_provider() {
            if let Err(e) = provider.provide_credentials().await {
                tracing::debug!(error=%e, "Could not load initial AWS credentials required for S3 support. S3 support disabled.");
                return Err(StorageErr::UnsupportedScheme("s3".to_string()));
            }
        } else {
            // This should not happen with aws_config::from_env()
            return Err(StorageErr::UnsupportedScheme("s3".to_string()));
        }

        if let Ok(role_arn) = env::var(ENV_VAR_ROLE_ARN) {
            // Create the AssumeRoleProvider using the base_config for its STS client needs
            let role_provider = aws_config::sts::AssumeRoleProvider::builder(role_arn)
                .configure(&config) // Use the base config to configure the provider
                .build()
                .await;
            config = config
                .into_builder()
                .credentials_provider(SharedCredentialsProvider::new(role_provider))
                .build();
        }

        let bucket = url.host_str().ok_or(StorageErr::InvalidURL("missing bucket"))?;
        let key = url.path().trim_start_matches('/');
        if key.is_empty() {
            return Err(StorageErr::InvalidURL("empty key"));
        }

        Ok(S3Handler {
            bucket: bucket.to_string(),
            key: key.to_string(),
            client: S3Client::new(&config),
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
        let resp_result = self.client.get_object().bucket(&self.bucket).key(&self.key).send().await;
        let resp = match resp_result {
            Ok(resp) => resp,
            Err(sdk_err) => {
                let code = sdk_err.code();
                tracing::debug!(error = %sdk_err, code = ?code, "S3 GetObject failed");
                // Return the generic S3 error, wrapping the SdkError
                return Err(StorageErr::S3(sdk_err.into()));
            }
        };

        let capacity = resp.content_length.unwrap_or_default() as usize;
        if capacity > self.max_size {
            return Err(StorageErr::SizeLimitExceeded(capacity));
        }

        let mut buffer = Vec::with_capacity(capacity);
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

pub async fn upload_image_uri(
    prover: &crate::provers::ProverObj,
    request: &crate::ProofRequest,
    config: &crate::config::ConfigLock,
) -> Result<String> {
    let required_image_id = Digest::from(request.requirements.imageId.0);
    let image_id_str = required_image_id.to_string();
    if prover.has_image(&image_id_str).await? {
        tracing::debug!("Skipping program upload for cached image ID: {image_id_str}");
        return Ok(image_id_str);
    }

    tracing::debug!("Fetching program with image ID {image_id_str} from URI {}", request.imageUrl);
    let uri = create_uri_handler(&request.imageUrl, config).await.context("URL handling failed")?;

    let image_data = uri
        .fetch()
        .await
        .with_context(|| format!("Failed to fetch image URI: {}", request.imageUrl))?;
    let image_id =
        risc0_zkvm::compute_image_id(&image_data).context("Failed to compute image ID")?;

    anyhow::ensure!(
        image_id == required_image_id,
        "image ID does not match requirements; expect {}, got {}",
        required_image_id,
        image_id
    );

    tracing::debug!("Uploading program with image ID {image_id_str} to prover");
    prover
        .upload_image(&image_id_str, image_data)
        .await
        .context("Failed to upload image to prover")?;

    Ok(image_id_str)
}

pub async fn upload_input_uri(
    prover: &crate::provers::ProverObj,
    request: &crate::ProofRequest,
    config: &crate::config::ConfigLock,
) -> Result<String> {
    Ok(match request.input.inputType {
        boundless_market::contracts::InputType::Inline => prover
            .upload_input(
                boundless_market::input::GuestEnv::decode(&request.input.data)
                    .with_context(|| "Failed to decode input")?
                    .stdin,
            )
            .await
            .context("Failed to upload input data")?,

        boundless_market::contracts::RequestInputType::Url => {
            let input_uri_str =
                std::str::from_utf8(&request.input.data).context("input url is not utf8")?;
            tracing::debug!("Input URI string: {input_uri_str}");
            let input_uri =
                create_uri_handler(input_uri_str, config).await.context("URL handling failed")?;

            let input_data = boundless_market::input::GuestEnv::decode(
                &input_uri
                    .fetch()
                    .await
                    .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?,
            )
            .with_context(|| format!("Failed to decode input from URI: {input_uri_str}"))?
            .stdin;

            prover.upload_input(input_data).await.context("Failed to upload input")?
        }
        //???
        _ => anyhow::bail!("Invalid input type: {:?}", request.input.inputType),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_s3::{config::Credentials, primitives::SdkBody};
    use aws_smithy_http_client::test_util::capture_request;
    use httpmock::prelude::*;
    use serial_test::serial;
    use std::sync::atomic::{AtomicU8, Ordering};
    use tracing_test::traced_test;

    #[tokio::test]
    #[traced_test]
    async fn http_fetch_success() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(&resp_data);
        });

        let url = url::Url::parse(&server.url("/image")).unwrap();
        let handler = HttpHandler::new(url, 1024, None, None).await.unwrap();

        let data = handler.fetch().await.unwrap();
        assert_eq!(data, resp_data);
        get_mock.assert();
    }

    #[tokio::test]
    #[traced_test]
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
        let handler = HttpHandler::new(url, 1024, None, Some(RETRIES)).await.unwrap();

        handler.fetch().await.unwrap();
        success_mock.assert();
    }

    #[tokio::test]
    #[traced_test]
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

    // NOTE: These are dummy values, they don't need to be real AWS keys but their presence allows
    // the default provider chain to "succeed" initially.
    const DUMMY_AWS_CREDENTIALS: [(&str, Option<&str>); 6] = [
        ("AWS_ACCESS_KEY_ID", Some("TESTKEY")),
        ("AWS_SECRET_ACCESS_KEY", Some("TESTSECRET")),
        ("AWS_REGION", Some("us-east-1")),
        ("AWS_ROLE_ARN", Some("arn:aws:iam::123456789012:role/TestRole")),
        ("AWS_SESSION_TOKEN", None),
        ("AWS_PROFILE", None),
    ];

    #[tokio::test]
    #[traced_test]
    #[serial] // Run serially because it modifies environment variables
    async fn s3_new_success_with_role_arn_env() {
        let url = url::Url::parse("s3://test-bucket/path/to/object").unwrap();
        let result = temp_env::async_with_vars(
            DUMMY_AWS_CREDENTIALS,
            // NOTE: This test doesn't mock STS, so it only checks if S3Handler::new *attempts* to
            // use the role provider without erroring out immediately.
            S3Handler::new(url, 1024, None),
        )
        .await;

        let handler = result.unwrap();
        assert_eq!(handler.bucket, "test-bucket");
        assert_eq!(handler.key, "path/to/object");
        assert_eq!(handler.to_string(), "s3://test-bucket/path/to/object");
    }

    async fn mock_s3_handler(data: Vec<u8>, max_size: usize) -> S3Handler {
        let (client, _) = capture_request(Some(
            http::Response::builder().status(200).body(SdkBody::from(data)).unwrap(),
        ));
        let conf = aws_config::from_env()
            .credentials_provider(Credentials::new("example", "example", None, None, "example"))
            .region("us-east-1")
            .http_client(client)
            .load()
            .await;

        S3Handler {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            client: S3Client::new(&conf),
            max_size,
        }
    }

    #[tokio::test]
    #[traced_test]
    async fn s3_fetch_success() {
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let handler = mock_s3_handler(resp_data.clone(), 1024).await;

        let data = handler.fetch().await.unwrap();
        assert_eq!(data, resp_data);
    }

    #[tokio::test]
    #[traced_test]
    async fn s3_fetch_max_size() {
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let handler = mock_s3_handler(resp_data.clone(), 1).await;

        let result = handler.fetch().await;
        assert!(matches!(result, Err(StorageErr::SizeLimitExceeded(_))));
    }
}
