// Copyright 2025 Boundless Foundation, Inc.
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

//! In-memory storage provider for testing.
//!
//! This provider uses an embedded HTTP server ([`httpmock::MockServer`]) to store
//! uploaded data in-memory and serve it via HTTP URLs. This allows the standard
//! [`HttpDownloader`](super::HttpDownloader) to download the data without any
//! special handling.

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use httpmock::MockServer;
use url::Url;

use crate::storage::{StorageError, StorageUploader};

/// In-memory storage provider for testing.
///
/// This provider stores uploaded data in-memory and provides HTTP URLs that can
/// be downloaded using the standard [`HttpDownloader`](super::HttpDownloader).
///
/// Internally, it uses [`httpmock::MockServer`] to:
/// 1. Store uploaded data in-memory
/// 2. Configure mock routes that respond with the uploaded content
/// 3. Return HTTP URLs pointing to these routes
///
/// This design means no special downloader is needed - the standard HTTP
/// downloader works seamlessly with the URLs produced by this uploader.
#[derive(Clone)]
pub struct MockStorageUploader {
    server: Arc<MockServer>,
}

impl fmt::Debug for MockStorageUploader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockStorageUploader").field("base_url", &self.server.base_url()).finish()
    }
}

impl Default for MockStorageUploader {
    fn default() -> Self {
        Self::new()
    }
}

impl MockStorageUploader {
    /// Creates a new mock storage provider with an embedded HTTP server.
    pub fn new() -> Self {
        Self::with_server(MockServer::start())
    }

    /// Creates a new mock storage provider with an existing mock server.
    ///
    /// This allows sharing a mock server between multiple components in tests.
    pub fn with_server(server: MockServer) -> Self {
        Self { server: Arc::new(server) }
    }

    fn upload_and_mock(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        let path = format!("/{key}");

        _ = self.server.mock(|when, then| {
            when.method(httpmock::Method::GET).path(&path);
            then.status(200).header("content-type", "application/octet-stream").body(data);
        });

        let url = Url::parse(&self.server.base_url())?.join(&path)?;

        tracing::debug!("Mock upload available at: {url}");

        Ok(url)
    }
}

#[async_trait]
impl StorageUploader for MockStorageUploader {
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError> {
        self.upload_and_mock(data, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{HttpDownloader, StorageDownloader};

    #[tokio::test]
    async fn test_mock_storage_roundtrip() {
        let uploader = MockStorageUploader::new();

        let test_data = b"test input data";
        let url = uploader.upload_input(test_data).await.unwrap();

        assert_eq!(url.scheme(), "http", "expected http:// URL");

        let downloader = HttpDownloader::default();
        let downloaded = downloader.download_url(url).await.unwrap();

        assert_eq!(downloaded, test_data);
    }
}
