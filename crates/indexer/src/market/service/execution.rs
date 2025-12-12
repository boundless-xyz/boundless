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
use crate::IndexerServiceConfig;
use alloy::primitives::B256;
use alloy::transports::http::reqwest;
use alloy::transports::http::reqwest::StatusCode;
use anyhow::{anyhow, Result};
use bytes::Bytes;
use std::collections::HashSet;
use url::Url;

#[derive(serde::Deserialize)]
struct ImageExistsResponse {
    exists: bool,
    url: String,
}

#[derive(serde::Deserialize)]
struct BentoInputUploadResponse {
    url: String,
    uuid: String,
}

#[derive(serde::Deserialize)]
struct BentoImageUploadResponse {
    url: String,
}

#[derive(serde::Serialize)]
struct BentoCreateSessionRequest {
    img: String,
    input: String,
    assumptions: Vec<String>,
    execute_only: bool,
    cycle_limit: u64,
}

#[derive(serde::Deserialize)]
struct BentoCreateSessionResponse {
    uuid: String,
}

#[derive(serde::Deserialize)]
struct BentoSessionStatusStats {
    cycles: u32,
    _segments: u64,
    total_cycles: u32,
}

#[derive(serde::Deserialize)]
struct BentoSessionStatusResponse {
    status: String,
    _receipt_url: String,
    _error_msg: String,
    state: String,
    _elapsed_time: String,
    stats: BentoSessionStatusStats,
}

pub async fn execute_requests(db: DbObj, config: IndexerServiceConfig) {
    tracing::info!("Started execution task");
    let mut interval = tokio::time::interval(config.execution_interval);

    let http_client =
        reqwest::Client::builder().timeout(config.execution_http_client_timeout).build().unwrap();

    let mut current_executing_requests = HashSet::new();

    loop {
        interval.tick().await;

        // Find cycle counts still in state PENDING
        tracing::debug!("Querying DB for cycle counts in status PENDING...");
        let pending_requests = match db.get_cycle_counts_pending(10).await {
            Ok(requests) => requests,
            Err(e) => {
                tracing::error!("Unable to get cycle counts by status: {}", e);
                continue;
            }
        };

        if !pending_requests.is_empty() {
            tracing::debug!("Pending requests found:");
            tracing::debug!(?pending_requests);
        } else {
            tracing::info!("No pending requests found");
        }

        // Get the inputs and image data for the pending requests
        let digest_vec: Vec<B256> = pending_requests.iter().copied().collect();
        let request_inputs_and_images = match db.get_request_params_for_execution(&digest_vec).await
        {
            Ok(inputs_and_images) => inputs_and_images,
            Err(e) => {
                tracing::error!("Unable to get requests inputs and image: {}", e);
                continue;
            }
        };

        for (request_digest, input_type, input_data, image_id, image_url, max_price) in
            request_inputs_and_images.clone()
        {
            let input = match download_or_decode_input(
                http_client.clone(),
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
                        "Unable to download or decode {} input for request '{}': {}",
                        input_type,
                        request_digest,
                        e
                    );
                    continue;
                }
            };

            let image_response =
                match bento_image_exists(http_client.clone(), &config, request_digest, &image_id)
                    .await
                {
                    Ok(resp) => resp,
                    Err(e) => {
                        tracing::error!(
                            "Failed to check if image exists for request '{}': {}",
                            request_digest,
                            e
                        );
                        continue;
                    }
                };

            // If the image doesn't exist, download it from its URL and upload it via the executor API
            if !image_response.exists {
                let image =
                    match download_file(http_client.clone(), &config, request_digest, &image_url)
                        .await
                    {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            tracing::error!(
                                "Failed to download image for request '{}' from URL '{}': {}",
                                request_digest,
                                image_url,
                                e
                            );
                            continue;
                        }
                    };

                match bento_upload_image(
                    http_client.clone(),
                    &config,
                    request_digest,
                    image,
                    &image_response.url,
                )
                .await
                {
                    Ok(_) => {}
                    Err(e) => {
                        tracing::error!(
                            "Failed to upload image for request '{}': {}",
                            request_digest,
                            e
                        );
                        continue;
                    }
                }
            }

            // Request URL and UUID for input upload
            let input_params =
                match bento_get_input_params(http_client.clone(), &config, request_digest).await {
                    Ok(input_params) => input_params,
                    Err(e) => {
                        tracing::error!(
                            "Failed to get input params for request '{}': {}",
                            request_digest,
                            e
                        );
                        continue;
                    }
                };

            tracing::debug!(
                "Input params: UUID '{}', URL '{}'",
                input_params.uuid,
                input_params.url
            );

            // Upload the request inputs
            match bento_upload_input(
                http_client.clone(),
                &config,
                request_digest,
                input,
                &input_params.url,
            )
            .await
            {
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(
                        "Failed to upload input for request '{}': {}",
                        request_digest,
                        e
                    );
                    continue;
                }
            }

            // Execute the request
            let execution_uuid = match bento_execute_request(
                http_client.clone(),
                &config,
                request_digest,
                &input_params.uuid,
                &image_id,
                max_price,
            )
            .await
            {
                Ok(execution_uuid) => execution_uuid,
                Err(e) => {
                    tracing::error!("Failed to execute request '{}': {}", request_digest, e);
                    continue;
                }
            };

            current_executing_requests
                .insert(CycleCountExecution { request_digest, session_uuid: execution_uuid });
        }

        // Update the cycle count status
        tracing::debug!("Updating cycle count status to EXECUTING...");
        db.set_cycle_counts_executing(&current_executing_requests).await.unwrap();

        // Monitor requests in status EXECUTING, i.e. the ones that just started executing
        // as well as any ones started earlier that haven't terminated yet
        tracing::debug!("Querying DB for cycle counts in status EXECUTING...");
        let executing_requests = db.get_cycle_counts_executing(30).await.unwrap();
        if !executing_requests.is_empty() {
            tracing::debug!("Executing requests found:");
            tracing::debug!(?executing_requests);
        } else {
            tracing::info!("No executing requests found");
        }

        current_executing_requests.extend(executing_requests);

        let mut completed_executions = Vec::new();

        // Update the cycle counts and status for execution requests that have completed
        for execution_info in current_executing_requests.clone() {
            let execution_status = match bento_get_execution_status(
                http_client.clone(),
                &config,
                execution_info.request_digest,
                &execution_info.session_uuid,
            )
            .await
            {
                Ok(status) => status,
                Err(e) => {
                    tracing::error!(
                        "Failed to query status for execution request '{}': {}",
                        execution_info.request_digest,
                        e
                    );
                    continue;
                }
            };

            if execution_status.state == "SUCCEEDED" {
                completed_executions.push(CycleCountExecutionUpdate {
                    request_digest: execution_info.request_digest,
                    program_cycles: execution_status.stats.cycles,
                    total_cycles: execution_status.stats.total_cycles,
                })
            }
        }

        tracing::debug!("Updating cycle counts with completed stats...");
        db.set_cycle_counts_completed(&completed_executions).await.unwrap();
    }
}

