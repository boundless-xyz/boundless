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

use crate::storage::config::StorageDownloaderConfig;
use crate::storage::{
    FileStorageDownloader, GcsStorageDownloader, HttpDownloader, S3StorageDownloader,
    StorageDownloader, StorageError,
};
use alloy::signers::k256::ecdsa::signature::digest::Digest;
use async_trait::async_trait;
use sha2::Sha256;
use url::Url;

/// A downloader that can fetch data from various URL schemes.
///
/// This struct dispatches to the appropriate backend based on URL scheme:
/// - `http://`, `https://` → HTTP downloader
/// - `file://` → File downloader
/// - `s3://` → S3 downloader (requires `s3` feature)
/// - `gs://` → GCS downloader (requires `gcs` feature)
#[derive(Clone, Debug)]
pub struct DefaultDownloader {
    http: HttpDownloader,
    file: Option<FileStorageDownloader>,
    #[cfg(feature = "s3")]
    s3: S3StorageDownloader,
    #[cfg(feature = "gcs")]
    gcs: GcsStorageDownloader,

    config: StorageDownloaderConfig,
}

impl DefaultDownloader {
    /// Creates a downloader with default settings.
    pub async fn new() -> Self {
        Self::from_config(StorageDownloaderConfig::default()).await
    }

    /// Creates a new downloader from configuration.
    pub async fn from_config(config: StorageDownloaderConfig) -> Self {
        let http = HttpDownloader::new(config.max_retries);
        let file =
            if crate::util::is_dev_mode() { Some(FileStorageDownloader::new()) } else { None };

        #[cfg(feature = "s3")]
        let s3 = S3StorageDownloader::new(config.max_retries).await;

        #[cfg(feature = "gcs")]
        let gcs = GcsStorageDownloader::new(config.max_retries).await;

        Self {
            http,
            file,
            #[cfg(feature = "s3")]
            s3,
            #[cfg(feature = "gcs")]
            gcs,
            config,
        }
    }

    /// TODO
    pub fn config(&self) -> &StorageDownloaderConfig {
        &self.config
    }

    /// Returns the cache key for the given URL.
    fn cache_key(url: &Url) -> String {
        let hash = Sha256::digest(url.as_str().as_bytes());
        hex::encode(hash)
    }

    async fn cache<D: StorageDownloader>(
        &self,
        downloader: &D,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let Some(cache_dir) = &self.config.cache_dir else {
            return downloader.download_url_with_limit(url, limit).await;
        };

        let cache_path = cache_dir.join(Self::cache_key(&url));
        if cache_path.exists() {
            tracing::debug!(?url, ?cache_path, "cache hit");
            let data = tokio::fs::read(&cache_path).await?;
            if data.len() > limit {
                return Err(StorageError::SizeLimitExceeded { size: data.len(), limit });
            }
            return Ok(data);
        }

        tracing::debug!(?url, ?cache_path, "cache miss, downloading");
        let data = downloader.download_url_with_limit(url, limit).await?;

        if let Err(err) = tokio::fs::create_dir_all(cache_dir).await {
            tracing::warn!(?err, "failed to create cache directory");
        } else if let Err(err) = tokio::fs::write(&cache_path, &data).await {
            tracing::warn!(?err, "failed to write cache file");
        }

        Ok(data)
    }
}

#[async_trait]
impl StorageDownloader for DefaultDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        max_size: usize,
    ) -> Result<Vec<u8>, StorageError> {
        match url.scheme() {
            "http" | "https" => self.cache(&self.http, url, max_size).await,
            "file" if self.file.is_some() => {
                self.cache(self.file.as_ref().unwrap(), url, max_size).await
            }
            #[cfg(feature = "s3")]
            "s3" => self.cache(&self.s3, url, max_size).await,
            #[cfg(feature = "gcs")]
            "gs" => self.cache(&self.gcs, url, max_size).await,
            scheme => Err(StorageError::UnsupportedScheme(scheme.to_string())),
        }
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        self.download_url_with_limit(url, self.config.max_size).await
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::default::DefaultDownloader;
    use crate::storage::*;

    #[tokio::test]
    async fn test_standard_downloader_http() {
        use crate::storage::default::DefaultDownloader;
        use httpmock::prelude::*;

        let server = MockServer::start();
        let resp_data = vec![0x41, 0x42, 0x43, 0x44];
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(200).body(&resp_data);
        });

        let downloader = DefaultDownloader::new().await;

        let url = Url::parse(&server.url("/test")).unwrap();
        let data = downloader.download_url(url).await.unwrap();

        assert_eq!(data, resp_data);
    }

    #[tokio::test]
    async fn test_file_roundtrip() {
        let downloader = DefaultDownloader::new().await;

        let file_provider = FileStorageUploader::new().unwrap();
        let input_data = b"test data for file roundtrip";
        let url = file_provider.upload_input(input_data).await.unwrap();

        let downloaded = downloader.download_url(url).await.unwrap();
        assert_eq!(downloaded, input_data);
    }
}
