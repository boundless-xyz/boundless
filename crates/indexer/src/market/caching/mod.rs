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

//! Caching layer for blockchain logs and transaction metadata.

use std::collections::HashMap;

use alloy::{primitives::B256, rpc::types::Log};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};

use crate::db::TxMetadata;

mod file;
mod s3;
mod serde;

pub use file::{FileCacheStorage, FileCacheStorageError};
pub use s3::{S3CacheStorage, S3CacheStorageError};

#[derive(thiserror::Error, Debug)]
pub enum CacheStorageError {
    #[error("File cache error: {0}")]
    FileError(#[from] FileCacheStorageError),

    #[error("S3 cache error: {0}")]
    S3Error(#[from] S3CacheStorageError),

    #[error("Invalid cache URI: {0}")]
    InvalidUri(String),

    #[error("Unsupported cache URI scheme: {0}")]
    UnsupportedScheme(String),
}

/// Trait for caching blockchain logs and transaction metadata.
#[async_trait]
pub trait CacheStorage: Send + Sync {
    /// Get cached logs for a given block range and event signatures.
    async fn get_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<Vec<Log>>, CacheStorageError>;

    /// Store logs in the cache.
    async fn put_logs(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        logs: &[Log],
    ) -> Result<(), CacheStorageError>;

    /// Get cached transaction metadata for a given block range and event signatures.
    async fn get_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
    ) -> Result<Option<HashMap<B256, TxMetadata>>, CacheStorageError>;

    /// Store transaction metadata in the cache.
    async fn put_tx_metadata(
        &self,
        chain_id: u64,
        start_block: u64,
        end_block: u64,
        event_signatures: &[B256],
        metadata: &HashMap<B256, TxMetadata>,
    ) -> Result<(), CacheStorageError>;
}

/// Generate a cache key from chain ID, block range, and event signatures.
pub fn generate_cache_key(
    chain_id: u64,
    start_block: u64,
    end_block: u64,
    event_signatures: &[B256],
) -> String {
    // Sort and hash the event signatures to ensure consistent keys
    let mut sorted_sigs = event_signatures.to_vec();
    sorted_sigs.sort();

    let mut hasher = Sha256::new();
    for sig in &sorted_sigs {
        hasher.update(sig.as_slice());
    }
    let events_hash = hex::encode(hasher.finalize());

    format!("{}/{}_{}_{}", chain_id, start_block, end_block, events_hash)
}

/// Create a cache storage instance from a URI.
///
/// Supported URI schemes:
/// - `file://path/to/cache` - Local file system cache
/// - `s3://bucket-name` - S3 bucket cache
pub async fn cache_storage_from_uri(uri: &str) -> Result<Box<dyn CacheStorage>, CacheStorageError> {
    let url = uri.parse::<url::Url>().map_err(|e| {
        CacheStorageError::InvalidUri(format!("Failed to parse URI '{}': {}", uri, e))
    })?;

    match url.scheme() {
        "file" => {
            let path = url.path();
            let storage = FileCacheStorage::new(path)?;
            Ok(Box::new(storage))
        }
        "s3" => {
            let bucket = url
                .host_str()
                .ok_or_else(|| {
                    CacheStorageError::InvalidUri(format!("S3 URI missing bucket name: {}", uri))
                })?
                .to_string();
            let storage = S3CacheStorage::from_env(bucket).await?;
            Ok(Box::new(storage))
        }
        scheme => Err(CacheStorageError::UnsupportedScheme(scheme.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_cache_key() {
        let chain_id = 1;
        let start_block = 100;
        let end_block = 200;
        let event_sigs = vec![B256::from([1u8; 32]), B256::from([2u8; 32])];

        let key1 = generate_cache_key(chain_id, start_block, end_block, &event_sigs);

        // Same inputs should produce same key
        let key2 = generate_cache_key(chain_id, start_block, end_block, &event_sigs);
        assert_eq!(key1, key2);

        // Different order of event signatures should produce same key (they're sorted)
        let event_sigs_reversed = vec![B256::from([2u8; 32]), B256::from([1u8; 32])];
        let key3 = generate_cache_key(chain_id, start_block, end_block, &event_sigs_reversed);
        assert_eq!(key1, key3);

        // Different chain ID should produce different key
        let key4 = generate_cache_key(42, start_block, end_block, &event_sigs);
        assert_ne!(key1, key4);
    }
}
