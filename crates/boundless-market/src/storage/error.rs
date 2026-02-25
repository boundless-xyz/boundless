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

//! Error types for the storage module.

use std::error::Error as StdError;

/// Unified error type for storage operations.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum StorageError {
    /// Unsupported URI scheme.
    #[error("unsupported URI scheme: {0}")]
    UnsupportedScheme(String),

    /// URI scheme is supported but the required credentials are not configured.
    #[error("{scheme}:// download requires credentials that are not configured. If this is a private input, only the intended prover with access to the {scheme}:// bucket can process this order.")]
    CredentialsUnavailable {
        /// The URI scheme (e.g. "s3", "gs").
        scheme: String,
    },

    /// URI scheme requires a feature flag that was not enabled at compile time.
    #[error("{scheme}:// support requires the `{feature}` feature flag. Rebuild with feature {feature} enabled to use. Note: {scheme}:// URIs are used for private/sensitive inputs and should only be enabled if you expect to process such orders.")]
    FeatureNotEnabled {
        /// The URI scheme (e.g. "s3", "gs").
        scheme: String,
        /// The cargo feature flag required (e.g. "s3", "gcs").
        feature: String,
    },

    /// Invalid URL.
    #[error("invalid URL: {0}")]
    InvalidUrl(&'static str),

    /// URL parse error.
    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Resource size exceeds maximum allowed size.
    #[error("size limit exceeded: {size} bytes (limit: {limit} bytes)")]
    SizeLimitExceeded {
        /// Actual size of the resource.
        size: usize,
        /// Maximum allowed size.
        limit: usize,
    },

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// HTTP error.
    #[error("HTTP error: {0}")]
    Http(#[source] Box<dyn StdError + Send + Sync + 'static>),

    /// HTTP request failed with non-success status.
    #[error("HTTP request failed with status: {0}")]
    HttpStatus(u16),

    /// S3 error.
    #[cfg(feature = "s3")]
    #[error("S3 error: {0}")]
    S3(#[source] Box<dyn StdError + Send + Sync + 'static>),

    /// Google Cloud Storage error.
    #[cfg(feature = "gcs")]
    #[error("GCS error: {0}")]
    Gcs(#[source] Box<dyn StdError + Send + Sync + 'static>),

    /// Environment variable error.
    #[error("environment variable error: {0}")]
    EnvVar(#[from] std::env::VarError),

    /// Missing configuration parameter.
    #[error("missing config parameter: {0}")]
    MissingConfig(&'static str),

    /// No storage uploader configured.
    #[error("no storage uploader configured")]
    NoUploader,

    /// Other error.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl StorageError {
    /// Create an HTTP error from a reqwest error.
    pub fn http(err: impl Into<Box<dyn StdError + Send + Sync + 'static>>) -> Self {
        Self::Http(err.into())
    }

    /// Create an S3 error.
    #[cfg(feature = "s3")]
    pub fn s3(err: impl Into<Box<dyn StdError + Send + Sync + 'static>>) -> Self {
        Self::S3(err.into())
    }

    /// Create a GCS error.
    #[cfg(feature = "gcs")]
    pub fn gcs(err: impl Into<Box<dyn StdError + Send + Sync + 'static>>) -> Self {
        Self::Gcs(err.into())
    }
}
