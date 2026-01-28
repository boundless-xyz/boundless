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

use crate::storage::{
    config::StorageDownloaderConfig, FileStorageDownloader, HttpDownloader, StorageDownloader,
    StorageError,
};
use async_trait::async_trait;
use sha2::{Digest as _, Sha256};
use url::Url;

/// A downloader that can fetch data from various URL schemes.
///
/// This struct dispatches to the appropriate backend based on URL scheme:
/// - `http://`, `https://` → HTTP downloader
/// - `file://` → File downloader
/// - `s3://` → S3 downloader (requires `s3` feature)
/// - `gs://` → GCS downloader (requires `gcs` feature)
#[derive(Clone, Debug)]
pub struct StandardDownloader {
    http: HttpDownloader,
    file: Option<FileStorageDownloader>,
    #[cfg(feature = "s3")]
    s3: Option<super::S3StorageDownloader>,
    #[cfg(feature = "gcs")]
    gcs: Option<super::GcsStorageDownloader>,

    config: StorageDownloaderConfig,
}

impl StandardDownloader {
    /// Creates a downloader with default settings.
    pub async fn new() -> Self {
        Self::from_config(StorageDownloaderConfig::default()).await
    }

    /// Creates a new downloader from configuration.
    pub async fn from_config(config: StorageDownloaderConfig) -> Self {
        let http = HttpDownloader::new(config.max_retries, config.ipfs_gateway.clone());
        let file =
            if crate::util::is_dev_mode() { Some(FileStorageDownloader::new()) } else { None };

        #[cfg(feature = "s3")]
        let s3 = match super::S3StorageDownloader::new(config.max_retries).await {
            Ok(s3) => Some(s3),
            Err(err) => {
                tracing::debug!(%err, "S3 downloader not available, s3:// URLs will fail");
                None
            }
        };

        #[cfg(feature = "gcs")]
        let gcs = match super::GcsStorageDownloader::new(config.max_retries).await {
            Ok(gcs) => Some(gcs),
            Err(err) => {
                tracing::debug!(%err, "GCS downloader not available, gs:// URLs will fail");
                None
            }
        };

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

    /// Returns the downloader configuration.
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
        if let Ok(metadata) = tokio::fs::metadata(&cache_path).await {
            let size = metadata.len() as usize;
            if size > limit {
                return Err(StorageError::SizeLimitExceeded { size, limit });
            }
            tracing::debug!(%url, "cache hit");
            let data = tokio::fs::read(&cache_path).await?;
            tracing::trace!(size = data.len(), %url, "read from cache");
            return Ok(data);
        }

        tracing::debug!(%url, "cache miss");
        let data = downloader.download_url_with_limit(url, limit).await?;

        if let Err(err) = tokio::fs::create_dir_all(cache_dir).await {
            tracing::warn!(%err, cache_path = %cache_path.display(), "failed to create cache directory");
        } else if let Err(err) = tokio::fs::write(&cache_path, &data).await {
            tracing::warn!(%err, cache_path = %cache_path.display(), "failed to write cache file");
        }

        Ok(data)
    }
}

#[async_trait]
impl StorageDownloader for StandardDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        max_size: usize,
    ) -> Result<Vec<u8>, StorageError> {
        match url.scheme() {
            "http" | "https" => self.cache(&self.http, url, max_size).await,
            "file" => match &self.file {
                // File URLs are already local, so caching would just copy them unnecessarily
                Some(file) => file.download_url_with_limit(url, max_size).await,
                None => Err(StorageError::UnsupportedScheme("file (dev mode only)".into())),
            },
            #[cfg(feature = "s3")]
            "s3" => match &self.s3 {
                Some(s3) => self.cache(s3, url, max_size).await,
                None => Err(StorageError::UnsupportedScheme("s3 (credentials unavailable)".into())),
            },
            #[cfg(feature = "gcs")]
            "gs" => match &self.gcs {
                Some(gcs) => self.cache(gcs, url, max_size).await,
                None => Err(StorageError::UnsupportedScheme("gs (credentials unavailable)".into())),
            },
            scheme => Err(StorageError::UnsupportedScheme(scheme.to_string())),
        }
    }

    /// Downloads from the URL using the configured `max_size` limit.
    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        self.download_url_with_limit(url, self.config.max_size).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    #[tokio::test]
    async fn download_http() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x42, 0x43, 0x44];
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(200).body(&resp_data);
        });

        let downloader = StandardDownloader::new().await;

        let url = Url::parse(&server.url("/test")).unwrap();
        let data = downloader.download_url(url).await.unwrap();
        assert_eq!(data, resp_data);
    }

    #[tokio::test]
    async fn cache_roundtrip() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x42, 0x43, 0x44];
        let mock = server.mock(|when, then| {
            when.method(GET).path("/cached");
            then.status(200).body(&resp_data);
        });

        let cache_dir = tempfile::tempdir().unwrap();
        let config = StorageDownloaderConfig {
            cache_dir: Some(cache_dir.path().to_path_buf()),
            ..Default::default()
        };
        let downloader = StandardDownloader::from_config(config).await;

        let url = Url::parse(&server.url("/cached")).unwrap();

        // First download - should hit the server
        let data1 = downloader.download_url(url.clone()).await.unwrap();
        assert_eq!(data1, resp_data);
        mock.assert_hits(1);

        // Second download - should hit the cache, not the server
        let data2 = downloader.download_url(url).await.unwrap();
        assert_eq!(data2, resp_data);
        mock.assert_hits(1); // Still 1, not 2
    }

    #[tokio::test]
    async fn unsupported_scheme() {
        let downloader = StandardDownloader::new().await;

        let url = Url::parse("ftp://example.com/file.txt").unwrap();
        let result = downloader.download_url(url).await;
        assert!(matches!(result, Err(StorageError::UnsupportedScheme(_))));
    }

    #[tokio::test]
    async fn download_url_respects_config_max_size() {
        let server = MockServer::start();
        let resp_data = vec![0x41; 100]; // 100 bytes
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/large");
            then.status(200).body(&resp_data);
        });

        let config = StorageDownloaderConfig {
            max_size: 50, // Only allow 50 bytes
            ..Default::default()
        };
        let downloader = StandardDownloader::from_config(config).await;

        let url = Url::parse(&server.url("/large")).unwrap();
        // download_url() should use config.max_size and reject the response
        let result = downloader.download_url(url).await;
        assert!(matches!(result, Err(StorageError::SizeLimitExceeded { size: 100, limit: 50 })));
    }
}
