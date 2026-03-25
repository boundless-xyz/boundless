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
use anyhow::anyhow;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest::{
    header::{CONTENT_RANGE, RANGE},
    Response, StatusCode,
};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use url::Url;

const RANGE_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ContentRange {
    start: u64,
    end: u64,
    total: u64,
}

impl ContentRange {
    fn len(self) -> usize {
        (self.end - self.start + 1) as usize
    }
}

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
    max_retries: u32,
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

        Self {
            client: builder.build(),
            ipfs_gateway: ipfs_gateway.map(normalize_gateway_url),
            max_retries: max_retries.unwrap_or(0) as u32,
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
    async fn download_impl(&self, url: Url, limit: usize) -> Result<Vec<u8>, StorageError> {
        if !is_http_url(&url) {
            return Err(StorageError::UnsupportedScheme(url.scheme().to_string()));
        }

        tracing::debug!(%url, "downloading from HTTP");

        let probe_end = RANGE_CHUNK_SIZE.saturating_sub(1);
        let probe = self.send_request(&url, Some((0, probe_end))).await?;

        match probe.status() {
            StatusCode::PARTIAL_CONTENT => self.download_with_ranges(url, probe, limit).await,
            StatusCode::OK => self.read_full_response(url, probe, limit).await,
            status => Err(StorageError::HttpStatus(status.as_u16())),
        }
    }

    async fn download_with_ranges(
        &self,
        url: Url,
        resp: Response,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let first_chunk = self.read_ranged_response(resp, 0, RANGE_CHUNK_SIZE - 1, limit).await?;
        if first_chunk.range.total as usize > limit {
            return Err(StorageError::SizeLimitExceeded {
                size: first_chunk.range.total as usize,
                limit,
            });
        }

        let mut buffer = Vec::with_capacity((first_chunk.range.total as usize).min(limit));
        buffer.extend_from_slice(&first_chunk.bytes);

        let mut next_start = first_chunk.range.end + 1;
        while next_start < first_chunk.range.total {
            let next_end = (next_start + RANGE_CHUNK_SIZE - 1).min(first_chunk.range.total - 1);
            let chunk = self.download_range_with_retry(&url, next_start, next_end, limit).await?;
            buffer.extend_from_slice(&chunk.bytes);
            next_start = chunk.range.end + 1;
        }

        tracing::trace!(size = buffer.len(), %url, "downloaded from HTTP with byte ranges");
        Ok(buffer)
    }

    async fn download_range_with_retry(
        &self,
        url: &Url,
        start: u64,
        end: u64,
        limit: usize,
    ) -> Result<DownloadedRange, StorageError> {
        for attempt in 0..=self.max_retries {
            match self.download_range_once(url, start, end, limit).await {
                Ok(chunk) => return Ok(chunk),
                Err(err) if attempt < self.max_retries && should_retry(&err) => {
                    tracing::warn!(
                        %url,
                        start,
                        end,
                        attempt = attempt + 1,
                        retries_remaining = self.max_retries - attempt,
                        error = %err,
                        "Retrying ranged HTTP download chunk"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        unreachable!("retry loop must return before exhausting attempts")
    }

    async fn download_range_once(
        &self,
        url: &Url,
        start: u64,
        end: u64,
        limit: usize,
    ) -> Result<DownloadedRange, StorageError> {
        let resp = self.send_request(url, Some((start, end))).await?;
        match resp.status() {
            StatusCode::PARTIAL_CONTENT => self.read_ranged_response(resp, start, end, limit).await,
            status => Err(StorageError::HttpStatus(status.as_u16())),
        }
    }

    async fn read_full_response(
        &self,
        url: Url,
        resp: Response,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let status = resp.status();
        if !status.is_success() {
            return Err(StorageError::HttpStatus(status.as_u16()));
        }

        // Check content length if available for early rejection
        let content_length = resp.content_length().unwrap_or(0) as usize;
        if content_length > limit {
            return Err(StorageError::SizeLimitExceeded { size: content_length, limit });
        }

        let buffer = Self::read_response_body(resp, content_length.min(limit), limit).await?;

        tracing::trace!(size = buffer.len(), %url, "downloaded from HTTP");
        Ok(buffer)
    }

    async fn read_ranged_response(
        &self,
        resp: Response,
        expected_start: u64,
        expected_end: u64,
        limit: usize,
    ) -> Result<DownloadedRange, StorageError> {
        let range = parse_content_range(&resp)?;
        if range.start != expected_start || range.end > expected_end {
            return Err(StorageError::Other(anyhow!(
                "unexpected content-range {}-{} for requested range {}-{}",
                range.start,
                range.end,
                expected_start,
                expected_end
            )));
        }
        if range.total as usize > limit {
            return Err(StorageError::SizeLimitExceeded { size: range.total as usize, limit });
        }

        let bytes = Self::read_response_body(resp, range.len(), limit).await?;
        if bytes.len() != range.len() {
            return Err(StorageError::Other(anyhow!(
                "incomplete ranged response: expected {} bytes, got {}",
                range.len(),
                bytes.len()
            )));
        }

        Ok(DownloadedRange { range, bytes })
    }

    async fn send_request(
        &self,
        url: &Url,
        range: Option<(u64, u64)>,
    ) -> Result<Response, StorageError> {
        let mut request = self.client.get(url.clone());
        if let Some((start, end)) = range {
            request = request.header(RANGE, format!("bytes={start}-{end}"));
        }
        request.send().await.map_err(StorageError::http)
    }

    async fn read_response_body(
        resp: Response,
        capacity: usize,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let mut buffer = Vec::with_capacity(capacity.min(limit));
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

fn parse_content_range(resp: &Response) -> Result<ContentRange, StorageError> {
    let header = resp.headers().get(CONTENT_RANGE).ok_or_else(|| {
        StorageError::Other(anyhow!("missing Content-Range header on 206 response"))
    })?;
    let header = header
        .to_str()
        .map_err(|err| StorageError::Other(anyhow!("invalid Content-Range header: {err}")))?;
    let range = header.strip_prefix("bytes ").ok_or_else(|| {
        StorageError::Other(anyhow!("unsupported Content-Range format: {header}"))
    })?;
    let (span, total) = range
        .split_once('/')
        .ok_or_else(|| StorageError::Other(anyhow!("invalid Content-Range value: {header}")))?;
    let (start, end) = span
        .split_once('-')
        .ok_or_else(|| StorageError::Other(anyhow!("invalid Content-Range span: {header}")))?;

    let start = start
        .parse::<u64>()
        .map_err(|err| StorageError::Other(anyhow!("invalid Content-Range start: {err}")))?;
    let end = end
        .parse::<u64>()
        .map_err(|err| StorageError::Other(anyhow!("invalid Content-Range end: {err}")))?;
    let total = total
        .parse::<u64>()
        .map_err(|err| StorageError::Other(anyhow!("invalid Content-Range total: {err}")))?;

    if end < start || end >= total {
        return Err(StorageError::Other(anyhow!("invalid Content-Range bounds: {header}")));
    }

    Ok(ContentRange { start, end, total })
}

#[derive(Debug)]
struct DownloadedRange {
    range: ContentRange,
    bytes: Vec<u8>,
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
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

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

    struct TestRangeServer {
        base_url: String,
        requests: Arc<Mutex<Vec<Option<(u64, u64)>>>>,
        _task: JoinHandle<()>,
    }

    impl TestRangeServer {
        async fn new(body: Vec<u8>, support_ranges: bool, fail_once_at_start: Option<u64>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let failed_once = Arc::new(AtomicBool::new(false));

            let requests_clone = requests.clone();
            let failed_once_clone = failed_once.clone();
            let task = tokio::spawn(async move {
                loop {
                    let Ok((mut stream, _)) = listener.accept().await else { break };
                    let body = body.clone();
                    let requests = requests_clone.clone();
                    let failed_once = failed_once_clone.clone();

                    tokio::spawn(async move {
                        let mut buf = Vec::new();
                        let mut chunk = [0_u8; 1024];
                        loop {
                            let read = stream.read(&mut chunk).await.unwrap();
                            if read == 0 {
                                return;
                            }
                            buf.extend_from_slice(&chunk[..read]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }

                        let request = String::from_utf8_lossy(&buf);
                        let range = request.lines().find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            if !name.eq_ignore_ascii_case("range") {
                                return None;
                            }
                            let value = value.trim().strip_prefix("bytes=")?;
                            let (start, end) = value.split_once('-')?;
                            Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()?))
                        });
                        requests.lock().unwrap().push(range);

                        if support_ranges {
                            if let Some((start, end)) = range {
                                let actual_end = end.min(body.len() as u64 - 1);
                                let slice = &body[start as usize..=actual_end as usize];
                                let headers = format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                                    slice.len(),
                                    start,
                                    actual_end,
                                    body.len()
                                );
                                stream.write_all(headers.as_bytes()).await.unwrap();

                                if fail_once_at_start == Some(start)
                                    && !failed_once.swap(true, Ordering::SeqCst)
                                {
                                    let split = (slice.len() / 2).max(1);
                                    stream.write_all(&slice[..split]).await.unwrap();
                                    let _ = stream.shutdown().await;
                                    return;
                                }

                                stream.write_all(slice).await.unwrap();
                                let _ = stream.shutdown().await;
                                return;
                            }
                        }

                        let headers = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        );
                        stream.write_all(headers.as_bytes()).await.unwrap();
                        stream.write_all(&body).await.unwrap();
                        let _ = stream.shutdown().await;
                    });
                }
            });

            Self { base_url: format!("http://{addr}"), requests, _task: task }
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("{}{}", self.base_url, path)).unwrap()
        }

        fn requests(&self) -> Vec<Option<(u64, u64)>> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl Drop for TestRangeServer {
        fn drop(&mut self) {
            self._task.abort();
        }
    }

    #[tokio::test]
    async fn download_uses_ranged_requests_when_supported() {
        let body = vec![0x5a; (RANGE_CHUNK_SIZE as usize * 2) + 123];
        let server = TestRangeServer::new(body.clone(), true, None).await;
        let data =
            HttpDownloader::new(Some(1), None).download_url(server.url("/range")).await.unwrap();

        assert_eq!(data, body);
        assert_eq!(
            server.requests(),
            vec![
                Some((0, RANGE_CHUNK_SIZE - 1)),
                Some((RANGE_CHUNK_SIZE, (RANGE_CHUNK_SIZE * 2) - 1)),
                Some(((RANGE_CHUNK_SIZE * 2), (body.len() as u64) - 1)),
            ]
        );
    }

    #[tokio::test]
    async fn ranged_download_retries_only_failed_chunk() {
        let body = vec![0x33; (RANGE_CHUNK_SIZE as usize * 3) + 321];
        let fail_start = RANGE_CHUNK_SIZE;
        let server = TestRangeServer::new(body.clone(), true, Some(fail_start)).await;
        let data =
            HttpDownloader::new(Some(2), None).download_url(server.url("/retry")).await.unwrap();

        assert_eq!(data, body);
        assert_eq!(
            server.requests(),
            vec![
                Some((0, RANGE_CHUNK_SIZE - 1)),
                Some((fail_start, (RANGE_CHUNK_SIZE * 2) - 1)),
                Some((fail_start, (RANGE_CHUNK_SIZE * 2) - 1)),
                Some(((RANGE_CHUNK_SIZE * 2), (RANGE_CHUNK_SIZE * 3) - 1)),
                Some(((RANGE_CHUNK_SIZE * 3), (body.len() as u64) - 1)),
            ]
        );
    }

    #[tokio::test]
    async fn falls_back_to_full_body_when_range_is_ignored() {
        let body = vec![0x7b; 4096];
        let server = TestRangeServer::new(body.clone(), false, None).await;
        let data =
            HttpDownloader::new(Some(1), None).download_url(server.url("/full")).await.unwrap();

        assert_eq!(data, body);
        assert_eq!(server.requests(), vec![Some((0, RANGE_CHUNK_SIZE - 1))]);
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
