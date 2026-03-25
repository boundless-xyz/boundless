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
//! - Retry policies with exponential backoff (request-level, via `reqwest-retry` middleware)
//! - Resumable downloads via HTTP Range headers (stream-level, on mid-transfer failures)
//! - Stream timeout to detect stalled connections
//! - Size limits
//! - IPFS gateway fallback

use std::time::Duration;

use crate::storage::{config::StorageDownloaderConfig, StorageDownloader, StorageError};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::StatusCode;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

/// Per-chunk stream timeout: if no data arrives for this long, the stream is
/// considered stalled and eligible for a range-based resume.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60);

/// HTTP/HTTPS downloader with retry and resume support.
///
/// This downloader handles `http://` and `https://` URLs. It supports:
/// - Configurable retry policies with exponential backoff (request-level)
/// - Automatic range-based resume on mid-stream failures
/// - Stream timeout to detect stalled connections
/// - Size limits to prevent downloading excessively large files
/// - Optional IPFS gateway fallback for URLs containing `/ipfs/`
#[derive(Clone, Debug)]
pub struct HttpDownloader {
    client: ClientWithMiddleware,
    max_retries: u8,
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
    /// When `max_retries` is set, two layers of retry are enabled:
    /// 1. Request-level retries (via `reqwest-retry` middleware) for transient HTTP errors.
    /// 2. Stream-level retries using HTTP `Range` headers when a download is interrupted
    ///    mid-transfer.
    ///
    /// # Panics
    ///
    /// Panics if `ipfs_gateway` uses a scheme other than `http` or `https`.
    pub fn new(max_retries: Option<u8>, ipfs_gateway: Option<Url>) -> Self {
        let retries = max_retries.unwrap_or(0);
        let mut builder = ClientBuilder::new(reqwest::Client::new());

        if retries > 0 {
            let retry_policy = ExponentialBackoff::builder().build_with_max_retries(retries as u32);
            builder = builder.with(RetryTransientMiddleware::new_with_policy(retry_policy));
        }

        Self {
            client: builder.build(),
            max_retries: retries,
            ipfs_gateway: ipfs_gateway.map(normalize_gateway_url),
        }
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
    ///
    /// On mid-stream failures, retries with a `Range: bytes=<offset>-` header so
    /// only the remaining bytes are re-fetched. Falls back to a full restart if
    /// the server doesn't support Range (responds 200 instead of 206).
    async fn download_impl(&self, url: Url, limit: usize) -> Result<Vec<u8>, StorageError> {
        if !is_http_url(&url) {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        tracing::debug!(%url, "downloading from HTTP");

        let mut buffer: Vec<u8> = Vec::new();
        let mut attempts = 0u8;

        loop {
            let mut request = self.client.get(url.clone());
            if !buffer.is_empty() {
                request =
                    request.header(reqwest::header::RANGE, format!("bytes={}-", buffer.len()));
            }

            let resp = request.send().await.map_err(StorageError::http)?;
            let status = resp.status();

            match status {
                StatusCode::OK => {
                    // Full response — either first request or server ignoring Range
                    buffer.clear();
                    let content_length = resp.content_length().unwrap_or(0) as usize;
                    if content_length > limit {
                        return Err(StorageError::SizeLimitExceeded {
                            size: content_length,
                            limit,
                        });
                    }
                    buffer.reserve(content_length.min(limit));
                }
                StatusCode::PARTIAL_CONTENT => {
                    // 206 — server supports Range, will append to existing buffer
                }
                _ => {
                    return Err(StorageError::HttpStatus(status.as_u16()));
                }
            }

            match stream_body(resp, &mut buffer, limit).await {
                Ok(()) => {
                    tracing::trace!(size = buffer.len(), %url, "downloaded from HTTP");
                    return Ok(buffer);
                }
                Err(StorageError::SizeLimitExceeded { .. }) => {
                    return Err(StorageError::SizeLimitExceeded { size: buffer.len(), limit });
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= self.max_retries || buffer.is_empty() {
                        return Err(e);
                    }
                    tracing::warn!(
                        bytes_received = buffer.len(),
                        attempt = attempts,
                        max_retries = self.max_retries,
                        error = %e,
                        %url,
                        "stream interrupted, attempting range-based resume"
                    );
                }
            }
        }
    }
}

/// Stream the response body into `buffer`, enforcing `limit` and [`STREAM_TIMEOUT`].
async fn stream_body(
    resp: reqwest::Response,
    buffer: &mut Vec<u8>,
    limit: usize,
) -> Result<(), StorageError> {
    let mut stream = resp.bytes_stream();

    loop {
        match tokio::time::timeout(STREAM_TIMEOUT, stream.next()).await {
            Ok(Some(Ok(chunk))) => {
                if buffer.len() + chunk.len() > limit {
                    return Err(StorageError::SizeLimitExceeded {
                        size: buffer.len() + chunk.len(),
                        limit,
                    });
                }
                buffer.extend_from_slice(&chunk);
            }
            Ok(Some(Err(e))) => return Err(StorageError::http(e)),
            Ok(None) => return Ok(()),
            Err(_elapsed) => {
                return Err(StorageError::http(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("download stalled: no data received for {}s", STREAM_TIMEOUT.as_secs()),
                )));
            }
        }
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

    /// Simulates a server that drops the connection mid-stream, then serves the
    /// remainder via a 206 response on the Range retry.
    #[tokio::test]
    async fn resumes_download_after_mid_stream_disconnect() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let data: Vec<u8> = (0..8192u16).flat_map(|i| i.to_le_bytes()).collect();
        let data_for_server = data.clone();
        let split_point = data.len() / 2;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let request_count = Arc::new(AtomicUsize::new(0));
        let rc = request_count.clone();

        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let n = rc.fetch_add(1, Ordering::SeqCst);
                let data = data_for_server.clone();

                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let bytes_read = stream.read(&mut buf).await.unwrap();
                    let request = String::from_utf8_lossy(&buf[..bytes_read]);

                    let has_range = request.lines().any(|l| l.to_lowercase().starts_with("range:"));

                    if n == 0 && !has_range {
                        // First request: send headers + half the data, then drop
                        let header =
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len());
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(&data[..split_point]).await;
                        let _ = stream.flush().await;
                        drop(stream);
                    } else if has_range {
                        let range_line = request
                            .lines()
                            .find(|l| l.to_lowercase().starts_with("range:"))
                            .unwrap();
                        let offset: usize = range_line
                            .split("bytes=")
                            .nth(1)
                            .unwrap()
                            .split('-')
                            .next()
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();

                        let remaining = &data[offset..];
                        let header = format!(
                            "HTTP/1.1 206 Partial Content\r\n\
                             Content-Length: {}\r\n\
                             Content-Range: bytes {}-{}/{}\r\n\r\n",
                            remaining.len(),
                            offset,
                            data.len() - 1,
                            data.len()
                        );
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(remaining).await;
                    } else {
                        let header =
                            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", data.len());
                        let _ = stream.write_all(header.as_bytes()).await;
                        let _ = stream.write_all(&data).await;
                    }
                });
            }
        });

        let url = Url::parse(&format!("http://127.0.0.1:{}/test", addr.port())).unwrap();
        let downloader = HttpDownloader::new(Some(3), None);
        let result = downloader.download_url(url).await.unwrap();

        assert_eq!(result.len(), data.len());
        assert_eq!(result, data);
        assert!(
            request_count.load(Ordering::SeqCst) >= 2,
            "expected at least 2 requests (initial + range resume)"
        );

        server.abort();
    }
}
