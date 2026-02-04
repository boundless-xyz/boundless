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

//! Storage provider implementations.

mod file;
mod http;
mod pinata;

#[cfg(feature = "gcs")]
mod gcs;
#[cfg(feature = "test-utils")]
mod mock;
#[cfg(feature = "s3")]
mod s3;

#[cfg(any(feature = "s3", feature = "gcs"))]
use std::time::Duration;

#[cfg(any(feature = "s3", feature = "gcs"))]
use url::Url;

#[cfg(any(feature = "s3", feature = "gcs"))]
use crate::storage::StorageError;

pub use file::{FileStorageDownloader, FileStorageUploader};
#[cfg(feature = "gcs")]
pub use gcs::{GcsStorageDownloader, GcsStorageUploader};
pub use http::HttpDownloader;
#[cfg(feature = "test-utils")]
pub use mock::MockStorageUploader;
pub use pinata::PinataStorageUploader;
#[cfg(feature = "s3")]
pub use s3::{S3StorageDownloader, S3StorageUploader};

/// Verify that a URL is publicly accessible via an unauthenticated HEAD request.
#[cfg(any(feature = "s3", feature = "gcs"))]
pub(crate) async fn verify_public_url(url: &Url) -> Result<(), StorageError> {
    tracing::trace!(%url, "verifying public accessibility");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| StorageError::Other(e.into()))?;
    let resp = client.head(url.as_str()).send().await.map_err(|e| StorageError::Other(e.into()))?;

    if !resp.status().is_success() {
        return Err(StorageError::Other(anyhow::anyhow!(
            "public URL verification failed: HEAD {} returned {}",
            url,
            resp.status()
        )));
    }
    Ok(())
}
