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

//! HTTP/HTTPS download handler.
//!
//! This module provides download functionality for HTTP and HTTPS URLs with support for:
//! - Retry policies
//! - Size limits

use crate::storage::config::StorageDownloaderConfig;
use crate::storage::{StorageDownloader, StorageError};
use alloy::primitives::bytes::Buf;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

/// HTTP/HTTPS downloader with retry support.
///
/// This downloader handles `http://` and `https://` URLs. It supports:
/// - Configurable retry policies with exponential backoff
/// - Size limits to prevent downloading excessively large files
#[derive(Clone, Debug)]
pub struct HttpDownloader {
    client: ClientWithMiddleware,
}

impl Default for HttpDownloader {
    fn default() -> Self {
        Self::new(StorageDownloaderConfig::default().max_retries)
    }
}

impl HttpDownloader {
    /// Creates a new HTTP downloader with optional retry configuration.
    pub fn new(max_retries: Option<u8>) -> Self {
        let mut builder = ClientBuilder::new(reqwest::Client::new());

        if let Some(max_retries) = max_retries {
            let retry_policy =
                ExponentialBackoff::builder().build_with_max_retries(max_retries as u32);
            let retry_middleware = RetryTransientMiddleware::new_with_policy(retry_policy);
            builder = builder.with(retry_middleware);
        }

        Self { client: builder.build() }
    }
}

#[async_trait]
impl StorageDownloader for HttpDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        if !matches!(url.scheme(), "http" | "https") {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        let response = self.client.get(url.clone()).send().await.map_err(StorageError::http)?;

        let status = response.status();
        if !status.is_success() {
            return Err(StorageError::HttpStatus(status.as_u16()));
        }

        // Check content length if available
        let capacity = response.content_length().unwrap_or_default() as usize;
        if capacity > limit {
            return Err(StorageError::SizeLimitExceeded { size: capacity, limit });
        }

        // Stream the response and check size as we go
        let mut buffer = Vec::with_capacity(capacity);
        let mut stream = response.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StorageError::http)?;
            buffer.extend_from_slice(chunk.chunk());
            if buffer.len() > limit {
                return Err(StorageError::SizeLimitExceeded { size: buffer.len(), limit });
            }
        }

        Ok(buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_http_download_success() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(200).body(&resp_data);
        });

        let url = Url::parse(&server.url("/test")).unwrap();
        let data = HttpDownloader::new(None).download_url(url).await.unwrap();

        assert_eq!(data, resp_data);
    }

    #[tokio::test]
    async fn test_http_download_max_size() {
        use httpmock::prelude::*;

        let server = MockServer::start();
        let resp_data = vec![0x41; 100];
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/test");
            then.status(200).body(&resp_data);
        });

        let downloader = HttpDownloader::new(None);
        let url = Url::parse(&server.url("/test")).unwrap();
        let result = downloader.download_url_with_limit(url, 10).await;

        assert!(matches!(result, Err(StorageError::SizeLimitExceeded { .. })));
    }
}
