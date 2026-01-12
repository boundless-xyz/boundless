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

pub use file::{FileStorageDownloader, FileStorageUploader};
pub use http::HttpDownloader;
pub use pinata::PinataStorageUploader;

#[cfg(feature = "gcs")]
pub use gcs::{GcsStorageDownloader, GcsStorageUploader};
#[cfg(feature = "test-utils")]
pub use mock::MockStorageUploader;
#[cfg(feature = "s3")]
pub use s3::{S3StorageDownloader, S3StorageUploader};
