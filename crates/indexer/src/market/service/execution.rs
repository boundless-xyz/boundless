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

use crate::db::market::{CycleCountExecution, CycleCountExecutionUpdate};
use crate::db::{DbObj, IndexerDb};
use crate::market::service::IndexerServiceExecutionConfig;
use alloy::primitives::{B256, U256};
use anyhow::Result;
use bonsai_sdk::non_blocking::{Client as BonsaiClient, SessionId};
use boundless_market::storage::fetch_url;
use broker::futures_retry::retry;
use bytes::Bytes;
use std::collections::HashSet;

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
            "Current execution state: {} pending cycle counts, {} executing",
            pending_count,
            executing_count
        );

        // Find cycle counts still in state PENDING, up to the max we want to have executing at a time
        let mut digests_to_process = HashSet::new();

        let mut pending_to_process = 0;
        if executing_count < config.max_concurrent_executing {
            pending_to_process = config.max_concurrent_executing - executing_count;
        }
        if pending_to_process > 0 {
            tracing::debug!(
                "Querying DB for cycle counts in status PENDING (max {})...",
                pending_to_process
            );

            let pending_cycle_counts = match db.get_cycle_counts_pending(pending_to_process).await {
                Ok(requests) => requests,
                Err(e) => {
                    tracing::error!("Unable to get cycle counts in status PENDING: {}", e);
                    continue;
                }
            };
            digests_to_process.extend(pending_cycle_counts);
        }

        if !digests_to_process.is_empty() {
            tracing::debug!("Pending requests to process:");
            tracing::debug!(?digests_to_process);
        } else {
            tracing::info!("No pending requests to process");
        }

        // Get the inputs and image data for the pending requests
        let digest_vec: Vec<B256> = digests_to_process.iter().copied().collect();
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
            // Obtain the request input from either the URL or the inline data
            let input: Bytes =
                match download_or_decode_input(&config, request_digest, &input_type, &input_data)
                    .await
                {
                    Ok(input) => input,
                    Err(e) => {
                        tracing::error!(
                            "Unable to download or decode {} input for request '{}': {}",
                            input_type,
                            request_digest,
                            e
                        );
                        failed_executions.push(request_digest);
                        continue;
                    }
                };

            // Upload the input via the bento API
            tracing::debug!("Uploading input for '{}'", request_digest);
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
                        "Uploaded input for '{}', obtained ID '{}'",
                        request_digest,
                        id
                    );
                    id
                }
                Err(e) => {
                    tracing::error!("Failed to upload input for '{}': {}", request_digest, e);
                    continue;
                }
            };

            // Check if the image exists for the request via the bento API
            tracing::debug!(
                "Checking if image '{}' exists for request '{}'",
                image_id,
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
                    tracing::debug!("Image exists: {}", exists);
                    exists
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to check if image exists for request '{}': {}",
                        request_digest,
                        e
                    );
                    continue;
                }
            };

            // If the image doesn't exist, download it from its URL and upload it via the bento API
            if !image_response {
                tracing::debug!(
                    "Downloading image for '{}' from URL '{}'",
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
                            "Downloaded image for request '{}' from URL '{}'",
                            request_digest,
                            image_url
                        );
                        bytes
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to download image for request '{}' from URL '{}': {}",
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
                            "Uploaded image for '{}' with ID '{}'",
                            request_digest,
                            image_id
                        );
                        result
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to upload image for '{}' with ID '{}': {}",
                            request_digest,
                            image_id,
                            e
                        );
                        continue;
                    }
                };
            }

            tracing::debug!(
                "Creating execution session for request '{}' with image ID '{}', input ID '{}'",
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
                        "Created session for '{}', obtained session ID '{}'",
                        request_digest,
                        id.uuid
                    );
                    id
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to create execution session for '{}': {}",
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
        tracing::debug!("Updating cycle counts with EXECUTING status...");
        db.set_cycle_counts_executing(&current_executing_requests).await.unwrap();

        // Monitor requests in status EXECUTING, i.e. the ones that just started executing
        // as well as any ones started earlier that haven't terminated yet
        tracing::debug!(
            "Querying DB for cycle counts in status EXECUTING (max {})...",
            config.max_status_queries
        );
        let executing_requests =
            db.get_cycle_counts_executing(config.max_status_queries).await.unwrap();
        if !executing_requests.is_empty() {
            tracing::debug!("Executing requests found:");
            tracing::debug!(?executing_requests);
        } else {
            tracing::info!("No executing requests found");
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
                    tracing::debug!(
                        "Retrieved status for request '{}', session '{}': {}",
                        execution_info.request_digest,
                        execution_info.session_uuid,
                        status.status
                    );
                    status
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to retrieve status for request '{}', session '{}': {}",
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
                                "Status for request '{}' is SUCCEEDED, but no stats are provided",
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
                        "Request '{}' with session '{}' is still running",
                        execution_info.request_digest,
                        execution_info.session_uuid
                    );
                }
                _ => {
                    failed_executions.push(execution_info.request_digest);
                }
            }
        }

        tracing::debug!("Updating cycle counts with COMPLETED status...");
        db.set_cycle_counts_completed(&completed_executions).await.unwrap();

        tracing::debug!("Updating cycle counts with FAILED status...");
        db.set_cycle_counts_failed(&failed_executions).await.unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_market::input::GuestEnv;
    use std::time::Duration;

    fn test_config() -> IndexerServiceExecutionConfig {
        IndexerServiceExecutionConfig {
            execution_interval: Duration::from_secs(60),
            bento_api_key: None,
            bento_api_url: None,
            bento_retry_count: 3,
            bento_retry_sleep_ms: 100,
            max_concurrent_executing: 5,
            max_status_queries: 20,
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
            request_digest,
            &"Url".to_string(),
            &hex_url,
        )
        .await;

        assert!(result.is_err());
    }
}
