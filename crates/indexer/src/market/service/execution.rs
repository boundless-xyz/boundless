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

use crate::db::market::{
    CycleCountExecution, CycleCountExecutionUpdate, ExecutionWithId, RequestWithId,
};
use crate::db::{DbObj, IndexerDb};
use crate::market::service::IndexerServiceExecutionConfig;
use alloy::primitives::{B256, U256};
use anyhow::{anyhow, Result};
use bonsai_sdk::non_blocking::{Client as BonsaiClient, SessionId};
use boundless_market::storage::StorageDownloader;
use boundless_market::{StandardDownloader, StorageDownloaderConfig};
use broker::futures_retry::retry;
use bytes::Bytes;
use moka::future::Cache;
use moka::policy::EvictionPolicy;
use std::collections::{HashMap, HashSet};

/// Max number of (image_url -> image_id) entries to keep for reuse.
const IMAGE_CACHE_MAX_ENTRIES: u64 = 256;

/// Max length for error strings stored in the DB.
const MAX_ERROR_LENGTH: usize = 100;

/// Type for the image URL -> image_id cache.
type ImageCache = Cache<String, String>;

/// Format an optional request ID for logging
fn fmt_request_id(id: Option<U256>) -> String {
    id.map(|i| format!("0x{:x}", i)).unwrap_or_else(|| "unknown".to_string())
}

fn truncate(s: String) -> String {
    s.chars().take(MAX_ERROR_LENGTH).collect()
}

