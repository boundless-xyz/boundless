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
//! - IPFS gateway fallback

use crate::storage::{config::StorageDownloaderConfig, StorageDownloader, StorageError};
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
/// - Optional IPFS gateway fallback for URLs containing `/ipfs/`
#[derive(Clone, Debug)]
pub struct HttpDownloader {
    client: ClientWithMiddleware,
    ipfs_gateway: Option<Url>,
}

impl Default for HttpDownloader {
    fn default() -> Self {
        let config = StorageDownloaderConfig::default();
        Self::new(config.max_retries, config.ipfs_gateway)
    }
}

impl HttpDownloader {
    /// Creates a new HTTP downloader with optional retry configuration.
    ///
    /// # Panics
    ///
    /// Panics if `ipfs_gateway` uses a scheme other than `http` or `https`.
    pub fn new(max_retries: Option<u8>, ipfs_gateway: Option<Url>) -> Self {
        let mut builder = ClientBuilder::new(reqwest::Client::new());

        if let Some(max_retries) = max_retries {
            let retry_policy =
                ExponentialBackoff::builder().build_with_max_retries(max_retries as u32);
            builder = builder.with(RetryTransientMiddleware::new_with_policy(retry_policy));
        }

        Self { client: builder.build(), ipfs_gateway: ipfs_gateway.map(normalize_gateway_url) }
    }

    /// Attempts to rewrite an IPFS gateway URL to use the configured fallback gateway.
    ///
    /// IPFS gateway URLs follow the path-style format: `https://<gateway>/ipfs/<CID>[/path]`.
    /// This method replaces the gateway while preserving the `/ipfs/...` path and any
    /// base path configured in the fallback gateway.
    ///
    /// Returns `None` if the URL is not an IPFS gateway URL.
    ///
    /// References:
    /// - <https://specs.ipfs.tech/http-gateways/path-gateway/>
    /// - <https://docs.ipfs.tech/how-to/address-ipfs-on-web/>
    fn rewrite_ipfs_url(&self, url: &Url) -> Option<Url> {
        let gateway = self.ipfs_gateway.as_ref()?;
        let ipfs_path = url.path().strip_prefix("/ipfs/")?;

        let mut new_url = gateway.join(&format!("ipfs/{ipfs_path}")).ok()?;
        new_url.set_query(url.query());

        // Don't rewrite if the URL is unchanged (already using the configured gateway)
        (new_url != *url).then_some(new_url)
    }

    /// Internal download implementation without IPFS fallback.
    async fn download_impl(&self, url: Url, limit: usize) -> Result<Vec<u8>, StorageError> {
        if !is_http_url(&url) {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        tracing::debug!(%url, "downloading from HTTP");

        let resp = self.client.get(url.clone()).send().await.map_err(StorageError::http)?;

        let status = resp.status();
        if !status.is_success() {
            return Err(StorageError::HttpStatus(status.as_u16()));
        }

        // Check content length if available for early rejection
        let content_length = resp.content_length().unwrap_or(0) as usize;
        if content_length > limit {
            return Err(StorageError::SizeLimitExceeded { size: content_length, limit });
        }

        // Stream the response body with size checking
        let mut buffer = Vec::with_capacity(content_length.min(limit));
        let mut stream = resp.bytes_stream();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(StorageError::http)?;
            if buffer.len() + chunk.len() > limit {
                return Err(StorageError::SizeLimitExceeded {
                    size: buffer.len() + chunk.len(),
                    limit,
                });
            }
            buffer.extend_from_slice(&chunk);
        }

        tracing::trace!(size = buffer.len(), %url, "downloaded from HTTP");

        Ok(buffer)
    }
}

fn is_http_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
}

fn normalize_gateway_url(mut url: Url) -> Url {
    assert!(
        is_http_url(&url),
        "IPFS gateway URL must use http or https scheme, got: {}",
        url.scheme()
    );

    // Ensure trailing slash so Url::join() appends rather than replaces
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    url
}

/// Returns `true` if the error is worth retrying with a fallback gateway.
fn should_retry(err: &StorageError) -> bool {
    match err {
        // Retry on any HTTP-related error (connection, status, etc.)
        StorageError::Http(_) | StorageError::HttpStatus(_) => true,
        // Don't retry on size limit - the fallback would have the same content
        StorageError::SizeLimitExceeded { .. } => false,
        // Don't retry on other errors
        _ => false,
    }
}

