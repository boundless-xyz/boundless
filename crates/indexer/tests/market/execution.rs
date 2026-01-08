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

//! End-to-end tests for the execute_requests function

use std::time::Duration;

use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use boundless_indexer::db::DbObj;
use boundless_indexer::{
    db::IndexerDb,
    market::service::{execute_requests, IndexerServiceExecutionConfig},
    test_utils::TestDb,
};
use boundless_market::contracts::{
    Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
};
use boundless_test_utils::guests::{ECHO_ID, ECHO_PATH};
use sqlx::Row;
use tokio::task::JoinHandle;
use wiremock::{
    matchers::{method, path, path_regex},
    Mock, MockServer, ResponseTemplate,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const TEST_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Helper to create a test config for testing
fn test_config(uri: String) -> IndexerServiceExecutionConfig {
    IndexerServiceExecutionConfig {
        execution_interval: Duration::from_millis(100),
        bento_api_url: uri,
        bento_api_key: "test-api-key".to_string(),
        bento_retry_count: 1,
        bento_retry_sleep_ms: 10,
        max_concurrent_executing: 10,
        max_status_queries: 10,
    }
}

/// Helper to generate a proof request for testing
fn generate_request(id: u32, client: &Address) -> ProofRequest {
    ProofRequest::new(
        RequestId::new(*client, id),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1_000_000_000_000u64), // Large enough for cycle count limit
            rampUpStart: 0,
            timeout: 3600,
            rampUpPeriod: 1,
            lockTimeout: 3600,
            lockCollateral: U256::from(0),
        },
    )
}

/// A struct to hold configuration for Bento API mocks
struct BentoMockConfig {
    input_uuid: String,
    session_uuid: String,
    expected_cycles: u64,
    expected_total_cycles: u64,
    image_exists: bool,
    fail_execution: bool,
}

impl Default for BentoMockConfig {
    fn default() -> BentoMockConfig {
        BentoMockConfig {
            input_uuid: "test-input-uuid".to_string(),
            session_uuid: "test-session-uuid".to_string(),
            expected_cycles: 0,
            expected_total_cycles: 0,
            image_exists: true,
            fail_execution: false,
        }
    }
}

/// Setup mock responses for the Bento API
async fn setup_bento_mocks(mock_server: &MockServer, config: BentoMockConfig) {
    let mock_url = mock_server.uri();

    // Mock GET /inputs/upload - return URL and uuid
    Mock::given(method("GET"))
        .and(path("/inputs/upload"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "url": format!("{}/put-input", mock_url),
            "uuid": config.input_uuid
        })))
        .mount(mock_server)
        .await;

    // Mock PUT to URL for input upload
    Mock::given(method("PUT"))
        .and(path("/put-input"))
        .respond_with(ResponseTemplate::new(200))
        .mount(mock_server)
        .await;

    // Mock GET /images/upload/{image_id} - return 204 to indicate image exists, 200 with JSON body to indicate it doesn't
    Mock::given(method("GET"))
        .and(path_regex(r"/images/upload/.+"))
        .respond_with(if config.image_exists {
            ResponseTemplate::new(204)
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": format!("{}/images/upload/test-image", mock_url)
            }))
        })
        .mount(mock_server)
        .await;

    // Mock POST /sessions/create - return session uuid
    Mock::given(method("POST"))
        .and(path("/sessions/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "uuid": config.session_uuid
        })))
        .mount(mock_server)
        .await;

    // Mock GET /sessions/status/{uuid} - return status with stats
    Mock::given(method("GET"))
        .and(path_regex(r"/sessions/status/.+"))
        .respond_with(ResponseTemplate::new(200).set_body_json(if config.fail_execution {
            serde_json::json!({
                "status": "FAILED",
                "state": null,
                "error_msg": "Execution failed",
                "elapsed_time": null,
                "stats": null
            })
        } else {
            serde_json::json!({
                "status": "SUCCEEDED",
                "state": null,
                "error_msg": null,
                "elapsed_time": null,
                "stats": {
                    "cycles": config.expected_cycles,
                    "total_cycles": config.expected_total_cycles,
                    "user_cycles": config.expected_cycles,
                    "segments": 1
                }
            })
        }))
        .mount(mock_server)
        .await;
}

