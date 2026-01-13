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
use boundless_market::storage::fetch_url;
use broker::futures_retry::retry;
use bytes::Bytes;
use std::collections::{HashMap, HashSet};

/// Format an optional request ID for logging
fn fmt_request_id(id: Option<U256>) -> String {
    id.map(|i| format!("0x{:x}", i)).unwrap_or_else(|| "unknown".to_string())
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

    let mut interval = tokio::time::interval(config.execution_interval);

    let bento_client = BonsaiClient::from_parts(
        config.bento_api_url.clone(),
        config.bento_api_key.clone(),
        risc0_zkvm::VERSION,
    )
    .unwrap();

    let mut num_iterations: u32 = 1;

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
        let prev_executing_requests: HashSet<ExecutionWithId> =
            db.get_cycle_counts_executing(config.max_status_queries).await.unwrap();

        tracing::info!(
            "Current cycle counts execution state: {} pending, {} executing [{}]",
            pending_count,
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

        for (request_digest, input_type, input_data, image_id, image_url, max_price) in
            request_inputs_and_images.clone()
        {
            // Get request_id for logging
            let request_id = digest_to_request_id.get(&request_digest).copied().flatten();

            // Validate required fields are not empty
            if image_id.is_empty() || input_type.is_empty() || input_data.is_empty() {
                tracing::error!(
                    "Cycle count request id={}, digest={:x} has empty required fields: image_id={}, input_type={}, input_data={}",
                    fmt_request_id(request_id),
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
                request_id,
                request_digest,
                &input_type,
                &input_data,
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
                    failed_executions.push(request_digest);
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
                    continue;
                }
            };

            if input_uuid.is_empty() {
                tracing::error!("Empty input UUID received after input upload for cycle count computation request id={}, digest={:x}",
                    fmt_request_id(request_id),
                    request_digest
                );
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
                    continue;
                }
            };

            // If the image doesn't exist, download it from its URL and upload it via the bento API
            if !image_response {
                tracing::trace!(
                    "Downloading image for cycle count computation request id={}, digest={:x} from URL '{}'",
                    fmt_request_id(request_id),
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
                            "Downloaded image for cycle count computation request id={}, digest={:x} from URL '{}'",
                            fmt_request_id(request_id),
                            request_digest,
                            image_url
                        );
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
                    continue;
                }
            };

            if execution_uuid.uuid.is_empty() {
                tracing::error!("Empty session UUID received after creating execution session for cycle count computation request id={}, digest={:x}",
                    fmt_request_id(request_id),
                    request_digest
                );
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
                        "Cycle count status for request id={}, digest={:x} is not SUCCEEDED or RUNNING: {}. Marking as FAILED.",
                        fmt_request_id(execution_info.request_id),
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

        if !failed_executions.is_empty() {
            let requests_info = failed_executions
                .iter()
                .map(|c| {
                    // Check executing requests first, then fall back to newly submitted requests
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

async fn download_or_decode_input(
    config: &IndexerServiceExecutionConfig,
    request_id: Option<U256>,
    request_digest: B256,
    input_type: &String,
    input_data: &str,
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
        let input = retry(
            config.bento_retry_count,
            config.bento_retry_sleep_ms,
            || async { fetch_url(&decoded_url).await },
            "fetch_url",
        )
        .await?;
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
        }
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            "0xGGGG",
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            "0x",
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Inline".to_string(),
            &hex_input,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Url".to_string(),
            &hex_url,
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
            &config,
            Some(U256::from(1)),
            request_digest,
            &"Unsupported".to_string(),
            "0x",
        )
        .await;

        assert!(result.is_err());
    }
}