pub async fn execute_requests(db: DbObj, config: IndexerServiceExecutionConfig) {
    tracing::info!("Started execution task");

    tracing::debug!("Configuration:");
    tracing::debug!("  bento_api_url: {}", config.bento_api_url);
    tracing::debug!("  execution_interval: {}", config.execution_interval.as_secs());
    tracing::debug!("  bento_retry_count: {}", config.bento_retry_count);
    tracing::debug!("  bento_retry_sleep_ms: {}", config.bento_retry_sleep_ms);
    tracing::debug!("  max_concurrent_executing: {}", config.max_concurrent_executing);
    tracing::debug!("  max_status_queries: {}", config.max_status_queries);
    tracing::debug!("  max_retries: {}", config.max_retries);
    tracing::debug!("  retry_base_delay_secs: {}", config.retry_base_delay_secs);
    tracing::debug!("  retry_max_delay_secs: {}", config.retry_max_delay_secs);

    let mut interval = tokio::time::interval(config.execution_interval);

    let bento_client = BonsaiClient::from_parts(
        config.bento_api_url.clone(),
        config.bento_api_key.clone(),
        risc0_zkvm::VERSION,
    )
    .unwrap();
    let downloader = downloader_from_config(&config).await;

    let image_cache: ImageCache = Cache::builder()
        .eviction_policy(EvictionPolicy::lru())
        .max_capacity(IMAGE_CACHE_MAX_ENTRIES)
        .build();
    let mut num_iterations: u32 = 1;

    loop {
        interval.tick().await;

        // Check on and log the current state of cycle counts
        let (pending_count, executing_count, _failed_count, retry_pending_count) =
            match db.count_cycle_counts_by_status().await {
                Ok(counts) => counts,
                Err(e) => {
                    tracing::error!("Unable to count cycle counts by status: {}", e);
                    continue;
                }
            };
        let prev_executing_requests: HashSet<ExecutionWithId> =
            db.get_cycle_counts_executing(config.max_status_queries).await.unwrap();

        tracing::info!(
            "Current cycle counts execution state: {} pending, {} retry_pending, {} executing [{}]",
            pending_count,
            retry_pending_count,
            executing_count,
            prev_executing_requests
                .iter()
                .map(|r| format!(
                    "id={}, digest={:x}",
                    fmt_request_id(r.request_id),
                    r.request_digest
                ))
                .collect::<Vec<_>>()
                .join(", ")
        );

        // Find cycle counts still in state PENDING, up to the max we want to have executing at a time
        let mut requests_to_process: HashSet<RequestWithId> = HashSet::new();

        let mut pending_to_process = 0;
        if executing_count < config.max_concurrent_executing {
            pending_to_process = config.max_concurrent_executing - executing_count;
        }
        if pending_to_process > 0 {
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let pending_cycle_counts =
                match db.get_cycle_counts_pending(pending_to_process, now_secs).await {
                    Ok(requests) => requests,
                    Err(e) => {
                        tracing::error!("Unable to get cycle counts in status PENDING: {}", e);
                        continue;
                    }
                };
            requests_to_process.extend(pending_cycle_counts);
        }

        // Build maps from request_digest for logging and retry logic
        let digest_to_request_id: HashMap<B256, Option<U256>> =
            requests_to_process.iter().map(|req| (req.request_digest, req.request_id)).collect();
        if !requests_to_process.is_empty() {
            tracing::debug!(
                "About to request cycle counts for {} requests: {:?}",
                requests_to_process.len(),
                requests_to_process
                    .iter()
                    .map(|r| format!(
                        "id={}, digest={:x}",
                        fmt_request_id(r.request_id),
                        r.request_digest
                    ))
                    .collect::<Vec<_>>()
            );
        } else {
            tracing::info!("No pending cycle counts to process");
        }

        // Get the inputs and image data for the pending requests
        let digest_vec: Vec<B256> = requests_to_process.iter().map(|r| r.request_digest).collect();
        let request_inputs_and_images = match db.get_request_params_for_execution(&digest_vec).await
        {
            Ok(inputs_and_images) => inputs_and_images,
            Err(e) => {
                tracing::error!("Unable to get requests inputs and image: {}", e);
                continue;
            }
        };

        let mut current_executing_requests = Vec::new();
        let mut failed_executions = Vec::new();
        let mut retry_executions: Vec<(B256, String)> = Vec::new();

        for (request_digest, input_type, input_data, image_id, image_url, max_price) in
            request_inputs_and_images.clone()
        {
            // Get request_id for logging
            let request_id = digest_to_request_id.get(&request_digest).copied().flatten();

            // Validate required fields are not empty
            if input_type.is_empty() || input_data.is_empty() {
                tracing::error!(
                    "Cycle count request id={}, digest={:x} has empty required fields: input_type={}, input_data={}",
                    fmt_request_id(request_id),
                    request_digest,
                    if input_type.is_empty() { "<empty>" } else { &input_type },
                    if input_data.is_empty() { "<empty>" } else { "<present>" }
                );
                failed_executions.push(request_digest);
                continue;
            }
            // When the predicate is of type ClaimDigestMatch, image_id is empty so we download the image to compute it.
            let (image_id, downloaded_image) = match resolve_image_id(
                &image_id,
                &image_url,
                &image_cache,
                &downloader,
            )
            .await
            {
                Ok((id, maybe_image)) => {
                    if maybe_image.is_some() {
                        tracing::debug!(
                            "Downloaded image for cycle count computation request id={}, digest={:x} from URL '{}'",
                            fmt_request_id(request_id),
                            request_digest,
                            image_url
                        );
                    }
                    (id, maybe_image)
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to resolve image for cycle count computation request id={}, digest={:x} from URL '{}': {}",
                        fmt_request_id(request_id),
                        request_digest,
                        image_url,
                        e
                    );
                    retry_executions.push((request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            // Obtain the request input from either the URL or the inline data
            let input: Bytes = match download_or_decode_input(
                request_id,
                request_digest,
                &input_type,
                &input_data,
                &downloader,
            )
            .await
            {
                Ok(input) => input,
                Err(e) => {
                    tracing::error!(
                            "Unable to download or decode {} input for cycle count computation request id={}, digest={:x}: {}",
                            input_type,
                            fmt_request_id(request_id),
                            request_digest,
                            e
                        );
                    retry_executions.push((request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            // Upload the input via the bento API
            tracing::trace!(
                "Uploading input for request id={}, digest={:x}",
                fmt_request_id(request_id),
                request_digest
            );
            let input_uuid: String = match retry(
                config.bento_retry_count,
                config.bento_retry_sleep_ms,
                || async { bento_client.upload_input(input.to_vec()).await },
                "upload_input",
            )
            .await
            {
                Ok(id) => {
                    tracing::debug!(
                        "Uploaded input for cycle count computation request id={}, digest={:x}, obtained ID '{}'",
                        fmt_request_id(request_id),
                        request_digest,
                        id
                    );
                    id
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to upload input for cycle count computation request id={}, digest={:x}: {}",
                        fmt_request_id(request_id),
                        request_digest,
                        e
                    );
                    retry_executions.push((request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            if input_uuid.is_empty() {
                tracing::error!("Empty input UUID received after input upload for cycle count computation request id={}, digest={:x}",
                    fmt_request_id(request_id),
                    request_digest
                );
                retry_executions
                    .push((request_digest, "Empty input UUID from Bento API".to_string()));
                continue;
            }

            // Check if the image exists for the request via the bento API
            tracing::trace!(
                "Checking if image '{}' exists for cycle count computation request id={}, digest={:x}",
                image_id,
                fmt_request_id(request_id),
                request_digest,
            );
            let image_response: bool = match retry(
                config.bento_retry_count,
                config.bento_retry_sleep_ms,
                || async { bento_client.has_img(&image_id).await },
                "has_img",
            )
            .await
            {
                Ok(exists) => {
                    tracing::trace!(
                        "Image exists for cycle count computation request id={}, digest={:x}: {}",
                        fmt_request_id(request_id),
                        request_digest,
                        exists
                    );
                    exists
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to check if image exists for cycle count computation request id={}, digest={:x}: {}",
                        fmt_request_id(request_id),
                        request_digest,
                        e
                    );
                    retry_executions.push((request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            // If the image doesn't exist, use already-downloaded bytes when available
            // or download it from its URL, and upload it via the bento API.
            if !image_response {
                tracing::trace!(
                    "Downloading image for cycle count computation request id={}, digest={:x} from URL '{}'",
                    fmt_request_id(request_id),
                    request_digest,
                    image_url
                );

                let has_downloaded_image = downloaded_image.is_some();
                let image: Vec<u8> = match image_bytes(downloaded_image, &image_url, &downloader)
                    .await
                {
                    Ok(bytes) => {
                        if has_downloaded_image {
                            tracing::trace!(
                                "Reusing already-downloaded image for cycle count computation request id={}, digest={:x}",
                                fmt_request_id(request_id),
                                request_digest
                            );
                        } else {
                            tracing::debug!(
                                "Downloaded image for cycle count computation request id={}, digest={:x} from URL '{}'",
                                fmt_request_id(request_id),
                                request_digest,
                                image_url
                            );
                        }
                        bytes
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to download image for cycle count computation request id={}, digest={:x} from URL '{}': {}",
                            fmt_request_id(request_id),
                            request_digest,
                            image_url,
                            e
                        );
                        retry_executions.push((request_digest, truncate(e.to_string())));
                        continue;
                    }
                };

                match retry(
                    config.bento_retry_count,
                    config.bento_retry_sleep_ms,
                    || async { bento_client.upload_img(&image_id, image.clone()).await },
                    "upload_img",
                )
                .await
                {
                    Ok(result) => {
                        tracing::debug!(
                            "Uploaded image for cycle count computation request id={}, digest={:x} with ID '{}'",
                            fmt_request_id(request_id),
                            request_digest,
                            image_id
                        );
                        result
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to upload image for cycle count computation request id={}, digest={:x} with ID '{}': {}",
                            fmt_request_id(request_id),
                            request_digest,
                            image_id,
                            e
                        );
                        retry_executions.push((request_digest, truncate(e.to_string())));
                        continue;
                    }
                };
            }

            tracing::trace!(
                "Creating execution session for cycle count computation request id={}, digest={:x} with image ID '{}', input ID '{}'",
                fmt_request_id(request_id),
                request_digest,
                image_id,
                input_uuid
            );

            // Start executing the request via the bento API
            let execution_uuid: SessionId = match retry(
                config.bento_retry_count,
                config.bento_retry_sleep_ms,
                || async {
                    bento_client
                        .create_session_with_limit(
                            image_id.clone(),
                            input_uuid.clone(),
                            Vec::new(),
                            true,
                            Some(max_price / 100000000),
                        )
                        .await
                },
                "create_session_with_limit",
            )
            .await
            {
                Ok(id) => {
                    tracing::debug!(
                        "Created session for cycle count computation request id={}, digest={:x}, obtained session ID '{}'",
                        fmt_request_id(request_id),
                        request_digest,
                        id.uuid
                    );
                    id
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create execution session for cycle count computation request id={}, digest={:x}: {}",
                        fmt_request_id(request_id),
                        request_digest,
                        e
                    );
                    retry_executions.push((request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            if execution_uuid.uuid.is_empty() {
                tracing::error!("Empty session UUID received after creating execution session for cycle count computation request id={}, digest={:x}",
                    fmt_request_id(request_id),
                    request_digest
                );
                retry_executions
                    .push((request_digest, "Empty session UUID from Bento API".to_string()));
                continue;
            }

            current_executing_requests
                .push(CycleCountExecution { request_digest, session_uuid: execution_uuid.uuid });
        }

        // Update the cycle count status
        if !current_executing_requests.is_empty() {
            db.set_cycle_counts_executing(&current_executing_requests).await.unwrap();
            tracing::debug!(
                "Updated cycle counts for {} requests with EXECUTING status",
                current_executing_requests.len()
            );
        }

        // Monitor requests in status EXECUTING, i.e. the ones that just started executing
        // as well as any ones started earlier that haven't terminated yet
        let executing_requests: HashSet<ExecutionWithId> =
            db.get_cycle_counts_executing(config.max_status_queries).await.unwrap();

        // Build a map from request_digest to request_id for all executing requests
        // (including ones that were already executing from previous iterations)
        let executing_digest_to_request_id: HashMap<B256, Option<U256>> =
            executing_requests.iter().map(|r| (r.request_digest, r.request_id)).collect();

        if !executing_requests.is_empty() {
            tracing::debug!(
                "Executing cycle count requests found: {}",
                executing_requests
                    .iter()
                    .map(|r| format!(
                        "id={}, digest={:x}",
                        fmt_request_id(r.request_id),
                        r.request_digest
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        } else {
            tracing::info!("No executing cycle count requests found");
        }

        let mut completed_executions = Vec::new();

        // Update the cycle counts and status for execution requests that have completed
        for execution_info in executing_requests.clone() {
            if execution_info.session_uuid.is_empty() {
                tracing::error!("Empty session UUID retrieved from DB for cycle count computation request id={}, digest={:x}",
                    fmt_request_id(execution_info.request_id),
                    execution_info.request_digest
                );
                continue;
            }

            let session_id: SessionId = SessionId::new(execution_info.session_uuid.clone());
            let execution_status = match retry(
                config.bento_retry_count,
                config.bento_retry_sleep_ms,
                || async { session_id.status(&bento_client).await },
                "status",
            )
            .await
            {
                Ok(status) => {
                    tracing::trace!(
                        "Retrieved cycle count status for request id={}, digest={:x}, session '{}': {}",
                        fmt_request_id(execution_info.request_id),
                        execution_info.request_digest,
                        execution_info.session_uuid,
                        status.status
                    );
                    status
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to retrieve cycle count status for request id={}, digest={:x}, session '{}': {}",
                        fmt_request_id(execution_info.request_id),
                        execution_info.request_digest,
                        execution_info.session_uuid,
                        e
                    );
                    retry_executions.push((execution_info.request_digest, truncate(e.to_string())));
                    continue;
                }
            };

            match execution_status.status.as_ref() {
                "SUCCEEDED" => {
                    let stats = match execution_status.stats {
                        Some(stats) => stats,
                        None => {
                            tracing::error!(
                                "Cycle count status for request id={}, digest={:x} is SUCCEEDED, but no stats are provided",
                                fmt_request_id(execution_info.request_id),
                                execution_info.request_digest
                            );
                            failed_executions.push(execution_info.request_digest);
                            continue;
                        }
                    };

                    completed_executions.push(CycleCountExecutionUpdate {
                        request_digest: execution_info.request_digest,
                        program_cycles: U256::from(stats.cycles),
                        total_cycles: U256::from(stats.total_cycles),
                    });
                }
                "RUNNING" => {
                    tracing::debug!(
                        "Cycle count for request id={}, digest={:x} with session '{}' is still running",
                        fmt_request_id(execution_info.request_id),
                        execution_info.request_digest,
                        execution_info.session_uuid
                    );
                }
                _ => {
                    tracing::error!(
                        "Cycle count status for request id={}, digest={:x} is not SUCCEEDED or RUNNING: {}. Scheduling retry.",
                        fmt_request_id(execution_info.request_id),
                        execution_info.request_digest,
                        execution_status.status
                    );
                    retry_executions.push((
                        execution_info.request_digest,
                        truncate(format!("Bento status: {}", execution_status.status)),
                    ));
                }
            }
        }

        if !completed_executions.is_empty() {
            let requests_info = completed_executions
                .iter()
                .map(|c| {
                    format!(
                        "id={}, digest={:x}",
                        fmt_request_id(
                            executing_digest_to_request_id
                                .get(&c.request_digest)
                                .copied()
                                .flatten()
                        ),
                        c.request_digest
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            db.set_cycle_counts_completed(&completed_executions).await.unwrap();
            tracing::debug!(
                "Updated cycle counts for {} requests with COMPLETED status: {}",
                completed_executions.len(),
                requests_info
            );
        }

        let now =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let all_retry_counts: HashMap<B256, u32> = requests_to_process
            .iter()
            .map(|req| (req.request_digest, req.retry_count))
            .chain(executing_requests.iter().map(|r| (r.request_digest, r.retry_count)))
            .collect();

        let mut retry_under_limit = Vec::new();
        let mut retry_exhausted = Vec::new();

        for (digest, error) in &retry_executions {
            let retry_count = all_retry_counts.get(digest).copied().unwrap_or(0);
            if retry_count >= config.max_retries {
                retry_exhausted.push(*digest);
                continue;
            }
            let backoff_delay = config
                .retry_base_delay_secs
                .saturating_mul(2u64.saturating_pow(retry_count))
                .min(config.retry_max_delay_secs);
            let retry_after = now + backoff_delay;
            tracing::info!(
                "Scheduling retry {}/{} for request digest={:x} at timestamp {}: {:.100}",
                retry_count + 1,
                config.max_retries,
                digest,
                retry_after,
                error
            );
            retry_under_limit.push((*digest, error.clone(), retry_after));
        }

        if !retry_under_limit.is_empty() {
            db.set_cycle_counts_retry_pending(&retry_under_limit).await.unwrap();
            tracing::debug!(
                "Updated cycle counts for {} requests with RETRY_PENDING status",
                retry_under_limit.len()
            );
        }

        failed_executions.extend(retry_exhausted);
        if !failed_executions.is_empty() {
            let requests_info = failed_executions
                .iter()
                .map(|c| {
                    let request_id = executing_digest_to_request_id
                        .get(c)
                        .copied()
                        .flatten()
                        .or_else(|| digest_to_request_id.get(c).copied().flatten());
                    format!("id={}, digest={:x}", fmt_request_id(request_id), c)
                })
                .collect::<Vec<_>>()
                .join(", ");
            db.set_cycle_counts_failed(&failed_executions).await.unwrap();
            tracing::debug!(
                "Updated cycle counts for {} requests with FAILED status: {}",
                failed_executions.len(),
                requests_info
            );
        }

        // If we were assigned a max iterations to go through (mainly for test purposes),
        // keep track of the current number of iterations and exit if we've reached the max
        if config.max_iterations > 0 {
            num_iterations += 1;
            if num_iterations > config.max_iterations {
                break;
            }
        }
    }
}

// When image_id is empty, fetches the image and computes the id (or uses cache); otherwise returns the existing id.
async fn resolve_image_id(
    image_id: &str,
    image_url: &str,
    cache: &ImageCache,
    downloader: &StandardDownloader,
) -> Result<(String, Option<Vec<u8>>), anyhow::Error> {
    if !image_id.is_empty() {
        return Ok((image_id.to_string(), None));
    }
    if let Some(id) = cache.get(image_url).await {
        return Ok((id, None));
    }
    let image = downloader.download(image_url).await?;
    let id = risc0_zkvm::compute_image_id(&image)?;
    let id_str = id.to_string();
    cache.insert(image_url.to_string(), id_str.clone()).await;
    Ok((id_str, Some(image)))
}

// Returns image bytes for upload: reuses `downloaded_image` when present, else fetches from `image_url`.
async fn image_bytes(
    downloaded_image: Option<Vec<u8>>,
    image_url: &str,
    downloader: &StandardDownloader,
) -> Result<Vec<u8>, anyhow::Error> {
    if let Some(img) = downloaded_image {
        return Ok(img);
    }
    Ok(downloader.download(image_url).await?)
}

async fn downloader_from_config(config: &IndexerServiceExecutionConfig) -> StandardDownloader {
    StandardDownloader::from_config(StorageDownloaderConfig {
        max_retries: Some(config.bento_retry_count.min(u8::MAX as u64) as u8),
        ..Default::default()
    })
    .await
}

async fn download_or_decode_input(
    request_id: Option<U256>,
    request_digest: B256,
    input_type: &String,
    input_data: &str,
    downloader: &StandardDownloader,
) -> Result<Bytes> {
    if input_type != "Url" && input_type != "Inline" {
        return Err(anyhow!(
            "Invalid input type for request id='{}', digest={:x}: '{}'",
            fmt_request_id(request_id),
            request_digest,
            input_type
        ));
    }

    tracing::debug!(
        "Download or decode input for request id='{}', digest={:x}, input type: '{}'",
        fmt_request_id(request_id),
        request_digest,
        input_type
    );

    let input_data_hex = input_data.trim_start_matches("0x");
    let decoded_input = hex::decode(input_data_hex)?;

    if input_type == "Url" {
        let decoded_url = String::from_utf8(decoded_input)?;
        tracing::debug!(
            "Downloading input for request id='{}', digest={:x}, url: '{}'",
            fmt_request_id(request_id),
            request_digest,
            decoded_url
        );
        let input = downloader.download(&decoded_url).await?;
        tracing::debug!(
            "Downloaded input for request id='{}', digest={:x}",
            fmt_request_id(request_id),
            request_digest
        );
        let decoded_env = boundless_market::input::GuestEnv::decode(&input)?.stdin;
        tracing::debug!(
            "Decoded input for request id='{}', digest={:x}",
            fmt_request_id(request_id),
            request_digest
        );
        Ok(Bytes::from(decoded_env))
    } else {
        let decoded_env = boundless_market::input::GuestEnv::decode(&decoded_input)?.stdin;
        tracing::debug!(
            "Decoded input for request id='{}', digest={:x}",
            fmt_request_id(request_id),
            request_digest
        );
        Ok(Bytes::from(decoded_env))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_market::input::GuestEnv;
    use boundless_test_utils::guests::{ECHO_ELF, ECHO_ID, ECHO_PATH};
    use risc0_zkvm::sha::Digest;
    use std::time::Duration;

    fn test_config() -> IndexerServiceExecutionConfig {
        IndexerServiceExecutionConfig {
            execution_interval: Duration::from_secs(60),
            bento_api_key: "apikey".to_string(),
            bento_api_url: "apiurl".to_string(),
            bento_retry_count: 3,
            bento_retry_sleep_ms: 100,
            max_concurrent_executing: 5,
            max_status_queries: 20,
            max_iterations: 1,
            max_retries: 5,
            retry_base_delay_secs: 900,
            retry_max_delay_secs: 14400,
        }
    }

    #[tokio::test]
    async fn test_resolve_image_id_empty() {
        let url = format!("file://{}", ECHO_PATH);
        let expected_id = Digest::from(ECHO_ID).to_string();
        let cache: ImageCache = Cache::builder()
            .eviction_policy(EvictionPolicy::lru())
            .max_capacity(IMAGE_CACHE_MAX_ENTRIES)
            .build();
        let config = test_config();
        let downloader = downloader_from_config(&config).await;
        let result = resolve_image_id("", &url, &cache, &downloader).await;
        assert!(result.is_ok(), "empty image_id should trigger fetch and compute");
        let (image_id, downloaded_image) = result.unwrap();
        assert!(!image_id.is_empty(), "image_id should be recomputed");
        assert_eq!(image_id, expected_id);
        let bytes = downloaded_image.expect("downloaded_image should be Some when we recompute");
        assert_eq!(bytes.as_slice(), ECHO_ELF);
    }

    #[tokio::test]
    async fn test_resolve_image_id() {
        // When image_id is set we do not fetch.
        let existing_id = risc0_zkvm::compute_image_id(boundless_test_utils::guests::ECHO_ELF)
            .unwrap()
            .to_string();
        let cache: ImageCache = Cache::builder()
            .eviction_policy(EvictionPolicy::lru())
            .max_capacity(IMAGE_CACHE_MAX_ENTRIES)
            .build();
        let config = test_config();
        let downloader = downloader_from_config(&config).await;
        let result =
            resolve_image_id(&existing_id, "http://dev.null/elf", &cache, &downloader).await;
        assert!(result.is_ok());
        let (image_id, downloaded_image) = result.unwrap();
        assert_eq!(image_id, existing_id);
        assert!(downloaded_image.is_none());
    }

    #[tokio::test]
    async fn test_image_bytes_reuse() {
        let bytes = vec![1u8, 2, 3, 4, 5];
        let config = test_config();
        let downloader = downloader_from_config(&config).await;
        let result = image_bytes(Some(bytes.clone()), "http://dev.null/elf", &downloader).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bytes);
    }

    #[tokio::test]
    async fn test_image_bytes_fetch() {
        let config = test_config();
        let downloader = downloader_from_config(&config).await;
        let result = image_bytes(None, "http://dev.null/elf", &downloader).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_inline() {
        let config = test_config();
        let request_digest = B256::from([1; 32]);

        // Create a GuestEnv with some test data
        let test_stdin = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let guest_env = GuestEnv::from_stdin(test_stdin.clone());
        let encoded = guest_env.encode().unwrap();

        // Hex-encode the GuestEnv
        let hex_input = format!("0x{}", hex::encode(&encoded));

        // Decode it
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
            &downloader_from_config(&config).await,
        )
        .await
        .unwrap();

        assert_eq!(result.as_ref(), test_stdin.as_slice());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_inline_without_0x_prefix() {
        let config = test_config();
        let request_digest = B256::from([2; 32]);

        // Create a GuestEnv with some test data
        let test_stdin = vec![10u8, 20, 30, 40];
        let guest_env = GuestEnv::from_stdin(test_stdin.clone());
        let encoded = guest_env.encode().unwrap();

        // Hex-encode without 0x prefix
        let hex_input = hex::encode(&encoded);

        // Decode it
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
            &downloader_from_config(&config).await,
        )
        .await
        .unwrap();

        assert_eq!(result.as_ref(), test_stdin.as_slice());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_inline_empty_stdin() {
        let config = test_config();
        let request_digest = B256::from([3; 32]);

        // Create a GuestEnv with empty stdin
        let guest_env = GuestEnv::from_stdin(Vec::new());
        let encoded = guest_env.encode().unwrap();
        let hex_input = format!("0x{}", hex::encode(&encoded));

        // Decode it
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
            &downloader_from_config(&config).await,
        )
        .await
        .unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_invalid_hex() {
        let config = test_config();
        let request_digest = B256::from([4; 32]);

        // Invalid hex string
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            "0xGGGG",
            &downloader_from_config(&config).await,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_invalid_guest_env() {
        let config = test_config();
        let request_digest = B256::from([5; 32]);

        // Valid hex but invalid GuestEnv encoding (unsupported version)
        let invalid_data = vec![99u8, 1, 2, 3]; // Version 99 is unsupported
        let hex_input = format!("0x{}", hex::encode(&invalid_data));

        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
            &downloader_from_config(&config).await,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_empty_input() {
        let config = test_config();
        let request_digest = B256::from([6; 32]);

        // Empty hex input (decodes to empty byte array)
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            "0x",
            &downloader_from_config(&config).await,
        )
        .await;

        // GuestEnv::decode returns error for empty input
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_v0_encoding() {
        let config = test_config();
        let request_digest = B256::from([7; 32]);

        // V0 encoding: version byte 0 followed by raw stdin bytes
        let test_stdin = vec![100u8, 200, 150];
        let mut v0_encoded = vec![0u8]; // Version 0
        v0_encoded.extend_from_slice(&test_stdin);
        let hex_input = format!("0x{}", hex::encode(&v0_encoded));

        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
            &downloader_from_config(&config).await,
        )
        .await
        .unwrap();

        assert_eq!(result.as_ref(), test_stdin.as_slice());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_url_file() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Enable dev mode for file:// URL support
        std::env::set_var("RISC0_DEV_MODE", "1");

        let config = test_config();
        let request_digest = B256::from([8; 32]);

        // Create test data
        let test_stdin = vec![10u8, 20, 30, 40, 50];
        let guest_env = GuestEnv::from_stdin(test_stdin.clone());
        let encoded = guest_env.encode().unwrap();

        // Write encoded GuestEnv to a temp file
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&encoded).unwrap();
        temp_file.flush().unwrap();
        let file_path = temp_file.path().to_str().unwrap();

        // Create file:// URL and hex-encode it
        let url = format!("file://{}", file_path);
        let hex_url = format!("0x{}", hex::encode(url.as_bytes()));

        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
            &downloader_from_config(&config).await,
        )
        .await
        .unwrap();

        assert_eq!(result.as_ref(), test_stdin.as_slice());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_url_file_not_found() {
        // Enable dev mode for file:// URL support
        std::env::set_var("RISC0_DEV_MODE", "1");

        let config = test_config();
        let request_digest = B256::from([9; 32]);

        // Create a file:// URL to a non-existent file
        let url = "file:///nonexistent/path/to/file.bin";
        let hex_url = format!("0x{}", hex::encode(url.as_bytes()));

        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
            &downloader_from_config(&config).await,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_url_invalid_guest_env() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Enable dev mode for file:// URL support
        std::env::set_var("RISC0_DEV_MODE", "1");

        let config = test_config();
        let request_digest = B256::from([10; 32]);

        // Write invalid GuestEnv data (unsupported version) to temp file
        let invalid_data = vec![99u8, 1, 2, 3];
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&invalid_data).unwrap();
        temp_file.flush().unwrap();
        let file_path = temp_file.path().to_str().unwrap();

        let url = format!("file://{}", file_path);
        let hex_url = format!("0x{}", hex::encode(url.as_bytes()));

        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
            &downloader_from_config(&config).await,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_download_or_decode_input_invalid_input_type() {
        let config = test_config();
        let request_digest = B256::from([4; 32]);

        // Invalid input type
        let result = download_or_decode_input(
            Some(U256::from(1)),
            request_digest,
            &"Unsupported".to_string(),
            "0x",
            &downloader_from_config(&config).await,
        )
        .await;

        assert!(result.is_err());
    }
}