/// Set up a test fixture with test DB, mock Bento server, and a number of proof requests for testing
async fn setup_test_fixture(
    bento_mock_config: Option<BentoMockConfig>,
    num_requests: u32,
) -> (TestDb, Vec<FixedBytes<32>>, MockServer) {
    // Set up test database
    let test_db = TestDb::new().await.unwrap();

    // Set up mock Bento API server
    let mock_server = MockServer::start().await;
    // Only set up the Bento API mock if there's a corresponding config. Some tests don't need it
    match bento_mock_config {
        Some(bento_mock_config) => {
            setup_bento_mocks(&mock_server, bento_mock_config).await;
        }
        None => {}
    }

    // Create test request with PENDING cycle count
    let mut requests = vec![];
    let mut digests = vec![];
    let mut statuses = vec![];
    for i in 0..num_requests {
        requests.push(generate_request(i, &Address::ZERO));
        digests.push(B256::from([i as u8; 32]));
        statuses.push("PENDING");
    }
    test_db.setup_requests_and_cycles(&digests, &requests, &statuses).await;

    // Verify initial state
    let pending_count = test_db.db.get_cycle_counts_pending(10).await.unwrap().len() as u32;
    assert_eq!(
        pending_count, num_requests,
        "Expected {} pending cycle counts initially",
        num_requests
    );

    (test_db, digests, mock_server)
}

/// Set up a tokio task to test the execute_requests function
async fn setup_test_task(db: DbObj, server_uri: String) -> JoinHandle<()> {
    let config = test_config(server_uri);
    let execution_handle = tokio::spawn(async move {
        execute_requests(db, config).await;
    });

    execution_handle
}

/// Helper to wait for failed request statuses. Stops on the expected condition, or panics after a max time
async fn wait_for_failed_status(db: DbObj) {
    let start = std::time::Instant::now();

    loop {
        let (pending, executing, failed_count) = db.count_cycle_counts_by_status().await.unwrap();

        if failed_count >= 1 {
            tracing::info!("Cycle count marked as failed");
            break;
        }

        if start.elapsed() > TEST_TIMEOUT {
            panic!(
                "Timeout waiting for cycle count to fail. Current state: pending={}, executing={}, failed={}",
                pending, executing, failed_count
            );
        }

        tokio::time::sleep(TEST_POLL_INTERVAL).await;
    }
}

/// Helper to check the status of multiple requests
async fn verify_request_status(test_db: TestDb, digests: &[B256], expected_status: String) {
    for digest in digests {
        let result = sqlx::query(
            "SELECT cycle_status
            FROM cycle_counts
            WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();

        let status: String = result.get("cycle_status");
        assert_eq!(status, expected_status, "Expected {} status", expected_status);
    }
}

/// Test the happy path of processing pending cycle counts
#[test_log::test(tokio::test)]
#[ignore = "Can be slow - run with --ignored"]
async fn test_execute_requests_processes_pending_cycle_counts() {
    let expected_cycles = 50_000_000u64;
    let expected_total_cycles = 51_000_000u64;

    let (test_db, _digests, mock_server) = setup_test_fixture(
        Some(BentoMockConfig { expected_cycles, expected_total_cycles, ..Default::default() }),
        2,
    )
    .await;

    // Start the test task
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri()).await;

    // Wait for cycle counts to transition to COMPLETED (with timeout)
    let start = std::time::Instant::now();

    loop {
        // Check if cycle counts have been completed
        let result = sqlx::query(
            "SELECT COUNT(*) as count
            FROM cycle_counts
            WHERE cycle_status = 'COMPLETED'",
        )
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
        let completed_count: i64 = result.get("count");

        if completed_count >= 2 {
            tracing::info!("All cycle counts completed");
            break;
        }

        if start.elapsed() > TEST_TIMEOUT {
            // Get current state for debugging
            let (pending, executing, failed) =
                test_db.db.count_cycle_counts_by_status().await.unwrap();

            panic!(
                "Timeout waiting for cycle counts to complete. Current state: pending={}, executing={}, failed={}, completed={}",
                pending, executing, failed, completed_count
            );
        }

        tokio::time::sleep(TEST_POLL_INTERVAL).await;
    }

    // Abort the background task
    execution_handle.abort();

    // Verify final state
    let results = sqlx::query(
        "SELECT request_digest, cycle_status, program_cycles, total_cycles
        FROM cycle_counts
        ORDER BY request_digest",
    )
    .fetch_all(&test_db.pool)
    .await
    .unwrap();

    assert_eq!(results.len(), 2, "Expected 2 cycle count records");

    for row in results {
        let status: String = row.get("cycle_status");
        assert_eq!(status, "COMPLETED", "Expected COMPLETED status");

        let program_cycles: String = row.get("program_cycles");
        assert_eq!(
            program_cycles,
            format!("{:078}", expected_cycles),
            "Expected program_cycles to be {}",
            expected_cycles
        );

        let total_cycles: String = row.get("total_cycles");
        assert_eq!(
            total_cycles,
            format!("{:078}", expected_total_cycles),
            "Expected total_cycles to be {}",
            expected_total_cycles
        );
    }
}

