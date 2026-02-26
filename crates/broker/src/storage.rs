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

use crate::config::ConfigLock;
use anyhow::{Context, Result};
use async_trait::async_trait;
use boundless_market::{
    contracts::Predicate,
    storage::{StandardDownloader, StorageDownloader, StorageDownloaderConfig, StorageError},
};
use hex::FromHex;
use risc0_zkvm::Digest;
use std::sync::Arc;
use tokio::sync::RwLock;
use url::Url;

/// A [`StorageDownloader`] wrapper that automatically reloads configuration.
///
/// This wrapper checks for configuration changes before each download operation,
/// replacing the inner [`StandardDownloader`] when settings change. This enables
/// runtime configuration updates without restarting the broker.
#[derive(Clone, Debug)]
pub struct ConfigurableDownloader {
    inner: Arc<RwLock<StandardDownloader>>,
    config_lock: ConfigLock,
}

impl ConfigurableDownloader {
    pub async fn new(config_lock: ConfigLock) -> Result<Self> {
        let config = config_lock
            .lock_all()
            .context("Failed to load config for StorageDownloader")
            .map(|c| StorageDownloaderConfig {
                max_size: c.market.max_file_size,
                max_retries: c.market.max_fetch_retries,
                cache_dir: c.market.cache_dir.clone(),
                ipfs_gateway: c.market.ipfs_gateway_fallback.clone(),
            })?;

        Ok(Self {
            inner: Arc::new(RwLock::new(StandardDownloader::from_config(config).await)),
            config_lock,
        })
    }

    fn current_config(&self) -> Option<StorageDownloaderConfig> {
        match self.config_lock.lock_all() {
            Ok(c) => Some(StorageDownloaderConfig {
                max_size: c.market.max_file_size,
                max_retries: c.market.max_fetch_retries,
                cache_dir: c.market.cache_dir.clone(),
                ipfs_gateway: c.market.ipfs_gateway_fallback.clone(),
            }),
            Err(e) => {
                tracing::warn!("Failed to reload config for StorageDownloader: {e:#}");
                None
            }
        }
    }

    async fn sync_config(&self) {
        let Some(target_config) = self.current_config() else { return };

        if self.inner.read().await.config() == &target_config {
            return; // no change
        }

        let new_downloader = StandardDownloader::from_config(target_config).await;
        *self.inner.write().await = new_downloader;
    }

    async fn downloader(&self) -> StandardDownloader {
        self.sync_config().await;
        self.inner.read().await.clone()
    }
}

