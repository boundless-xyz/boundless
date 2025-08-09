// Copyright 2025 RISC Zero, Inc.
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

//! Provider implementations for uploading image and input files such that they are publicly
//! accessible to provers.

use std::{
    fmt,
    fmt::Debug,
    result::Result::Ok,
    sync::atomic::{AtomicUsize, Ordering},
};

use async_trait::async_trait;
use httpmock::MockServer;
use reqwest::Url;

use super::StorageProvider;

/// A `StorageProvider` implementation for testing using [MockServer].
///
/// This provider doesn't actually upload files to a real storage system. Instead, it:
/// 1. Configures the MockServer to respond to requests at a unique URL with the provided content
/// 2. Returns that URL from the upload methods
pub struct MockStorageProvider {
    server: MockServer,
    next_id: AtomicUsize,
}

impl fmt::Debug for MockStorageProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockStorageProvider")
            .field("server", &"<MockServer>")
            .field("next_id", &self.next_id.load(std::sync::atomic::Ordering::Relaxed))
            .finish()
    }
}

/// Error type for the temporary file storage provider.
#[derive(Debug, thiserror::Error)]
pub enum MockStorageError {
    /// Error type for the temporary file storage provider.
    #[error("invalid URL: {0}")]
    UrlParseError(#[from] url::ParseError),
}

impl MockStorageProvider {
    /// Starts a new MockServer and creates a MockStorageProvider from it.
    pub fn start() -> Self {
        Self::from_server(MockServer::start())
    }

    /// Create a new MockStorageProvider with the given MockServer.
    pub fn from_server(server: MockServer) -> Self {
        Self { server, next_id: AtomicUsize::new(1) }
    }

    fn get_next_id(&self) -> usize {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Helper function to upload data and configure the mock server.
    fn upload_and_mock(
        &self,
        data: impl AsRef<[u8]>,
        path_prefix: &str,
    ) -> Result<Url, MockStorageError> {
        let path = format!("/{}/{}", path_prefix, self.get_next_id());

        // set up a mock route to respond to requests for this path
        let _mock_handle = self.server.mock(|when, then| {
            when.method(httpmock::Method::GET).path(&path);
            then.status(200).header("content-type", "application/octet-stream").body(data);
        });

        // Create the URL that points to this resource
        let url = Url::parse(&self.server.base_url()).and_then(|url| url.join(&path))?;

        tracing::debug!("Mock upload available at: {url}");

        Ok(url)
    }
}

#[async_trait]
impl StorageProvider for MockStorageProvider {
    type Error = MockStorageError;

    async fn upload_program(&self, program: &[u8]) -> Result<Url, Self::Error> {
        tracing::debug!("Mocking upload of program: {} bytes", program.len());
        self.upload_and_mock(program, "program")
    }

    async fn upload_input(&self, input: &[u8]) -> Result<Url, Self::Error> {
        tracing::debug!("Mocking upload of input: {} bytes", input.len());
        self.upload_and_mock(input, "input")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_storage_provider() {
        // Create our mock storage provider
        let storage = MockStorageProvider::start();

        // Upload some test data
        let program_data = boundless_market_test_utils::ECHO_ELF;
        let input_data = b"test input data";

        let program_url = storage.upload_program(program_data).await.unwrap();
        let input_url = storage.upload_input(input_data).await.unwrap();

        let response = reqwest::get(program_url).await.unwrap();
        assert_eq!(response.status(), 200);
        let content = response.bytes().await.unwrap();
        assert_eq!(&content[..], program_data);

        let response = reqwest::get(input_url).await.unwrap();
        assert_eq!(response.status(), 200);
        let content = response.bytes().await.unwrap();
        assert_eq!(&content[..], input_data);
    }
}