#[async_trait]
impl StorageDownloader for HttpDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        match self.download_impl(url.clone(), limit).await {
            Ok(data) => Ok(data),
            Err(e) if should_retry(&e) => match self.rewrite_ipfs_url(&url) {
                Some(gateway_url) => {
                    tracing::debug!(
                        original = %url,
                        fallback = %gateway_url,
                        error = %e,
                        "Retrying download with IPFS gateway"
                    );
                    self.download_impl(gateway_url, limit).await
                }
                None => Err(e),
            },
            Err(e) => Err(e),
        }
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        // No size limit configured; download full content
        self.download_url_with_limit(url, usize::MAX).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    fn mock_server_with_body(path: &str, body: &[u8]) -> (MockServer, Vec<u8>) {
        let server = MockServer::start();
        let body = body.to_vec();
        server.mock(|when, then| {
            when.method(GET).path(path);
            then.status(200).body(&body);
        });
        (server, body)
    }

    #[tokio::test]
    async fn download_success() {
        let (server, expected) = mock_server_with_body("/test", &[0x41; 4]);
        let url = Url::parse(&server.url("/test")).unwrap();

        let data = HttpDownloader::default().download_url(url).await.unwrap();
        assert_eq!(data, expected);
    }

    #[tokio::test]
    async fn download_rejects_oversized_response() {
        let (server, _) = mock_server_with_body("/test", &[0x41; 100]);
        let url = Url::parse(&server.url("/test")).unwrap();

        let result = HttpDownloader::default().download_url_with_limit(url, 10).await;
        assert!(matches!(result, Err(StorageError::SizeLimitExceeded { .. })));
    }

    #[tokio::test]
    async fn ipfs_gateway_rewrites_basic_path() {
        let (gateway, _) = mock_server_with_body("/ipfs/QmTestHash", b"content");
        let downloader = HttpDownloader::new(None, Some(Url::parse(&gateway.base_url()).unwrap()));

        let url = Url::parse("http://other-gateway.invalid/ipfs/QmTestHash").unwrap();
        let data = downloader.download_url(url).await.unwrap();
        assert_eq!(data, b"content");
    }

    #[tokio::test]
    async fn ipfs_gateway_preserves_nested_path() {
        let (gateway, _) = mock_server_with_body("/ipfs/QmHash/subdir/file.txt", b"nested");
        let downloader = HttpDownloader::new(None, Some(Url::parse(&gateway.base_url()).unwrap()));

        let url = Url::parse("http://other-gateway.invalid/ipfs/QmHash/subdir/file.txt").unwrap();
        let data = downloader.download_url(url).await.unwrap();
        assert_eq!(data, b"nested");
    }

    #[tokio::test]
    async fn ipfs_gateway_preserves_base_path() {
        let (gateway, _) = mock_server_with_body("/v1/ipfs/QmHash", b"with-base");
        let gateway_url = Url::parse(&format!("{}/v1/", gateway.base_url())).unwrap();
        let downloader = HttpDownloader::new(None, Some(gateway_url));

        let url = Url::parse("http://other-gateway.invalid/ipfs/QmHash").unwrap();
        let data = downloader.download_url(url).await.unwrap();
        assert_eq!(data, b"with-base");
    }

    #[tokio::test]
    async fn ipfs_fallback_triggers_on_failure() {
        let (gateway, _) = mock_server_with_body("/ipfs/QmHash", b"fallback");
        let downloader = HttpDownloader::new(None, Some(Url::parse(&gateway.base_url()).unwrap()));

        // Primary URL is unreachable, should fall back to gateway
        let url = Url::parse("http://unreachable.invalid/ipfs/QmHash").unwrap();
        let data = downloader.download_url(url).await.unwrap();
        assert_eq!(data, b"fallback");
    }

    #[tokio::test]
    async fn ipfs_fallback_avoids_infinite_loop() {
        let gateway = MockServer::start();
        // Gateway returns 404 for this CID
        gateway.mock(|when, then| {
            when.method(GET).path("/ipfs/QmNotFound");
            then.status(404);
        });
        let gateway_url = Url::parse(&gateway.base_url()).unwrap();
        let downloader = HttpDownloader::new(None, Some(gateway_url.clone()));

        // URL already points to configured gateway - should fail without retrying itself
        let url = gateway_url.join("ipfs/QmNotFound").unwrap();
        let result = downloader.download_url(url).await;

        assert!(matches!(result, Err(StorageError::HttpStatus(404))));
    }

    #[test]
    #[should_panic(expected = "IPFS gateway URL must use http or https scheme")]
    fn rejects_non_http_gateway_scheme() {
        let _ = HttpDownloader::new(None, Some(Url::parse("ftp://gateway.example.com").unwrap()));
    }
}