async fn download_or_decode_input(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    input_type: &String,
    input_data: &String,
) -> Result<Bytes> {
    tracing::debug!(
        "Download or decode input for '{}', input type: '{}'",
        request_digest,
        input_type
    );

    let input_data_hex = input_data.trim_start_matches("0x");
    let decoded_input = String::from_utf8(hex::decode(input_data_hex)?)?;

    if input_type == "Url" {
        let input = download_file(http_client, config, request_digest, &decoded_input).await?;
        tracing::debug!("Downloaded input for request '{}'", request_digest);
        Ok(input)
    } else {
        let decoded_env =
            boundless_market::input::GuestEnv::decode(&decoded_input.as_bytes())?.stdin;
        tracing::debug!("Decoded input for request '{}'", request_digest);
        Ok(Bytes::from(decoded_env))
    }
}

async fn download_file(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    url: &String,
) -> Result<Bytes> {
    tracing::debug!("Download file for request '{}' from URL '{}'", request_digest, url);

    // Validate the URL
    let valid_url = Url::parse(&url)?;

    // Validate the file size
    let response = http_client.head(valid_url.clone()).send().await?;
    let content_length = match response.headers().get("Content-Length") {
        Some(content_length) => content_length.to_str()?,
        None => {
            return Err(anyhow!(
                "Unable to determine input content length for request '{}', URL '{}'",
                request_digest,
                valid_url
            ))
        }
    };
    let parsed_length = content_length.parse::<u32>()?;
    if parsed_length > config.max_execution_data_size * 1024 * 1024 {
        return Err(anyhow!(
            "Input size '{}' too large for request '{}'",
            parsed_length,
            request_digest
        ));
    }

    // Download the request inputs
    Ok(http_client.get(valid_url).send().await?.bytes().await?)
}

/// Queries the Bento API to check if an image by the passed ID exists
/// If so, returns (true, ""), else (false, <image_url>) with the image_url returned by the API
async fn bento_image_exists(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    image_id: &String,
) -> Result<ImageExistsResponse> {
    tracing::debug!("Checking if image '{}' exists for request '{}'", image_id, request_digest);

    let normalized_id = match image_id.strip_prefix("0x") {
        Some(id) => id,
        None => &image_id,
    };

    let api_url = Url::parse(config.bento_api_url.as_str())?
        .join(&format!("images/upload/{}", normalized_id))?;

    let response = http_client
        .get(api_url)
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .send()
        .await?;

    match response.status() {
        StatusCode::NO_CONTENT => {
            tracing::debug!("Image '{}' exists for request '{}'", normalized_id, request_digest);
            Ok(ImageExistsResponse { exists: true, url: String::from("") })
        }
        StatusCode::OK => {
            let json_response = response.json::<BentoImageUploadResponse>().await?;
            tracing::debug!(
                "Image '{}' does not exist for request '{}', returned URL is '{}'",
                normalized_id,
                request_digest,
                json_response.url
            );
            Ok(ImageExistsResponse { exists: false, url: json_response.url })
        }
        _ => Err(anyhow!(
            "Unable to check if image '{}' exists for request '{}': status code {}, response '{}'",
            normalized_id,
            request_digest,
            response.status().as_str(),
            response.text().await?
        )),
    }
}

