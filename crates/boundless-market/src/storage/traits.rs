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

//! Traits for storage operations.

use super::StorageError;
use async_trait::async_trait;
use auto_impl::auto_impl;
use sha2::{Digest as _, Sha256};
use url::Url;

/// A trait for uploading risc0-zkvm programs and input files to a storage uploader.
#[async_trait]
#[auto_impl(Arc)]
pub trait StorageUploader: Send + Sync {
    /// Upload raw bytes with the given key.
    ///
    /// This is the core upload method that implementations must provide.
    async fn upload_bytes(&self, data: &[u8], key: &str) -> Result<Url, StorageError>;

    /// Upload a risc0-zkvm program binary.
    ///
    /// Returns the URL which can be used to publicly access the uploaded program. This URL can be
    /// included in a request sent to Boundless.
    ///
    /// The default implementation computes the image ID and delegates to [`upload_bytes`](Self::upload_bytes).
    async fn upload_program(&self, program: &[u8]) -> Result<Url, StorageError> {
        let image_id = risc0_zkvm::compute_image_id(program)?;
        self.upload_bytes(program, &format!("{image_id}.bin")).await
    }

    /// Upload the input for use in a proof request.
    ///
    /// Returns the URL which can be used to publicly access the uploaded input. This URL can be
    /// included in a request sent to Boundless.
    ///
    /// The default implementation computes the SHA256 digest and delegates to [`upload_bytes`](Self::upload_bytes).
    async fn upload_input(&self, input: &[u8]) -> Result<Url, StorageError> {
        let digest = Sha256::digest(input);
        self.upload_bytes(input, &format!("{}.input", hex::encode(digest))).await
    }
}

/// A trait for downloading data from URLs.
#[async_trait]
#[auto_impl(Arc)]
pub trait StorageDownloader: Send + Sync {
    /// Downloads data from the given URL, returning at most `limit` bytes.
    ///
    /// This method allows callers to override the default size limit configured in the
    /// downloader. Pass `usize::MAX` to effectively disable the limit for this call.
    /// This is useful for trusted sources or priority requestors that should not be
    /// subject to the global size restrictions.
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError>;

    /// Parses `url` and downloads data, returning at most `limit` bytes.
    async fn download_with_limit(&self, url: &str, limit: usize) -> Result<Vec<u8>, StorageError> {
        self.download_url_with_limit(Url::parse(url)?, limit).await
    }

    /// Downloads the full content from the given URL.
    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        self.download_url_with_limit(url, usize::MAX).await
    }

    /// Parses `url` and downloads the full content.
    async fn download(&self, url: &str) -> Result<Vec<u8>, StorageError> {
        self.download_url(Url::parse(url)?).await
    }
}