/// Test that invalid request params result in FAILED requests
#[test_log::test(tokio::test)]
#[ignore = "Can be slow - run with --ignored"]
async fn test_execute_requests_invalid_request_params() {
    // Create the test fixture with 3 requests, which we'll patch with invalid parameters for testing.
    // Any of input_type, input_data or image_id if empty should trigger a failed request
    let (test_db, digests, mock_server) = setup_test_fixture(None, 3).await;

    // Set empty input type for request 1
    sqlx::query(
        "UPDATE proof_requests
        SET input_type = ''
        WHERE request_digest = $1",
    )
    .bind(format!("{:x}", digests[0]))
    .execute(&test_db.pool)
    .await
    .unwrap();

    // Set empty input data for request 2
    sqlx::query(
        "UPDATE proof_requests
        SET input_data = ''
        WHERE request_digest = $1",
    )
    .bind(format!("{:x}", digests[1]))
    .execute(&test_db.pool)
    .await
    .unwrap();

    // Set empty image_id for request 3
    sqlx::query(
        "UPDATE proof_requests
        SET image_id = ''
        WHERE request_digest = $1",
    )
    .bind(format!("{:x}", digests[2]))
    .execute(&test_db.pool)
    .await
    .unwrap();

    // Start the test task
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri()).await;

    // Wait for cycle count to transition to FAILED
    wait_for_failed_status(test_db.db.clone()).await;

    // Abort the background task
    execution_handle.abort();

    // Verify final state
    verify_request_status(test_db, &digests, "FAILED".to_string()).await;
}

/// Test that a failed execution results in FAILED requests
#[test_log::test(tokio::test)]
#[ignore = "Can be slow - run with --ignored"]
async fn test_execute_requests_handles_failed_execution() {
    let (test_db, digests, mock_server) =
        setup_test_fixture(Some(BentoMockConfig { fail_execution: true, ..Default::default() }), 2)
            .await;

    // Start the test task
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri()).await;

    // Wait for cycle count to transition to FAILED
    wait_for_failed_status(test_db.db.clone()).await;

    // Abort the background task
    execution_handle.abort();

    // Verify final state
    verify_request_status(test_db, &digests, "FAILED".to_string()).await;
}

/// Test that an input decode error results in FAILED requests
#[test_log::test(tokio::test)]
#[ignore = "Can be slow - run with --ignored"]
async fn test_execute_requests_handles_input_decode_error() {
    // Create the test fixture with 1 request, no Bento mocks needed since we fail before API calls
    let (test_db, digests, mock_server) = setup_test_fixture(None, 1).await;

    // Patch input_data to invalid hex, causing download_or_decode_input to fail
    sqlx::query(
        "UPDATE proof_requests
        SET input_data = 'not-valid-hex'
        WHERE request_digest = $1",
    )
    .bind(format!("{:x}", digests[0]))
    .execute(&test_db.pool)
    .await
    .unwrap();

    // Start the test task
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri()).await;

    // Wait for cycle count to transition to FAILED
    wait_for_failed_status(test_db.db.clone()).await;

    // Abort the background task
    execution_handle.abort();

    // Verify final state
    verify_request_status(test_db, &digests, "FAILED".to_string()).await;
}

/// Test that an image download error results in FAILED requests
#[test_log::test(tokio::test)]
#[ignore = "Can be slow - run with --ignored"]
async fn test_execute_requests_handles_image_download_error() {
    // Create the test fixture with 1 request. The Bento API will return that the image doesn't exist,
    // forcing a download, which will fail, marking the request as FAILED
    let (test_db, digests, mock_server) =
        setup_test_fixture(Some(BentoMockConfig { image_exists: false, ..Default::default() }), 1)
            .await;

    // Patch image_url to a non-existent file, causing fetch_url to fail
    sqlx::query(
        "UPDATE proof_requests
        SET image_url = 'file:///nonexistent/path/to/image.elf'
        WHERE request_digest = $1",
    )
    .bind(format!("{:x}", digests[0]))
    .execute(&test_db.pool)
    .await
    .unwrap();

    // Start the test task
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri()).await;

    // Wait for cycle count to transition to FAILED
    wait_for_failed_status(test_db.db.clone()).await;

    // Abort the background task
    execution_handle.abort();

    // Verify final state
    verify_request_status(test_db, &digests, "FAILED".to_string()).await;
}