async fn bento_upload_image(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    image_data: Bytes,
    image_url: &String,
) -> Result<()> {
    if image_data.is_empty() {
        return Err(anyhow!("No input data to upload for request '{}'", request_digest));
    }

    // Validate the URL
    let valid_url = Url::parse(&image_url)?;

    let response = http_client
        .put(valid_url.clone())
        .header("Content-Type", "application/octet-stream")
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .body(image_data)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Unable to upload image for request '{}' to URL '{}': status code {}, response '{}'",
            request_digest,
            valid_url,
            response.status().as_str(),
            response.text().await?
        ));
    };

    tracing::debug!(
        "Image successfully uploaded to '{}' for request '{}'",
        valid_url,
        request_digest
    );
    Ok(())
}

async fn bento_get_input_params(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
) -> Result<BentoInputUploadResponse> {
    let api_url = Url::parse(config.bento_api_url.as_str())?.join("inputs/upload")?;

    let response = http_client
        .get(api_url)
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .send()
        .await?;

    if response.status().is_success() {
        tracing::debug!("Successfully queried input params for request '{}'", request_digest);
        Ok(response.json::<BentoInputUploadResponse>().await?)
    } else {
        Err(anyhow!(
            "Unable to get input URL and UUID for request '{}': status code {}, response '{}'",
            request_digest,
            response.status().as_str(),
            response.text().await?
        ))
    }
}

async fn bento_upload_input(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    input_data: Bytes,
    input_url: &String,
) -> Result<()> {
    if input_data.is_empty() {
        return Err(anyhow!("No input data to upload for request '{}'", request_digest));
    };

    // Validate the URL
    let valid_url = Url::parse(&input_url)?;

    let response = http_client
        .put(valid_url.clone())
        .header("Content-Type", "application/octet-stream")
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .body(input_data)
        .send()
        .await?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Unable to upload image for request '{}' to URL '{}': status code {}, response '{}'",
            request_digest,
            valid_url,
            response.status().as_str(),
            response.text().await?
        ));
    };

    tracing::debug!(
        "Input successfully uploaded to '{}' for request '{}'",
        valid_url,
        request_digest
    );
    Ok(())
}

async fn bento_execute_request(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    input_uuid: &String,
    image_id: &String,
    max_price: String,
) -> Result<String> {
    let max_price_parsed = max_price.parse::<u64>()?;
    let exec_cycle_limit = max_price_parsed / 100000000;

    let normalized_id = match image_id.strip_prefix("0x") {
        Some(id) => id,
        None => &image_id,
    };

    let api_url = Url::parse(config.bento_api_url.as_str())?.join("sessions/create")?;

    let body = BentoCreateSessionRequest {
        img: String::from(normalized_id),
        input: input_uuid.clone(),
        assumptions: Vec::new(),
        execute_only: true,
        cycle_limit: exec_cycle_limit,
    };

    tracing::debug!(
        "Starting execution with image ID '{}', input ID '{}', cycle limit '{}' for request '{}'",
        normalized_id,
        &input_uuid,
        &exec_cycle_limit,
        request_digest
    );

    let response = http_client
        .post(api_url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .json(&body)
        .send()
        .await?;

    if !response.status().is_success() {
        Err(anyhow!(
            "Unable to start execution for request '{}', status code {}, response: '{}'",
            request_digest,
            response.status().as_str(),
            response.text().await?
        ))
    } else {
        let json_response = response.json::<BentoCreateSessionResponse>().await?;
        tracing::debug!(
            "Successfully created session for request '{}', UUID: '{}'",
            request_digest,
            json_response.uuid
        );
        Ok(json_response.uuid)
    }
}

async fn bento_get_execution_status(
    http_client: reqwest::Client,
    config: &IndexerServiceConfig,
    request_digest: B256,
    session_uuid: &String,
) -> Result<BentoSessionStatusResponse> {
    tracing::debug!(
        "Querying execution status for ID '{}' for request '{}'",
        session_uuid,
        request_digest
    );

    let api_url = Url::parse(config.bento_api_url.as_str())?
        .join(&format!("sessions/status/{}", session_uuid))?;

    let response = http_client
        .get(api_url)
        .header("Accept", "application/json")
        .header("x-api-key", config.bento_api_key.clone())
        .send()
        .await?;

    if !response.status().is_success() {
        Err(anyhow!(
            "Unable to query status for execution for request '{}': status code {}, response '{}'",
            request_digest,
            response.status().as_str(),
            response.text().await?
        ))
    } else {
        let json_response = response.json::<BentoSessionStatusResponse>().await?;

        tracing::debug!(
            "Successfully queried session for request '{}', UUID: '{}', status '{}', cycles {}, total_cycles {}",
            request_digest,
            session_uuid,
            json_response.status,
            json_response.stats.cycles,
            json_response.stats.total_cycles
        );

        Ok(json_response)
    }
}
