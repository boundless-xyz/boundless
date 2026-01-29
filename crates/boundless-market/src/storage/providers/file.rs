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

//! Provider implementation for storing programs and inputs locally as files.
//!
//! This provider is primarily intended for local development and testing.

use crate::storage::{
    StorageDownloader, StorageError, StorageUploader, StorageUploaderConfig, StorageUploaderType,
};
use async_trait::async_trait;
use std::{path::Path, sync::Arc};
use tempfile::TempDir;
use url::Url;

/// Storage provider that stores files locally.
///
/// This provider can be used for:
/// - **Uploading**: Stores files in a temporary directory and returns `file://` URLs.
/// - **Downloading**: Reads files from `file://` URLs.
///
/// # Security Note
///
/// The `file://` scheme should only be enabled in development mode, as it allows
/// reading arbitrary files from the filesystem.
#[derive(Clone, Debug)]
pub struct FileStorageUploader {
    /// Directory where uploaded files are stored.
    path: Arc<TempDir>,
}

impl FileStorageUploader {
    /// Creates a new file storage uploader with a temporary directory.
    pub fn new() -> Result<Self, StorageError> {
        Ok(Self { path: Arc::new(tempfile::tempdir()?) })
    }

    /// Creates a new file storage uploader with a temporary directory under the given path.
    pub fn with_path(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Ok(Self { path: Arc::new(tempfile::tempdir_in(path)?) })
    }

    /// Creates a new file storage uploader from upload configuration.
    pub fn from_config(config: &StorageUploaderConfig) -> Result<Self, StorageError> {
        assert_eq!(config.storage_uploader, StorageUploaderType::File);

        let uploader = match &config.file_path {
            Some(path) => Self::with_path(path)?,
            None => Self::new()?,
        };
        Ok(uploader)
    }

    async fn save_file(&self, data: &[u8], filename: &str) -> Result<Url, StorageError> {
        let file_path = self.path.path().join(filename);
        tokio::fs::write(&file_path, data).await?;

        Url::from_file_path(&file_path).map_err(|()| {
            StorageError::Other(anyhow::anyhow!(
                "failed to convert file path to URL: {:?}",
                file_path
            ))
        })
    }
}

#[async_trait]
impl StorageUploader for FileStorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.save_file(data, key).await
    }
}

/// Downloader for `file://` URLs.
#[derive(Clone, Debug, Default)]
pub struct FileStorageDownloader {}

impl FileStorageDownloader {
    /// Creates a new file downloader.
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl StorageDownloader for FileStorageDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        if url.scheme() != "file" {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        tracing::debug!(%url, "downloading from file");

        let path = Path::new(url.path());

        // Check file size before reading
        let metadata = tokio::fs::metadata(path).await?;
        let size = metadata.len() as usize;
        if size > limit {
            return Err(StorageError::SizeLimitExceeded { size, limit });
        }

        let data = tokio::fs::read(path).await?;
        tracing::trace!(size = data.len(), %url, "downloaded from file");

        Ok(data)
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        // No size limit configured; download full content
        self.download_url_with_limit(url, usize::MAX).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn roundtrip() {
        let uploader = FileStorageUploader::new().unwrap();

        let test_data = b"test input data";
        let url = uploader.upload_input(test_data).await.unwrap();

        assert_eq!(url.scheme(), "file", "expected file:// URL");

        let downloader = FileStorageDownloader::new();
        let downloaded = downloader.download_url(url).await.unwrap();

        assert_eq!(downloaded, test_data);
    }

    #[tokio::test]
    async fn rejects_oversized_file() {
        let uploader = FileStorageUploader::new().unwrap();

        let test_data = b"this is more than 10 bytes";
        let url = uploader.upload_input(test_data).await.unwrap();

        let downloader = FileStorageDownloader::new();
        let downloaded = downloader.download_url_with_limit(url, 10).await;

        assert!(matches!(downloaded, Err(StorageError::SizeLimitExceeded { .. })));
    }
}
