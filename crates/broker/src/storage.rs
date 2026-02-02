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

//! Storage utilities for fetching and uploading images/inputs from URIs.
//!
//! This module provides broker-specific wrappers around the shared URI fetching
//! functionality in `boundless_market::prover_utils::storage`.

use anyhow::{Context, Result};
use boundless_market::{
    contracts::Predicate,
    prover_utils::{config::MarketConf, storage::fetch_uri_with_config},
};
use hex::FromHex;
use risc0_zkvm::Digest;

/// Creates a [`MarketConf`] from broker configuration for fetch operations.
///
/// This extracts the relevant config values from the broker's `ConfigLock` to create
/// a config that can be passed to the shared URI fetching functions.
///
/// # Arguments
/// * `config` - The broker's configuration lock
/// * `skip_max_size_limit` - If true, sets max file size to usize::MAX (for priority requestors)
pub fn market_config_from_broker(
    config: &crate::config::ConfigLock,
    skip_max_size_limit: bool,
) -> MarketConf {
    let config = config.lock_all().expect("lock failed");
    let mut market_config = config.market.clone();
    if skip_max_size_limit {
        market_config.max_file_size = usize::MAX;
    }
    market_config
}

/// Fetches and uploads an image from a URL to the prover.
///
/// This function:
/// 1. Checks if the prover already has the image cached
/// 2. Fetches the image from the URL using broker config (size limits, retries, etc.)
/// 3. Validates the image ID matches the predicate requirements
/// 4. Uploads the image to the prover
pub async fn upload_image_uri(
    prover: &crate::provers::ProverObj,
    request: &crate::ProofRequest,
    config: &crate::config::ConfigLock,
) -> Result<String> {
    let market_config = market_config_from_broker(config, false);

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
    let image_data = fetch_uri_with_config(&request.imageUrl, &market_config)
        .await
        .with_context(|| format!("Failed to fetch image URI: {}", request.imageUrl))?;

    let image_id = risc0_zkvm::compute_image_id(&image_data)
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
    config: &crate::config::ConfigLock,
    priority_requestors: &crate::requestor_monitor::PriorityRequestors,
) -> Result<String> {
    let client_addr = request.client_address();
    let skip_max_size_limit = priority_requestors.is_priority_requestor(&client_addr);
    let market_config = market_config_from_broker(config, skip_max_size_limit);

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

            let input_data = boundless_market::input::GuestEnv::decode(
                &fetch_uri_with_config(input_uri_str, &market_config)
                    .await
                    .with_context(|| format!("Failed to fetch input URI: {input_uri_str}"))?,
            )
            .with_context(|| format!("Failed to decode input from URI: {input_uri_str}"))?
            .stdin;

            prover.upload_input(input_data).await.context("Failed to upload input")?
        }
        _ => anyhow::bail!("Invalid input type: {:?}", request.input.inputType),
    })
}
