// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result, bail};
use aws_sdk_s3::{
    Client,
    config::{Builder, Credentials, Region},
    error::ProvideErrorMetadata,
    operation::{create_bucket::CreateBucketError, head_object::HeadObjectError},
    primitives::ByteStream,
    types::CreateBucketConfiguration,
};
use std::path::Path;

/// Object store elf dir
pub const ELF_BUCKET_DIR: &str = "elfs";

/// Object store input dir
pub const INPUT_BUCKET_DIR: &str = "inputs";

/// Guest executor logs dir
pub const EXEC_LOGS_BUCKET_DIR: &str = "exec_logs";

/// Object store receipts dir
pub const RECEIPT_BUCKET_DIR: &str = "receipts";

/// Object store preflight journals dir
pub const PREFLIGHT_JOURNALS_BUCKET_DIR: &str = "preflight_journals";

/// Object store stark receipt dir
pub const STARK_BUCKET_DIR: &str = "stark";

/// Object store receipts groth16 dir
pub const GROTH16_BUCKET_DIR: &str = "groth16";

/// Object store receipts blake3_groth16 dir
pub const BLAKE3_GROTH16_BUCKET_DIR: &str = "blake3_groth16";

/// Object store work receipts dir
pub const WORK_RECEIPTS_BUCKET_DIR: &str = "work_receipts";

/// S3 client object
pub struct S3Client {
    bucket: String,
    client: Client,
}

impl S3Client {
    /// Initialize a s3 client from a minio config
    pub async fn from_minio(
        url: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
    ) -> Result<Self> {
        let cred = Credentials::new(access_key, secret_key, None, None, "loaded-from-custom-env");

        let s3_config = Builder::new()
            .endpoint_url(url)
            .credentials_provider(cred)
            .behavior_version_latest()
            .region(Region::new(region.to_string()))
            .force_path_style(true)
            .build();

        let client = aws_sdk_s3::Client::from_conf(s3_config);

        // Check if bucket exists first - only create if we're certain it doesn't exist
        let bucket_exists = match client.head_bucket().bucket(bucket).send().await {
            Ok(_) => true,
            Err(err) => {
                // Check if it's a NotFound error (bucket doesn't exist)
                // Check both the error code and HTTP status code
                let is_not_found = if let Some(service_err) = err.as_service_error() {
                    service_err.code() == Some("NotFound") || service_err.code() == Some("404")
                } else {
                    false
                };

                // Also check the raw response status code if available
                let status_404 = err
                    .raw_response()
                    .and_then(|r| Some(r.status().as_u16() == 404))
                    .unwrap_or(false);

                // Only if we're certain it's a 404/NotFound, the bucket doesn't exist
                // Otherwise, assume it exists (or we can't determine) and don't try to create
                !(is_not_found || status_404)
            }
        };

        // Only attempt to create the bucket if we're certain it doesn't exist
        if !bucket_exists {
            let cfg =
                CreateBucketConfiguration::builder().location_constraint(region.into()).build();
            let res =
                client.create_bucket().create_bucket_configuration(cfg).bucket(bucket).send().await;

            if let Err(err) = res {
                let Some(service_err) = err.as_service_error() else {
                    bail!(format!("Minio SDK error: {err:?}"));
                };
                match service_err {
                    CreateBucketError::BucketAlreadyOwnedByYou(_) => {
                        // Bucket was created by another process between check and create
                    }
                    _ => {
                        // Check if it's an OperationAborted error (transient conflict)
                        if service_err.code() == Some("OperationAborted") {
                            // Another process is creating the bucket - this is fine, assume success
                        } else {
                            bail!(format!("Failed to create bucket: {err:?}"));
                        }
                    }
                }
            }
        }

        Ok(Self { bucket: bucket.to_string(), client })
    }

    /// Reads a s3 object encoded with bincode
    pub async fn read_from_s3<T>(&self, key: &str) -> Result<T>
    where
        T: serde::de::DeserializeOwned,
    {
        let result = self.client.get_object().bucket(&self.bucket).key(key).send().await?;

        let encoded = result.body.collect().await?.to_vec();
        bincode::deserialize(&encoded).context("Failed to deserialize s3 object")
    }

    /// Reads a s3 object to byte buffer
    pub async fn read_buf_from_s3(&self, key: &str) -> Result<Vec<u8>> {
        let result = self.client.get_object().bucket(&self.bucket).key(key).send().await?;

        Ok(result.body.collect().await?.to_vec())
    }

    /// Write a bincode serializable object to S3
    pub async fn write_to_s3<T>(&self, key: &str, obj: T) -> Result<()>
    where
        T: serde::Serialize,
    {
        let bytes = bincode::serialize(&obj)?;
        let data_stream = ByteStream::from(bytes);
        self.client.put_object().bucket(&self.bucket).key(key).body(data_stream).send().await?;
        Ok(())
    }

    /// Write a buffer to S3
    pub async fn write_buf_to_s3(&self, key: &str, bytes: Vec<u8>) -> Result<()> {
        let data_stream = ByteStream::from(bytes);
        self.client.put_object().bucket(&self.bucket).key(key).body(data_stream).send().await?;
        Ok(())
    }

    pub async fn write_file_to_s3(&self, key: &str, in_path: &Path) -> Result<()> {
        let data_stream = ByteStream::read_from().path(in_path).build().await?;
        self.client.put_object().bucket(&self.bucket).key(key).body(data_stream).send().await?;
        Ok(())
    }

    pub async fn object_exists(&self, key: &str) -> Result<bool> {
        match self.client.head_bucket().bucket(&self.bucket).send().await {
            Ok(_) => match self.client.head_object().bucket(&self.bucket).key(key).send().await {
                Ok(_) => Ok(true),
                Err(err) => match err.into_service_error() {
                    HeadObjectError::NotFound(_) => Ok(false),
                    err => Err(err.into()),
                },
            },
            Err(err) => Err(err.into()),
        }
    }

    /// List objects in the bucket with optional prefix
    pub async fn list_objects(&self, prefix: Option<&str>) -> Result<Vec<String>> {
        let mut objects = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut request = self.client.list_objects_v2().bucket(&self.bucket);

            if let Some(prefix) = prefix {
                request = request.prefix(prefix);
            }

            if let Some(token) = continuation_token {
                request = request.continuation_token(token);
            }

            let response = request.send().await?;

            let contents = response.contents();
            for object in contents {
                if let Some(key) = object.key() {
                    objects.push(key.to_string());
                }
            }

            continuation_token = response.next_continuation_token().map(|s| s.to_string());
            if continuation_token.is_none() {
                break;
            }
        }

        Ok(objects)
    }
}
