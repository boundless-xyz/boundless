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

//! Storage module for uploading and downloading programs and inputs.

use async_trait::async_trait;
use url::Url;

mod config;
mod default;
mod error;
mod providers;
mod traits;

pub use config::{StorageDownloaderConfig, DEFAULT_IPFS_GATEWAY_URL};
pub use config::{StorageUploaderConfig, StorageUploaderType};
pub use default::StandardDownloader;
pub use error::StorageError;
pub use providers::{
    FileStorageDownloader, FileStorageUploader, HttpDownloader, PinataStorageUploader,
};
pub use traits::{StorageDownloader, StorageUploader};

#[cfg(feature = "test-utils")]
pub use providers::MockStorageUploader;
#[cfg(feature = "gcs")]
pub use providers::{GcsStorageDownloader, GcsStorageUploader};
#[cfg(feature = "s3")]
pub use providers::{S3StorageDownloader, S3StorageUploader};

/// A storage uploader enum that can upload to various backends.
///
/// This enum provides a unified interface over all upload providers,
/// allowing runtime selection of the storage backend.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum StandardUploader {
    /// S3 storage uploader.
    #[cfg(feature = "s3")]
    S3(S3StorageUploader),
    /// Google Cloud Storage provider.
    #[cfg(feature = "gcs")]
    Gcs(GcsStorageUploader),
    /// Pinata/IPFS storage uploader.
    Pinata(PinataStorageUploader),
    /// Local file storage uploader.
    File(FileStorageUploader),
    /// In-memory mock storage uploader for testing.
    #[cfg(feature = "test-utils")]
    Mock(MockStorageUploader),
}

#[async_trait]
impl StorageUploader for StandardUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        match self {
            #[cfg(feature = "s3")]
            Self::S3(p) => p.upload_bytes(data, key).await,
            #[cfg(feature = "gcs")]
            Self::Gcs(p) => p.upload_bytes(data, key).await,
            Self::Pinata(p) => p.upload_bytes(data, key).await,
            Self::File(p) => p.upload_bytes(data, key).await,
            #[cfg(feature = "test-utils")]
            Self::Mock(p) => p.upload_bytes(data, key).await,
        }
    }
}

impl StandardUploader {
    /// Creates a storage uploader from configuration.
    pub async fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        match config.storage_uploader {
            #[cfg(feature = "s3")]
            StorageUploaderType::S3 => Ok(Self::S3(S3StorageUploader::from_config(config).await?)),
            #[cfg(feature = "gcs")]
            StorageUploaderType::Gcs => {
                Ok(Self::Gcs(GcsStorageUploader::from_config(config).await?))
            }
            StorageUploaderType::Pinata => {
                Ok(Self::Pinata(PinataStorageUploader::from_config(config)?))
            }
            StorageUploaderType::File => Ok(Self::File(FileStorageUploader::from_config(config)?)),
            #[cfg(feature = "test-utils")]
            StorageUploaderType::Mock => Ok(Self::Mock(MockStorageUploader::new())),
            StorageUploaderType::None => Err(StorageError::NoUploader),
        }
    }
}
