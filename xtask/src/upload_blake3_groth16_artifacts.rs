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

use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use clap::Parser;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tokio_tar::Builder;

#[derive(Debug, Parser)]
pub struct UploadBlake3Groth16Artifacts {
    /// blake3-groth16 setup directory
    #[arg(long, env)]
    blake3_groth16_setup_dir: std::path::PathBuf,

    /// S3-compatible bucket name
    #[arg(long)]
    bucket: String,

    /// S3-compatible endpoint URL (optional as not needed if using AWS S3)
    #[arg(long)]
    endpoint: Option<String>,

    /// S3-compatible region (optional, defaults to us-east-1)
    #[arg(long)]
    region: Option<String>,

    /// Access key ID
    #[arg(long, env)]
    access_key_id: Option<String>,

    /// Secret access key
    #[arg(long, env)]
    secret_access_key: Option<String>,

    /// Is R2 bucket
    #[arg(long)]
    is_r2_bucket: bool,

    #[arg(long, default_value = "6")]
    /// XZ compression level (1-9)
    compression_level: i32,

    #[arg(long, default_value_t = false)]
    publish: bool,
}

const ARCHIVE_NAME: &str = "blake3_groth16_artifacts.tar.xz";
const FILES_TO_ARCHIVE: &[&str] = &[
    "verify_for_guest_final.zkey",
    "verify_for_guest_graph.bin",
    "fuzzed_msm_results.bin",
    "preprocessed_coeffs.bin",
];

impl UploadBlake3Groth16Artifacts {
    pub async fn run(&self) {
        let tar_xz_path = self.blake3_groth16_setup_dir.join(ARCHIVE_NAME);

        let file = self.create_tar_xz(tar_xz_path.clone()).await.unwrap();
        tracing::info!(
            "Created tar.xz at {} ({} bytes)",
            tar_xz_path.display(),
            file.metadata().await.unwrap().len()
        );
        if self.publish {
            self.publish(tar_xz_path).await.unwrap();
        } else {
            tracing::info!("Skipping upload as --publish not set");
        }
    }

    async fn publish(&self, tar_xz_path: impl AsRef<std::path::Path>) -> Result<()> {
        let uploader = BucketUploader::new(
            self.is_r2_bucket,
            self.endpoint.clone(),
            self.access_key_id.clone(),
            self.secret_access_key.clone(),
            None,
            self.bucket.clone(),
            self.region.clone(),
        )
        .await
        .context("Failed to create bucket uploader")?;

        tracing::info!("Uploading to bucket: {}", &uploader.bucket);

        let byte_stream = ByteStream::read_from()
            .path(tar_xz_path)
            .build()
            .await
            .context("Failed to create s3 byte stream")?;

        uploader
            .client
            .put_object()
            .bucket(&uploader.bucket)
            .key(format!("v3/proving/{}", ARCHIVE_NAME))
            .body(byte_stream)
            .send()
            .await
            .context("Failed to upload artifacts to S3 bucket")?;

        tracing::info!("Uploaded artifacts to bucket: {}", &uploader.bucket);
        Ok(())
    }

    async fn create_tar_xz(&self, output_path: impl AsRef<std::path::Path>) -> Result<File> {
        tracing::info!(
            "Creating tar.xz at {} with compression level {}",
            output_path.as_ref().display(),
            self.compression_level
        );
        let level = async_compression::Level::Precise(self.compression_level);
        let output_file = File::create(output_path).await?;

        let writer = async_compression::tokio::write::LzmaEncoder::with_quality(output_file, level);

        let mut tar_builder = Builder::new(writer);

        for file_path in FILES_TO_ARCHIVE {
            tracing::info!("Adding file to tar.xz: {}", file_path);
            tar_builder
                .append_file(
                    format!("blake3_groth16_artifacts/{}", file_path),
                    &mut File::open(self.blake3_groth16_setup_dir.join(file_path)).await?,
                )
                .await?;
        }
        tar_builder.finish().await?;
        let mut writer = tar_builder.into_inner().await?;
        writer.shutdown().await?;

        Ok(writer.into_inner())
    }
}

struct BucketUploader {
    client: Client,
    bucket: String,
}

impl BucketUploader {
    async fn new(
        is_r2_bucket: bool,
        endpoint: Option<String>,
        access_key_id: Option<String>,
        secret_access_key: Option<String>,
        session_token: Option<String>,
        bucket: String,
        region: Option<String>,
    ) -> Result<Self> {
        let mut credentials: Option<Credentials> = None;
        if is_r2_bucket {
            credentials = Some(Credentials::new(
                access_key_id.unwrap(),
                secret_access_key.unwrap(),
                None,
                None,
                "R2",
            ));
        } else if access_key_id.is_some() && secret_access_key.is_some() {
            credentials = Some(Credentials::new(
                access_key_id.unwrap(),
                secret_access_key.unwrap(),
                session_token,
                None,
                "static",
            ));
        }

        let mut config_builder = aws_config::defaults(BehaviorVersion::latest());

        if let Some(credentials) = credentials {
            config_builder = config_builder.credentials_provider(credentials);
        }

        if let Some(endpoint_url) = endpoint {
            config_builder = config_builder.endpoint_url(endpoint_url);
        }

        if let Some(region_value) = region {
            config_builder = config_builder.region(Region::new(region_value));
        }

        let config = config_builder.load().await;
        let client = Client::new(&config);

        Ok(Self { client, bucket })
    }
}
