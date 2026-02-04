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

//! Configuration types for storage uploaders.

use std::path::PathBuf;

use clap::{builder::ArgPredicate, Args, ValueEnum};
use derive_builder::Builder;
use url::Url;

/// The type of storage uploader to use for uploads.
#[derive(Default, Clone, Debug, ValueEnum, PartialEq, Eq)]
#[non_exhaustive]
pub enum StorageUploaderType {
    /// No storage uploader.
    #[default]
    None,
    /// S3 storage uploader.
    #[cfg(feature = "s3")]
    S3,
    /// Google Cloud Storage uploader.
    #[cfg(feature = "gcs")]
    Gcs,
    /// Pinata storage uploader.
    Pinata,
    /// Temporary file storage uploader.
    File,
    /// In-memory mock storage uploader for testing.
    #[cfg(feature = "test-utils")]
    Mock,
}

/// Configuration for the storage module (upload providers).
///
/// This configuration is used to set up storage uploaders for uploading programs and inputs.
///
/// # Authentication
///
/// - **S3**: Uses the AWS SDK default credential chain (environment variables,
///   `~/.aws/credentials`, IAM role, etc.). No explicit credentials needed in config.
/// - **GCS**: Uses the Google Cloud SDK default credential chain (ADC) via
///   `GOOGLE_APPLICATION_CREDENTIALS`, workload identity, or `gcloud auth application-default login`.
///   Explicit credentials can be provided via `gcs_credentials_json`.
/// - **Pinata**: Requires a JWT token.
#[derive(Clone, Default, Debug, Args, Builder)]
#[non_exhaustive]
pub struct StorageUploaderConfig {
    /// Storage uploader to use [possible values: s3, gcs, pinata, file]
    ///
    /// - For 's3', the following option is required:
    ///   --s3-bucket (optionally: --s3-url, --aws-region)
    /// - For 'gcs', the following option is required:
    ///   --gcs-bucket (optionally: --gcs-url, --gcs-credentials-json)
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
    pub storage_uploader: StorageUploaderType,

    // **S3 Storage Uploader Options**
    /// S3 bucket name
    #[cfg(feature = "s3")]
    #[arg(long, env, required_if_eq("storage_uploader", "s3"))]
    #[builder(setter(strip_option, into), default)]
    pub s3_bucket: Option<String>,
    /// S3 endpoint URL (optional, for S3-compatible services like MinIO)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option, into), default)]
    pub s3_url: Option<String>,
    /// AWS access key (optional, uses AWS default credential chain if not set)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option, into), default)]
    pub aws_access_key_id: Option<String>,
    /// AWS secret key (required if aws_access_key_id is set)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option, into), default)]
    pub aws_secret_access_key: Option<String>,
    /// AWS region (optional, can be inferred from environment)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option, into), default)]
    pub aws_region: Option<String>,
    /// Use presigned URLs for S3 (default: true)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub s3_presigned: Option<bool>,
    /// Return public HTTPS URLs instead of s3:// or presigned URLs (requires bucket to be public)
    #[cfg(feature = "s3")]
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub s3_public_url: Option<bool>,

    // **GCS Storage Uploader Options**
    /// GCS bucket name
    #[cfg(feature = "gcs")]
    #[arg(long, env, required_if_eq("storage_uploader", "gcs"))]
    #[builder(setter(strip_option, into), default)]
    pub gcs_bucket: Option<String>,
    /// GCS endpoint URL (optional, for emulators like fake-gcs-server)
    #[cfg(feature = "gcs")]
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub gcs_url: Option<String>,
    /// GCS service account credentials JSON (optional, uses ADC if not set)
    #[cfg(feature = "gcs")]
    #[arg(long, env)]
    #[builder(setter(strip_option, into), default)]
    pub gcs_credentials_json: Option<String>,
    /// Return public HTTPS URLs instead of gs:// URLs (requires bucket to be publicly readable)
    #[cfg(feature = "gcs")]
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub gcs_public_url: Option<bool>,

    // **Pinata Storage Uploader Options**
    /// Pinata JWT
    #[arg(long, env, required_if_eq("storage_uploader", "pinata"))]
    #[builder(setter(strip_option, into), default)]
    pub pinata_jwt: Option<String>,
    /// Pinata API URL
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub pinata_api_url: Option<Url>,
    /// Pinata gateway URL
    #[arg(long, env)]
    #[builder(setter(strip_option), default)]
    pub ipfs_gateway_url: Option<Url>,

    // **File Storage Uploader Options**
    /// Path for file storage uploader
    #[arg(long)]
    #[builder(setter(strip_option, into), default)]
    pub file_path: Option<PathBuf>,
}

impl StorageUploaderConfig {
    /// Create a new builder to construct a config.
    pub fn builder() -> StorageUploaderConfigBuilder {
        Default::default()
    }

    /// Create a new configuration for a [StorageUploaderType::File].
    pub fn dev_mode() -> Self {
        Self { storage_uploader: StorageUploaderType::File, ..Default::default() }
    }
}

/// Default IPFS gateway URL for fallback downloads.
pub const DEFAULT_IPFS_GATEWAY_URL: &str = "https://gateway.beboundless.cloud";

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
    /// Optional IPFS gateway URL for fallback when downloading IPFS content.
    ///
    /// When set, if an HTTP download fails for a URL containing `/ipfs/`,
    /// the downloader will retry with this gateway.
    pub ipfs_gateway: Option<Url>,
}

impl Default for StorageDownloaderConfig {
    fn default() -> Self {
        Self {
            max_size: usize::MAX,
            max_retries: None,
            cache_dir: None,
            // Safe to unwrap: DEFAULT_IPFS_GATEWAY_URL is a valid URL constant
            ipfs_gateway: Some(Url::parse(DEFAULT_IPFS_GATEWAY_URL).unwrap()),
        }
    }
}
