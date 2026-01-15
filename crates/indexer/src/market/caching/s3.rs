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

//! S3-based cache storage implementation.

use std::{collections::HashMap, env, env::VarError};

use alloy::{primitives::B256, rpc::types::Log};
use async_trait::async_trait;
use aws_config::retry::RetryConfig;
use aws_sdk_s3::{
    config::{ProvideCredentials, SharedCredentialsProvider},
    primitives::ByteStream,
    Error as S3Error,
};

use crate::db::TxMetadata;

use super::{generate_cache_key, serde::SerializableTxMetadata, CacheStorage, CacheStorageError};

#[derive(thiserror::Error, Debug)]
pub enum S3CacheStorageError {
    #[error("AWS S3 error: {0:?}")]
    S3Error(#[from] Box<S3Error>),

    #[error("Environment variable error: {0:?}")]
    EnvVar(#[from] VarError),

    #[error("Serialization error: {0:?}")]
    Serialization(#[from] serde_json::Error),

    #[error("Missing config parameter: {0:?}")]
    Config(String),

    #[error("IO error: {0:?}")]
    Io(#[from] std::io::Error),

    #[error("AWS ByteStream error: {0:?}")]
    ByteStreamError(String),
}

/// S3-based cache storage that stores logs and metadata as JSON in an S3 bucket.
#[derive(Clone, Debug)]
pub struct S3CacheStorage {
    bucket: String,
    client: aws_sdk_s3::Client,
}

const ENV_VAR_ROLE_ARN: &str = "AWS_ROLE_ARN";

impl S3CacheStorage {
    /// Create a new S3 cache storage from environment variables.
    /// Uses IAM role-based authentication (no explicit credentials needed).
    /// Automatically loads credentials from ECS task role or other AWS credential sources.
    pub async fn from_env(bucket: String) -> Result<Self, S3CacheStorageError> {
        let retry_config = RetryConfig::standard();

        // Load AWS config from environment, which will automatically pick up:
        // - ECS task role credentials (when running in ECS)
        // - EC2 instance profile credentials (when running on EC2)
        // - Environment variables (AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY)
        // - AWS credentials file (~/.aws/credentials)
        let mut config = aws_config::from_env().retry_config(retry_config).load().await;

        // Verify credentials are available
        if let Some(provider) = config.credentials_provider() {
            if let Err(e) = provider.provide_credentials().await {
                tracing::debug!(
                    error = %e,
                    "Could not load initial AWS credentials required for S3 support"
                );
                return Err(S3CacheStorageError::Config(format!(
                    "Failed to load AWS credentials: {}",
                    e
                )));
            }
        } else {
            // This should not happen with aws_config::from_env()
            return Err(S3CacheStorageError::Config(
                "No credentials provider available".to_string(),
            ));
        }

        // Support optional role assumption (for cross-account access)
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

        let client = aws_sdk_s3::Client::new(&config);

        Ok(Self { bucket, client })
    }

    /// Get the S3 key for logs cache.
    fn logs_key(&self, cache_key: &str) -> String {
        format!("indexer-cache/{}.logs.json", cache_key)
    }

    /// Get the S3 key for metadata cache.
    fn metadata_key(&self, cache_key: &str) -> String {
        format!("indexer-cache/{}.metadata.json", cache_key)
    }

    /// Get an object from S3.
    async fn get_object(&self, key: &str) -> Result<Option<Vec<u8>>, S3CacheStorageError> {
        let result = self.client.get_object().bucket(&self.bucket).key(key).send().await;

        match result {
            Ok(output) => {
                let data = output
                    .body
                    .collect()
                    .await
                    .map_err(|e| S3CacheStorageError::ByteStreamError(e.to_string()))?
                    .into_bytes()
                    .to_vec();
                Ok(Some(data))
            }
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_no_such_key() {
                    Ok(None)
                } else {
                    Err(Box::new(S3Error::from(service_err)).into())
                }
            }
        }
    }

    /// Put an object to S3.
    async fn put_object(&self, key: &str, data: Vec<u8>) -> Result<(), S3CacheStorageError> {
        let byte_stream = ByteStream::from(data);

        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(byte_stream)
            .send()
            .await
            .map_err(|e| Box::new(S3Error::from(e.into_service_error())))?;

        Ok(())
    }
}

#[async_trait]
impl CacheStorage for S3CacheStorage {
    async fn get_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<Vec<Log>>, CacheStorageError> {
        let cache_key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let s3_key = self.logs_key(&cache_key);

        tracing::debug!("Getting cached logs from S3 bucket: {} key: {}", self.bucket, s3_key);
        let data = match self.get_object(&s3_key).await? {
            Some(data) => data,
            None => return Ok(None),
        };

        let logs: Vec<Log> = serde_json::from_slice(&data)
            .map_err(|e| CacheStorageError::from(S3CacheStorageError::from(e)))?;
        Ok(Some(logs))
    }

    async fn put_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        logs: &[Log],
    ) -> Result<(), CacheStorageError> {
        let cache_key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let s3_key = self.logs_key(&cache_key);

        tracing::debug!("Putting cached logs to S3 bucket: {} key: {}", self.bucket, s3_key);
        let data = serde_json::to_vec(logs)
            .map_err(|e| CacheStorageError::from(S3CacheStorageError::from(e)))?;
        self.put_object(&s3_key, data).await.map_err(CacheStorageError::from)?;

        tracing::debug!("Cached {} logs to S3 bucket: {} key: {}", logs.len(), self.bucket, s3_key);
        Ok(())
    }

    async fn get_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<HashMap<B256, TxMetadata>>, CacheStorageError> {
        let cache_key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let s3_key = self.metadata_key(&cache_key);

        tracing::debug!(
            "Getting cached tx metadata from S3 bucket: {} key: {}",
            self.bucket,
            s3_key
        );
        let data = match self.get_object(&s3_key).await? {
            Some(data) => data,
            None => return Ok(None),
        };
        tracing::debug!(
            "Got {} cached tx metadata from S3 bucket: {} key: {}",
            data.len(),
            self.bucket,
            s3_key
        );

        let serializable_map: HashMap<String, SerializableTxMetadata> =
            serde_json::from_slice(&data)
                .map_err(|e| CacheStorageError::from(S3CacheStorageError::from(e)))?;

        let metadata: HashMap<B256, TxMetadata> = serializable_map
            .into_iter()
            .map(|(k, v)| {
                let bytes = hex::decode(&k).expect("Invalid hex in cache");
                (B256::from_slice(&bytes), v.into())
            })
            .collect();

        Ok(Some(metadata))
    }

    async fn put_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        metadata: &HashMap<B256, TxMetadata>,
    ) -> Result<(), CacheStorageError> {
        let cache_key = generate_cache_key(chain_id, start_block, end_block, event_signatures);
        let s3_key = self.metadata_key(&cache_key);

        tracing::debug!("Putting cached tx metadata to S3 bucket: {} key: {}", self.bucket, s3_key);
        let serializable_map: HashMap<String, SerializableTxMetadata> =
            metadata.iter().map(|(k, v)| (hex::encode(k), v.into())).collect();

        let data = serde_json::to_vec(&serializable_map)
            .map_err(|e| CacheStorageError::from(S3CacheStorageError::from(e)))?;
        self.put_object(&s3_key, data).await.map_err(CacheStorageError::from)?;

        tracing::debug!(
            "Cached {} tx metadata entries to S3 bucket: {} key: {}",
            metadata.len(),
            self.bucket,
            s3_key
        );
        Ok(())
    }
}
