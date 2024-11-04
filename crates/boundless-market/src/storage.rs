// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

//! Provider implementations for uploading image and input files such that they are publicly
//! accessible to provers.

use std::{env::VarError, result::Result::Ok, sync::Arc, time::Duration};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use aws_sdk_s3::{
    config::{Builder, Credentials, Region},
    presigning::{PresigningConfig, PresigningConfigError},
    primitives::ByteStream,
    types::CreateBucketConfiguration,
    Error as S3Error,
};
use reqwest::{
    multipart::{Form, Part},
    Url,
};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

#[async_trait]
pub trait StorageProvider {
    type Error;

    // TODO(victor): Should this be upload_elf?
    /// Upload a risc0-zkvm ELF binary.
    ///
    /// Returns the URL which can be used to publicly access the uploaded ELF. This URL can be
    /// included in a proving request sent to Boundless.
    async fn upload_image(&self, elf: &[u8]) -> Result<String, Self::Error>;

    /// Upload the input for use in a proving request.
    ///
    /// Returns the URL which can be used to publicly access the uploaded input. This URL can be
    /// included in a proving request sent to Boundless.
    async fn upload_input(&self, input: &[u8]) -> Result<String, Self::Error>;
}

#[derive(Clone)]
#[non_exhaustive]
pub enum BuiltinStorageProvider {
    S3(S3StorageProvider),
    Pinata(PinataStorageProvider),
    File(TempFileStorageProvider),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BuiltinStorageProviderError {
    #[error("S3 storage provider error")]
    S3(#[from] S3StorageProviderError),
    #[error("Pinata storage provider error")]
    Pinata(#[from] PinataStorageProviderError),
    #[error("temp file storage provider error")]
    File(#[from] TempFileStorageProviderError),
    #[error("no storage provider is configured")]
    NoProvider,
}

#[async_trait]
impl StorageProvider for BuiltinStorageProvider {
    type Error = BuiltinStorageProviderError;

    async fn upload_image(&self, elf: &[u8]) -> Result<String, Self::Error> {
        Ok(match self {
            Self::S3(provider) => provider.upload_image(elf).await?,
            Self::Pinata(provider) => provider.upload_image(elf).await?,
            Self::File(provider) => provider.upload_image(elf).await?,
        })
    }

    async fn upload_input(&self, input: &[u8]) -> Result<String, Self::Error> {
        Ok(match self {
            Self::S3(provider) => provider.upload_input(input).await?,
            Self::Pinata(provider) => provider.upload_input(input).await?,
            Self::File(provider) => provider.upload_input(input).await?,
        })
    }
}

/// Creates a storage provider based on the environment variables.
///
/// If the environment variable `RISC0_DEV_MODE` is set, a temporary file storage provider is used.
/// Otherwise, the following environment variables are checked in order:
/// - `PINATA_JWT`, `PINATA_API_URL`, `IPFS_GATEWAY_URL`: Pinata storage provider;
/// - `S3_ACCESS`, `S3_SECRET`, `S3_BUCKET`, `S3_URL`, `AWS_REGION`: S3 storage provider.
pub async fn storage_provider_from_env(
) -> Result<BuiltinStorageProvider, BuiltinStorageProviderError> {
    if risc0_zkvm::is_dev_mode() {
        return Ok(BuiltinStorageProvider::File(TempFileStorageProvider::new().await?));
    }

    if let Ok(provider) = PinataStorageProvider::from_env().await {
        return Ok(BuiltinStorageProvider::Pinata(provider));
    }

    if let Ok(provider) = S3StorageProvider::from_env().await {
        return Ok(BuiltinStorageProvider::S3(provider));
    }

    Err(BuiltinStorageProviderError::NoProvider)
}

/// Storage provider that uploads inputs and inputs to IPFS via Pinata.
#[derive(Clone)]
pub struct PinataStorageProvider {
    client: reqwest::Client,
    api_key: String,
    api_url: Url,
    gateway_url: Url,
}

#[derive(thiserror::Error, Debug)]
pub enum PinataStorageProviderError {
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),

    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("environment variable error: {0}")]
    EnvVar(#[from] VarError),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

const DEFAULT_PINATA_API_URL: &str = "https://api.pinata.cloud";
const DEFAULT_GATEWAY_URL: &str = "https://gateway.pinata.cloud";

impl PinataStorageProvider {
    pub async fn from_env() -> Result<Self, PinataStorageProviderError> {
        let api_key = std::env::var("PINATA_JWT")
            .context("failed to fetch environment variable 'PINATA_JWT'")?;
        if api_key.is_empty() {
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

        Ok(Self { api_key, api_url, gateway_url, client })
    }

    async fn upload(
        &self,
        data: impl AsRef<[u8]>,
        filename: impl Into<String>,
    ) -> Result<Url, PinataStorageProviderError> {
        // https://docs.pinata.cloud/api-reference/endpoint/pin-file-to-ipfs
        let url = self.api_url.join("/pinning/pinFileToIPFS")?;
        let form = Form::new().part(
            "file",
            Part::bytes(data.as_ref().to_vec())
                .mime_str("application/octet-stream")?
                .file_name(filename.into()),
        );

        let request = self
            .client
            .post(url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .build()?;

        tracing::debug!("Sending upload HTTP request: {:#?}", request);

        let response = self.client.execute(request).await?;

        tracing::debug!("Received HTTP response: {:#?}", response);
        let response = response.error_for_status()?;

        let json_value: serde_json::Value = response.json().await?;
        let ipfs_hash = json_value
            .as_object()
            .ok_or(anyhow!("response from Pinata is not a JSON object"))?
            .get("IpfsHash")
            .ok_or(anyhow!("response from Pinata does not contain IpfsHash"))?
            .as_str()
            .ok_or(anyhow!("response from Pinata contains an invalid IpfsHash"))?;

        let data_url = self.gateway_url.join(&format!("ipfs/{ipfs_hash}"))?;
        Ok(data_url)
    }
}

#[async_trait]
impl StorageProvider for PinataStorageProvider {
    type Error = PinataStorageProviderError;

    async fn upload_image(&self, elf: &[u8]) -> Result<String, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(elf)?;
        let filename = format!("{}.elf", image_id);
        self.upload(elf, filename).await.map(|url| url.clone().into())
    }

    async fn upload_input(&self, input: &[u8]) -> Result<String, Self::Error> {
        let digest = Sha256::digest(input);
        let filename = format!("{}.input", hex::encode(digest.as_slice()));
        self.upload(input, filename).await.map(|url| url.clone().into())
    }
}

#[derive(Clone)]
pub struct S3StorageProvider {
    client: aws_sdk_s3::Client,
    bucket: String,
}

#[derive(thiserror::Error, Debug)]
pub enum S3StorageProviderError {
    #[error("AWS S3 error: {0}")]
    S3Error(#[from] S3Error),

    #[error("S3 presigning error: {0}")]
    PresigningConfigError(#[from] PresigningConfigError),

    #[error("environment variable error: {0}")]
    EnvVar(#[from] VarError),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl S3StorageProvider {
    pub async fn from_env() -> Result<Self, S3StorageProviderError> {
        let access_key = std::env::var("S3_ACCESS_KEY")?;
        let secret_key = std::env::var("S3_SECRET_KEY")?;
        let bucket = std::env::var("S3_BUCKET")?;
        let url = std::env::var("S3_URL")?;
        let region = std::env::var("AWS_REGION")?;
        let cred = Credentials::new(access_key, secret_key, None, None, "loaded-from-custom-env");

        let s3_config = Builder::new()
            .endpoint_url(url)
            .credentials_provider(cred)
            .behavior_version_latest()
            .region(Region::new(region))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);

        // Attempt to provision the bucket if it does not exist
        let cfg = CreateBucketConfiguration::builder().build();
        let res = client
            .create_bucket()
            .create_bucket_configuration(cfg)
            .bucket(&bucket)
            .send()
            .await
            .map_err(|e| S3Error::from(e.into_service_error()));

        if let Err(err) = res {
            match err {
                S3Error::BucketAlreadyOwnedByYou(_) => {}
                _ => return Err(err.into()),
            }
        }

        Ok(Self { client, bucket })
    }

    async fn upload(
        &self,
        data: impl AsRef<[u8]>,
        key: &str,
    ) -> Result<String, S3StorageProviderError> {
        let byte_stream = ByteStream::from(data.as_ref().to_vec());

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(byte_stream)
            .send()
            .await
            .map_err(|e| S3Error::from(e.into_service_error()))?;

        // TODO(victor): Presigned requests are somewhat large. It would be nice to instead set up
        // IAM permissions on the upload to make it public, and provide a simple URL.
        let presigned_request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(Duration::from_secs(3600))?)
            .await
            .map_err(|e| S3Error::from(e.into_service_error()))?;

        Ok(presigned_request.uri().to_string())
    }
}

#[async_trait]
impl StorageProvider for S3StorageProvider {
    type Error = S3StorageProviderError;

    async fn upload_image(&self, elf: &[u8]) -> Result<String, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(elf)?;
        let key = format!("image/{}", image_id);
        self.upload(elf, &key).await
    }

    async fn upload_input(&self, input: &[u8]) -> Result<String, Self::Error> {
        let digest = Sha256::digest(input);
        let key = format!("input/{}", hex::encode(digest.as_slice()));
        self.upload(input, &key).await
    }
}

#[derive(Clone)]
pub struct TempFileStorageProvider {
    temp_dir: Arc<TempDir>,
}

#[derive(thiserror::Error, Debug)]
pub enum TempFileStorageProviderError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl TempFileStorageProvider {
    pub async fn new() -> Result<Self, TempFileStorageProviderError> {
        Ok(Self { temp_dir: Arc::new(tempfile::tempdir()?) })
    }

    async fn save_file(
        &self,
        data: impl AsRef<[u8]>,
        filename: &str,
    ) -> Result<Url, TempFileStorageProviderError> {
        let file_path = self.temp_dir.path().join(filename);
        tokio::fs::write(&file_path, data.as_ref()).await?;

        let file_url = Url::from_file_path(&file_path)
            .map_err(|()| anyhow!("failed to convert file path to URL: {:?}", file_path))?;
        Ok(file_url)
    }
}

#[async_trait]
impl StorageProvider for TempFileStorageProvider {
    type Error = TempFileStorageProviderError;

    async fn upload_image(&self, elf: &[u8]) -> Result<String, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(elf)?;
        let filename = format!("{}.elf", image_id);
        let file_url = self.save_file(elf, &filename).await?;
        Ok(file_url.to_string())
    }

    async fn upload_input(&self, input: &[u8]) -> Result<String, Self::Error> {
        let digest = Sha256::digest(input);
        let filename = format!("{}.input", hex::encode(digest.as_slice()));
        let file_url = self.save_file(input, &filename).await?;
        Ok(file_url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{StorageProvider, TempFileStorageProvider};

    #[tokio::test]
    async fn test_temp_file_storage_provider() {
        let provider = TempFileStorageProvider::new().await.unwrap();

        let image_data = guest_util::ECHO_ELF;
        let input_data = b"test input data";

        let image_url = provider.upload_image(image_data).await.unwrap();
        let input_url = provider.upload_input(input_data).await.unwrap();

        println!("Image URL: {}", image_url);
        println!("Input URL: {}", input_url);
    }
}
