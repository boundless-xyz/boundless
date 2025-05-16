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

//! Provider implementation for uploading programs and inputs to AWS S3.

use std::{env::VarError, fmt::Debug, result::Result::Ok, time::Duration};

use async_trait::async_trait;
use aws_sdk_s3::{
    config::{Builder, Credentials, Region},
    presigning::{PresigningConfig, PresigningConfigError},
    primitives::ByteStream,
    types::CreateBucketConfiguration,
    Error as S3Error,
};
use reqwest::Url;
use sha2::{Digest as _, Sha256};
use tokio::sync::OnceCell;
use url::ParseError;

use super::{StorageProvider, StorageProviderConfig};

#[derive(Clone, Debug)]
/// Storage provider that uploads programs and inputs to S3.
pub struct S3StorageProvider {
    s3_bucket: String,
    client: aws_sdk_s3::Client,
    presigned: bool, // use s3:// urls or presigned https://
    // Used to coordinate the lazy initialization of the bucket.
    bucket_init: OnceCell<()>,
}

#[derive(thiserror::Error, Debug)]
/// Error type for the S3 storage provider.
pub enum S3StorageProviderError {
    /// Error type for S3 errors.
    ///
    /// Inside a [Box] because [S3Error] is rather large.
    #[error("AWS S3 error: {0}")]
    S3Error(#[from] Box<S3Error>),

    /// Error type for S3 presigning errors.
    #[error("S3 presigning error: {0}")]
    PresigningConfigError(#[from] PresigningConfigError),

    /// Error type for environment variable errors.
    #[error("environment variable error: {0}")]
    EnvVar(#[from] VarError),

    /// Error type for missing configuration parameters.
    #[error("missing config parameter: {0}")]
    Config(String),

    /// Error type for when S3 returns a string that fails to parse as a URL.
    #[error("failed to parse URL returned by S3: {0}")]
    UrlParseError(#[from] ParseError),

    /// Error type for other errors.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl S3StorageProvider {
    /// Creates a new S3 storage provider from the environment variables.
    pub fn from_env() -> Result<Self, S3StorageProviderError> {
        let access_key = std::env::var("S3_ACCESS_KEY")?;
        let secret_key = std::env::var("S3_SECRET_KEY")?;
        let bucket = std::env::var("S3_BUCKET")?;
        let url = std::env::var("S3_URL")?;
        let region = std::env::var("AWS_REGION")?;
        let presigned = std::env::var_os("S3_NO_PRESIGNED").is_none();

        Ok(Self::from_parts(access_key, secret_key, bucket, url, region, presigned))
    }

    /// Creates a new S3 storage provider from the given parts.
    pub fn from_parts(
        access_key: String,
        secret_key: String,
        bucket: String,
        url: String,
        region: String,
        presigned: bool,
    ) -> Self {
        let cred = Credentials::new(
            access_key.clone(),
            secret_key.clone(),
            None,
            None,
            "loaded-from-custom-env",
        );

        let s3_config = Builder::new()
            .endpoint_url(url.clone())
            .credentials_provider(cred)
            .behavior_version_latest()
            .region(Region::new(region.clone()))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);

        Self { s3_bucket: bucket, client, presigned, bucket_init: OnceCell::new() }
    }

    /// Creates a new S3 storage provider from the given configuration.
    pub fn from_config(config: &StorageProviderConfig) -> Result<Self, S3StorageProviderError> {
        let access_key = config
            .s3_access_key
            .clone()
            .ok_or_else(|| S3StorageProviderError::Config("s3_access_key".to_string()))?;

        let secret_key = config
            .s3_secret_key
            .clone()
            .ok_or_else(|| S3StorageProviderError::Config("s3_secret_key".to_string()))?;

        let bucket = config
            .s3_bucket
            .clone()
            .ok_or_else(|| S3StorageProviderError::Config("s3_bucket".to_string()))?;
        let url = config
            .s3_url
            .clone()
            .ok_or_else(|| S3StorageProviderError::Config("s3_url".to_string()))?;

        let region = config
            .aws_region
            .clone()
            .ok_or_else(|| S3StorageProviderError::Config("s3_region".to_string()))?;

        let presigned = config
            .s3_use_presigned
            .ok_or_else(|| S3StorageProviderError::Config("s3_use_presigned".to_string()))?;

        Ok(Self::from_parts(access_key, secret_key, bucket, url, region, presigned))
    }

    async fn upload(
        &self,
        data: impl AsRef<[u8]>,
        key: &str,
    ) -> Result<Url, S3StorageProviderError> {
        self.ensure_bucket_init().await?;

        let byte_stream = ByteStream::from(data.as_ref().to_vec());

        self.client
            .put_object()
            .bucket(&self.s3_bucket)
            .key(key)
            .body(byte_stream)
            .send()
            .await
            .map_err(|e| Box::new(S3Error::from(e.into_service_error())))?;

        if !self.presigned {
            return Ok(Url::parse(&format!("s3://{}/{}", self.s3_bucket, key)).unwrap());
        }

        // TODO(victor): Presigned requests are somewhat large. It would be nice to instead set up
        // IAM permissions on the upload to make it public, and provide a simple URL.
        let presigned_request = self
            .client
            .get_object()
            .bucket(&self.s3_bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(Duration::from_secs(3600))?)
            .await
            .map_err(|e| Box::new(S3Error::from(e.into_service_error())))?;

        Ok(Url::parse(presigned_request.uri())?)
    }

    async fn ensure_bucket_init(&self) -> Result<(), S3StorageProviderError> {
        self.bucket_init
            .get_or_try_init(async || {
                // Attempt to provision the bucket if it does not exist
                let cfg = CreateBucketConfiguration::builder().build();
                let res = self
                    .client
                    .create_bucket()
                    .create_bucket_configuration(cfg)
                    .bucket(&self.s3_bucket)
                    .send()
                    .await
                    .map_err(|e| S3Error::from(e.into_service_error()));

                match res {
                    Ok(_) => Ok(()),
                    Err(err) => match err {
                        S3Error::BucketAlreadyOwnedByYou(_) => Ok(()),
                        _ => Err(Box::new(err).into()),
                    },
                }
            })
            .await
            // a simple incantation.
            .map(|&()| ())
    }
}

#[async_trait]
impl StorageProvider for S3StorageProvider {
    type Error = S3StorageProviderError;

    async fn upload_program(&self, program: &[u8]) -> Result<Url, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(program)?;
        let key = format!("program/{}", image_id);
        self.upload(program, &key).await
    }

    async fn upload_input(&self, input: &[u8]) -> Result<Url, Self::Error> {
        let digest = Sha256::digest(input);
        let key = format!("input/{}", hex::encode(digest.as_slice()));
        self.upload(input, &key).await
    }
}