#[async_trait]
impl StorageDownloader for ConfigurableDownloader {
    async fn download_url_with_limit(
        &self,
        url: Url,
        limit: usize,
    ) -> Result<Vec<u8>, StorageError> {
        self.downloader().await.download_url_with_limit(url, limit).await
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, StorageError> {
        // Delegate to inner downloader which applies config.max_size limit.
        self.downloader().await.download_url(url).await
    }
}

pub async fn upload_image_uri(
    prover: &crate::provers::ProverObj,
    request: &crate::ProofRequest,
    downloader: &impl StorageDownloader,
) -> Result<String> {
    let predicate = Predicate::try_from(request.requirements.predicate.clone())
        .with_context(|| format!("Failed to parse predicate for request {:x}", request.id))?;

    let image_id_str = predicate.image_id().map(|image_id| image_id.to_string());

    // When predicate is ClaimDigestMatch, we do not have the image id, so we must always download and upload the image.
    if let Some(ref image_id_str) = image_id_str {
        if prover.has_image(image_id_str).await? {
            tracing::debug!(
                "Skipping program upload for cached image ID: {image_id_str} for request {:x}",
                request.id
            );
            return Ok(image_id_str.clone());
        }
    }

    tracing::debug!(
        "Fetching program for request {:x} with image ID {image_id_str:?} from URI {}",
        request.id,
        request.imageUrl
    );
    let image_data = downloader
        .download(&request.imageUrl)
        .await
        .with_context(|| format!("Failed to fetch image URI: {}", request.imageUrl))?;

    let image_id = prover
        .compute_image_id(&image_data)
        .await
        .context(format!("Failed to compute image ID for request {:x}", request.id))?;

    if let Some(ref image_id_str) = image_id_str {
        let required_image_id = Digest::from_hex(image_id_str)?;
        anyhow::ensure!(
            image_id == required_image_id,
            "image ID does not match requirements; expect {}, got {}",
            required_image_id,
            image_id
        );
    }

    let image_id_str = image_id.to_string();

    tracing::debug!(
        "Uploading program for request {:x} with image ID {image_id_str} to prover",
        request.id
    );
    prover
        .upload_image(&image_id_str, image_data)
        .await
        .context("Failed to upload image to prover")?;

    Ok(image_id_str)
}

/// Fetches and uploads input from a URL to the prover.
///
/// This function:
/// 1. For inline inputs: decodes and uploads directly
/// 2. For URL inputs: fetches using broker config (with optional size limit bypass for priority requestors)
/// 3. Decodes the input using GuestEnv format
/// 4. Uploads to the prover
pub async fn upload_input_uri(
    prover: &crate::provers::ProverObj,
    request: &crate::ProofRequest,
    downloader: &impl StorageDownloader,
    priority_requestors: &crate::requestor_monitor::PriorityRequestors,
) -> Result<String> {
    Ok(match request.input.inputType {
        boundless_market::contracts::RequestInputType::Inline => prover
            .upload_input(
                boundless_market::input::GuestEnv::decode(&request.input.data)
                    .with_context(|| "Failed to decode input")?
                    .stdin,
            )
            .await
            .context("Failed to upload input data")?,

        boundless_market::contracts::RequestInputType::Url => {
            let input_uri_str =
                std::str::from_utf8(&request.input.data).context("input url is not utf8")?;
            tracing::debug!("Input URI string: {input_uri_str}");

            let client_addr = request.client_address();
            let input = if priority_requestors.is_priority_requestor(&client_addr) {
                downloader.download_with_limit(input_uri_str, usize::MAX).await
            } else {
                downloader.download(input_uri_str).await
            }
            .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?;
            let input_data = boundless_market::input::GuestEnv::decode(&input)
                .with_context(|| format!("Failed to decode input from URI: {input_uri_str}"))?
                .stdin;

            prover.upload_input(input_data).await.context("Failed to upload input")?
        }
        _ => anyhow::bail!("Invalid input type: {:?}", request.input.inputType),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MarketConfig;
    use boundless_market::storage::{MockStorageUploader, StorageUploader};

    #[tokio::test]
    async fn test_config_update_and_snapshot_behavior() {
        let config_lock = ConfigLock::default();
        let downloader = ConfigurableDownloader::new(config_lock.clone()).await.unwrap();

        // Simulate holding a downloader during a long download
        let held = downloader.downloader().await;
        assert_eq!(held.config().max_size, MarketConfig::default().max_file_size);
        assert_eq!(held.config().cache_dir, MarketConfig::default().cache_dir);

        // Config update should succeed even while `held` is alive
        {
            let mut cfg = config_lock.load_write().unwrap();
            cfg.market.max_file_size = 9999;
            cfg.market.cache_dir = Some("/tmp/cache".into());
        }

        // New downloader() call should see the updated config
        let new = downloader.downloader().await;
        assert_eq!(new.config().max_size, 9999);
        assert_eq!(new.config().cache_dir, Some("/tmp/cache".into()));

        // Original held downloader still has old config (it's a snapshot)
        assert_eq!(held.config().max_size, MarketConfig::default().max_file_size);
        assert_eq!(held.config().cache_dir, MarketConfig::default().cache_dir);
    }

    #[tokio::test]
    async fn test_download_enforces_max_file_size() {
        let config_lock = ConfigLock::default();
        let downloader = ConfigurableDownloader::new(config_lock.clone()).await.unwrap();
        // Reduce the max file size limit
        {
            let mut cfg = config_lock.load_write().unwrap();
            cfg.market.max_file_size = 10; // 10 bytes limit
        }

        // Upload data larger than the limit
        let uploader = MockStorageUploader::new();
        let large_data = vec![0u8; 100]; // 100 bytes, exceeds limit
        let url = uploader.upload_bytes(&large_data, "large_file").await.unwrap();

        // downloading should fail with size limit exceeded
        let result = downloader.download(url.as_str()).await;
        assert!(matches!(result, Err(StorageError::SizeLimitExceeded { .. })));
        let result = downloader.download_url(url).await;
        assert!(matches!(result, Err(StorageError::SizeLimitExceeded { .. })));
    }
}
