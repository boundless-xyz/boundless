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

//! Configuration types for storage providers.

use std::path::PathBuf;

use clap::{builder::ArgPredicate, Args, ValueEnum};
use derive_builder::Builder;
use url::Url;

/// The type of storage provider to use for uploads.
#[derive(Default, Clone, Debug, ValueEnum, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageUploaderType {
    /// No storage provider.
    #[default]
    None,
    /// S3 storage provider.
    #[cfg(feature = "s3")]
    S3,
    /// Google Cloud Storage provider.
    #[cfg(feature = "gcs")]
    Gcs,
    /// Pinata storage provider.
    Pinata,
    /// Temporary file storage provider.
    File,
    /// In-memory mock storage provider for testing.
    #[cfg(feature = "test-utils")]
    Mock,
}

/// Configuration for the storage module (upload providers).
///
/// This configuration is used to set up storage providers for uploading programs and inputs.
///
/// # Authentication
///
/// - **S3**: Uses the AWS SDK default credential chain (environment variables,
///   `~/.aws/credentials`, IAM role, etc.). No explicit credentials needed in config.
/// - **GCS**: Uses Application Default Credentials (ADC) via `GOOGLE_APPLICATION_CREDENTIALS`,
///   workload identity, or `gcloud auth application-default login`.
/// - **Pinata**: Requires a JWT token.
#[non_exhaustive]
#[derive(Clone, Default, Debug, Args, Builder)]
pub struct StorageUploaderConfig {
    /// Storage provider to use [possible values: s3, gcs, pinata, file]
    ///
    /// - For 's3', the following option is required:
    ///   --s3-bucket (optionally: --s3-url, --aws-region)
    /// - For 'gcs', the following option is required:
    ///   --gcs-bucket (optionally: --gcs-url)
    /// - For 'pinata', the following option is required:
    ///   --pinata-jwt (optionally: --pinata-api-url, --ipfs-gateway-url)
    /// - For 'file', no additional options are required (optionally: --file-path)
    #[arg(long, env, value_enum, default_value = "none", default_value_ifs = [
        ("s3_bucket", ArgPredicate::IsPresent, "s3"),
        ("gcs_bucket", ArgPredicate::IsPresent, "gcs"),
        ("pinata_jwt", ArgPredicate::IsPresent, "pinata"),
        ("file_path", ArgPredicate::IsPresent, "file")
    ])]
    #[builder(default)]
    pub storage_provider: StorageUploaderType,

    // **S3 Storage Provider Options**
    /// S3 bucket name
    #[cfg(feature = "s3")]
    #[arg(long, env, required_if_eq("storage_provider", "s3"))]
    #[builder(setter(strip_option, into), default)]
    pub s3_bucket: Option<String>,
    /// S3 access key (optional, uses AWS default credential chain if not set)
    #[cfg(feature = "s3")]
    #[arg(long, env, requires("s3_bucket"))]
    #[builder(setter(strip_option, into), default)]
    pub s3_access_key: Option<String>,
    /// S3 secret key (required if s3_access_key is set)
    #[cfg(feature = "s3")]
    #[arg(long, env, requires = "s3_access_key")]
    #[builder(setter(strip_option, into), default)]
    pub s3_secret_key: Option<String>,
    /// S3 endpoint URL (optional, for S3-compatible services like MinIO)
    #[cfg(feature = "s3")]
    #[arg(long, env, requires("s3_bucket"))]
    #[builder(setter(strip_option, into), default)]
    pub s3_url: Option<String>,
    /// AWS region (optional, can be inferred from environment)
    #[cfg(feature = "s3")]
    #[arg(long, env, requires("s3_bucket"))]
    #[builder(setter(strip_option, into), default)]
    pub aws_region: Option<String>,

    /// Use presigned URLs for S3 (default: true)
    #[cfg(feature = "s3")]
    #[arg(long, env, default_value = "true")]
    #[builder(setter(strip_option), default)]
    pub s3_use_presigned: Option<bool>,

    // **GCS Storage Provider Options**
    /// GCS bucket name
    #[cfg(feature = "gcs")]
    #[arg(long, env, required_if_eq("storage_provider", "gcs"))]
    #[builder(setter(strip_option, into), default)]
    pub gcs_bucket: Option<String>,
    /// GCS endpoint URL (optional, for emulators like fake-gcs-server)
    #[cfg(feature = "gcs")]
    #[arg(long, env, requires("gcs_bucket"))]
    #[builder(setter(strip_option), default)]
    pub gcs_url: Option<String>,

    // **Pinata Storage Provider Options**
    /// Pinata JWT
    #[arg(long, env, required_if_eq("storage_provider", "pinata"))]
    #[builder(setter(strip_option, into), default)]
    pub pinata_jwt: Option<String>,
    /// Pinata API URL
    #[arg(long, env, requires("pinata_jwt"))]
    #[builder(setter(strip_option), default)]
    pub pinata_api_url: Option<Url>,
    /// Pinata gateway URL
    #[arg(long, env, requires("pinata_jwt"))]
    #[builder(setter(strip_option), default)]
    pub ipfs_gateway_url: Option<Url>,

    // **File Storage Provider Options**
    /// Path for file storage provider
    #[arg(long)]
    #[builder(setter(strip_option, into), default)]
    pub file_path: Option<PathBuf>,
}

impl StorageUploaderConfig {
    /// Create a new [`StorageUploaderConfigBuilder`].
    pub fn builder() -> StorageUploaderConfigBuilder {
        Default::default()
    }

    /// Create a new configuration for a [StorageUploaderType::File].
    pub fn dev_mode() -> Self {
        Self { storage_provider: StorageUploaderType::File, ..Default::default() }
    }
}

/// Configuration for download operations (construction-time settings).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageDownloaderConfig {
    /// Maximum size in bytes for downloaded content.
    pub max_size: usize,
    /// Maximum number of retry attempts for failed downloads.
    ///
    /// If not set, nothing is retried.
    pub max_retries: Option<u8>,
    /// Optional cache directory for storing downloaded images and inputs.
    ///
    /// If not set, files will be re-downloaded every time.
    pub cache_dir: Option<PathBuf>,
}

impl Default for StorageDownloaderConfig {
    fn default() -> Self {
        Self { max_size: usize::MAX, max_retries: None, cache_dir: None }
    }
}
