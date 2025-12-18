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

use crate::db::market::{
    CycleCountExecution, CycleCountExecutionUpdate, ExecutionWithId, RequestWithId,
};
use crate::db::{DbObj, IndexerDb};
use crate::market::service::IndexerServiceExecutionConfig;
use alloy::primitives::{B256, U256};
use anyhow::Result;
use bonsai_sdk::non_blocking::{Client as BonsaiClient, SessionId};
use boundless_market::storage::fetch_url;
use broker::futures_retry::retry;
use bytes::Bytes;
use std::collections::{HashMap, HashSet};

pub async fn execute_requests(db: DbObj, config: IndexerServiceExecutionConfig) {
    tracing::info!("Started execution task");

    tracing::debug!("Configuration:");
    tracing::debug!("  execution_interval: {}", config.execution_interval.as_secs());
    tracing::debug!("  bento_retry_count: {}", config.bento_retry_count);
    tracing::debug!("  bento_retry_sleep_ms: {}", config.bento_retry_sleep_ms);
    tracing::debug!("  max_concurrent_executing: {}", config.max_concurrent_executing);
    tracing::debug!("  max_status_queries: {}", config.max_status_queries);

    let mut interval = tokio::time::interval(config.execution_interval);

    let bento_client = BonsaiClient::from_parts(
        config.bento_api_url.clone().unwrap(),
        config.bento_api_key.clone().unwrap(),
        risc0_zkvm::VERSION,
    )
    .unwrap();

    loop {
        interval.tick().await;

        // Check on and log the current state of cycle counts
        let (pending_count, executing_count, _failed_count) =
            match db.count_cycle_counts_by_status().await {
                Ok(counts) => counts,
                Err(e) => {
                    tracing::error!("Unable to count cycle counts by status: {}", e);
                    continue;
                }
            };

        tracing::info!(
            "Current cycle counts execution state: {} pending, {} executing",
            pending_count,
            executing_count
        );

        // Find cycle counts still in state PENDING, up to the max we want to have executing at a time
        let mut requests_to_process: HashSet<RequestWithId> = HashSet::new();

        let mut pending_to_process = 0;
        if executing_count < config.max_concurrent_executing {
            pending_to_process = config.max_concurrent_executing - executing_count;
        }
        if pending_to_process > 0 {
            let pending_cycle_counts = match db.get_cycle_counts_pending(pending_to_process).await {
                Ok(requests) => requests,
                Err(e) => {
                    tracing::error!("Unable to get cycle counts in status PENDING: {}", e);
                    continue;
                }
            };
            requests_to_process.extend(pending_cycle_counts);
        }

        // Build a map from request_digest to request_id for logging
        let digest_to_request_id: HashMap<B256, U256> =
            requests_to_process.iter().map(|req| (req.request_digest, req.request_id)).collect();

        if !requests_to_process.is_empty() {
            tracing::debug!(
                "About to request cycle counts for {} requests: {:?}",
                requests_to_process.len(),
                requests_to_process
                    .iter()
                    .map(|r| format!("id=0x{:x}, digest={:x}", r.request_id, r.request_digest))
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

        let mut current_executing_requests = HashSet::new();
        let mut failed_executions = Vec::new();

        for (request_digest, input_type, input_data, image_id, image_url, max_price) in
            request_inputs_and_images.clone()
        {
            let request_id = match digest_to_request_id.get(&request_digest).copied() {
                Some(id) => id,
                None => {
                    tracing::error!(
                        "Cycle count request digest={:x} not found in digest_to_request_id map - this should not happen. Skipping request.",
                        request_digest
                    );
                    failed_executions.push(request_digest);
                    continue;
                }
            };

            // Validate required fields are not empty
            if image_id.is_empty() || input_type.is_empty() || input_data.is_empty() {
                tracing::error!(
                    "Cycle count request id=0x{:x}, digest={:x} has empty required fields: image_id={}, input_type={}, input_data={}",
                    request_id,
                    request_digest,
                    if image_id.is_empty() { "<empty>" } else { &image_id },
                    if input_type.is_empty() { "<empty>" } else { &input_type },
                    if input_data.is_empty() { "<empty>" } else { "<present>" }
                );
                failed_executions.push(request_digest);
                continue;
            }

            // Obtain the request input from either the URL or the inline data
            let input: Bytes = match download_or_decode_input(
                &config,
                request_digest,
                &input_type,
                &input_data,
            )
            .await
            {
                Ok(input) => input,
                Err(e) => {
                    tracing::error!(
                            "Unable to download or decode {} input for cycle count computation request id=0x{:x}, digest={:x}: {}",
                            input_type,
                            request_id,
                            request_digest,
                            e
                        );
                    failed_executions.push(request_digest);
                    continue;
                }
            };

            // Upload the input via the bento API
            tracing::trace!(
                "Uploading input for request id=0x{:x}, digest={:x}",
                request_id,
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
                        "Uploaded input for cycle count computation request id=0x{:x}, digest={:x}, obtained ID '{}'",
                        request_id,
                        request_digest,
                        id
                    );
                    id
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to upload input for cycle count computation request id=0x{:x}, digest={:x}: {}",
                        request_id,
                        request_digest,
                        e
                    );
                    continue;
                }
            };

            // Check if the image exists for the request via the bento API
            tracing::trace!(
                "Checking if image '{}' exists for cycle count computation request id=0x{:x}, digest={:x}",
                image_id,
                request_id,
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
                    tracing::trace!("Image exists for cycle count computation request id=0x{:x}, digest={:x}: {}", request_id, request_digest, exists);
                    exists
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to check if image exists for cycle count computation request id=0x{:x}, digest={:x}: {}",
                        request_id,
                        request_digest,
                        e
                    );
                    continue;
                }
            };

            // If the image doesn't exist, download it from its URL and upload it via the bento API
            if !image_response {
                tracing::trace!(
                    "Downloading image for cycle count computation request id=0x{:x}, digest={:x} from URL '{}'",
                    request_id,
                    request_digest,
                    image_url
                );

                let image: Vec<u8> = match retry(
                    config.bento_retry_count,
                    config.bento_retry_sleep_ms,
                    || async { fetch_url(&image_url).await },
                    "fetch_url",
                )
                .await
                {
                    Ok(bytes) => {
                        tracing::debug!(
                            "Downloaded image for cycle count computation request id=0x{:x}, digest={:x} from URL '{}'",
                            request_id,
                            request_digest,
                            image_url
                        );
                        bytes
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to download image for cycle count computation request id=0x{:x}, digest={:x} from URL '{}': {}",
                            request_id,
                            request_digest,
                            image_url,
                            e
                        );
                        failed_executions.push(request_digest);
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
                            "Uploaded image for cycle count computation request id=0x{:x}, digest={:x} with ID '{}'",
                            request_id,
                            request_digest,
                            image_id
                        );
                        result
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to upload image for cycle count computation request id=0x{:x}, digest={:x} with ID '{}': {}",
                            request_id,
                            request_digest,
                            image_id,
                            e
                        );
                        continue;
                    }
                };
            }

            tracing::trace!(
                "Creating execution session for cycle count computation request id=0x{:x}, digest={:x} with image ID '{}', input ID '{}'",
                request_id,
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
                        "Created session for cycle count computation request id=0x{:x}, digest={:x}, obtained session ID '{}'",
                        request_id,
                        request_digest,
                        id.uuid
                    );
                    id
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create execution session for cycle count computation request id=0x{:x}, digest={:x}: {}",
                        request_id,
                        request_digest,
                        e
                    );
                    continue;
                }
            };

            current_executing_requests
                .insert(CycleCountExecution { request_digest, session_uuid: execution_uuid.uuid });
        }

        // Update the cycle count status
        db.set_cycle_counts_executing(&current_executing_requests).await.unwrap();
        tracing::debug!(
            "Updated cycle counts for {} requests with EXECUTING status",
            current_executing_requests.len()
        );

        // Monitor requests in status EXECUTING, i.e. the ones that just started executing
        // as well as any ones started earlier that haven't terminated yet
        let executing_requests: HashSet<ExecutionWithId> =
            db.get_cycle_counts_executing(config.max_status_queries).await.unwrap();
        if !executing_requests.is_empty() {
            tracing::debug!(
                "Executing cycle count requests found: {}",
                executing_requests
                    .iter()
                    .map(|r| format!("id=0x{:x}, digest={:x}", r.request_id, r.request_digest))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        } else {
            tracing::info!("No executing cycle count requests found");
        }

        let mut completed_executions = Vec::new();

        // Update the cycle counts and status for execution requests that have completed
        for execution_info in executing_requests.clone() {
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
                        "Retrieved cycle count status for request id=0x{:x}, digest={:x}, session '{}': {}",
                        execution_info.request_id,
                        execution_info.request_digest,
                        execution_info.session_uuid,
                        status.status
                    );
                    status
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to retrieve cycle count status for request id=0x{:x}, digest={:x}, session '{}': {}",
                        execution_info.request_id,
                        execution_info.request_digest,
                        execution_info.session_uuid,
                        e
                    );
                    continue;
                }
            };

            match execution_status.status.as_ref() {
                "SUCCEEDED" => {
                    let stats = match execution_status.stats {
                        Some(stats) => stats,
                        None => {
                            tracing::error!(
                                "Cycle count status for request id=0x{:x}, digest={:x} is SUCCEEDED, but no stats are provided",
                                execution_info.request_id,
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
                    tracing::trace!(
                        "Cycle count for request id=0x{:x}, digest={:x} with session '{}' is still running",
                        execution_info.request_id,
                        execution_info.request_digest,
                        execution_info.session_uuid
                    );
                }
                _ => {
                    tracing::error!(
                        "Cycle count status for request id=0x{:x}, digest={:x} is not SUCCEEDED or RUNNING: {}. Marking as FAILED.",
                        execution_info.request_id,
                        execution_info.request_digest,
                        execution_status.status
                    );
                    failed_executions.push(execution_info.request_digest);
                }
            }
        }

        if !completed_executions.is_empty() {
            let requests_info = completed_executions
                .iter()
                .map(|c| {
                    format!(
                        "id=0x{:x}, digest={:x}",
                        digest_to_request_id.get(&c.request_digest).unwrap(),
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

        if !failed_executions.is_empty() {
            let requests_info = failed_executions
                .iter()
                .map(|c| format!("id=0x{:x}, digest={:x}", digest_to_request_id.get(c).unwrap(), c))
                .collect::<Vec<_>>()
                .join(", ");
            db.set_cycle_counts_failed(&failed_executions).await.unwrap();
            tracing::debug!(
                "Updated cycle counts for {} requests with FAILED status: {}",
                failed_executions.len(),
                requests_info
            );
        }
    }
}

async fn download_or_decode_input(
    config: &IndexerServiceExecutionConfig,
    request_digest: B256,
    input_type: &String,
    input_data: &str,
) -> Result<Bytes> {
    tracing::debug!(
        "Download or decode input for '{}', input type: '{}'",
        request_digest,
        input_type
    );

    let input_data_hex = input_data.trim_start_matches("0x");
    let decoded_input = hex::decode(input_data_hex)?;

    if input_type == "Url" {
        let decoded_url = String::from_utf8(decoded_input)?;
        tracing::debug!("Downloading input for '{}', url: '{}'", request_digest, decoded_url);
        let input = retry(
            config.bento_retry_count,
            config.bento_retry_sleep_ms,
            || async { fetch_url(&decoded_url).await },
            "fetch_url",
        )
        .await?;
        tracing::debug!("Downloaded input for request '{}'", request_digest);
        let decoded_env = boundless_market::input::GuestEnv::decode(&input)?.stdin;
        tracing::debug!("Decoded input for request '{}'", request_digest);
        Ok(Bytes::from(decoded_env))
    } else {
        let decoded_env = boundless_market::input::GuestEnv::decode(&decoded_input)?.stdin;
        tracing::debug!("Decoded input for request '{}'", request_digest);
        Ok(Bytes::from(decoded_env))
    }
}
