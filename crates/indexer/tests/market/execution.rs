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

//! End-to-end tests for the `execute_requests` function.
//!
//! These tests verify the cycle count execution flow by mocking the Bento API
//! and observing how requests transition through states (PENDING → EXECUTING → COMPLETED/FAILED).
//!
//! # Test Strategy
//!
//! - **Happy path tests**: Verify successful execution flow with valid inputs
//! - **Error handling tests**: Verify correct state transitions when API calls fail
//! - **Edge case tests**: Verify behavior with malformed responses (empty UUIDs, missing stats)
//!
//! # Mock Configuration
//!
//! Tests use [`BentoMockConfig`] to control mock behavior. The mock server simulates:
//! - Input upload (`GET /inputs/upload`, `PUT /put-input`)
//! - Image existence check (`GET /images/upload/{id}`)
//! - Session creation (`POST /sessions/create`)
//! - Status polling (`GET /sessions/status/{uuid}`)

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
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
use sqlx::{PgPool, Row};
use tokio::task::JoinHandle;
use uuid::Uuid;
use wiremock::{
    matchers::{method, path, path_regex},
    Mock, MockServer, Request, Respond, ResponseTemplate,
};

/// Extract the database connection string from a sqlx::test PgPool.
/// sqlx::test creates an isolated database per test with a unique name.
async fn get_db_url_from_pool(pool: &PgPool) -> String {
    let base_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
    let db_name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(pool)
        .await
        .expect("failed to query current_database()");

    if let Some(last_slash) = base_url.rfind('/') {
        format!("{}/{}", &base_url[..last_slash], db_name)
    } else {
        format!("{}/{}", base_url, db_name)
    }
}

/// Creates an [`IndexerServiceExecutionConfig`] with test-appropriate defaults.
fn test_config(uri: String, max_iterations: u32) -> IndexerServiceExecutionConfig {
    IndexerServiceExecutionConfig {
        execution_interval: Duration::from_millis(100),
        bento_api_url: uri,
        bento_api_key: "test-api-key".to_string(),
        bento_retry_count: 1,
        bento_retry_sleep_ms: 10,
        max_concurrent_executing: 10,
        max_status_queries: 10,
        max_iterations,
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

/// Configuration for Bento API mocks.
///
/// This struct controls how the mock Bento API responds to requests during testing.
/// By default, all operations succeed and return valid responses. Set individual
/// fields to simulate various failure scenarios or edge cases.
struct BentoMockConfig {
    /// Cycle count to return in successful status responses
    expected_cycles: u64,
    /// Total cycle count to return in successful status responses
    expected_total_cycles: u64,
    /// If true, image exists check returns 204 (exists); if false, returns 200 (doesn't exist, triggers download)
    image_exists: bool,
    /// If true, input upload returns empty UUID (simulates malformed response)
    no_input_uuid: bool,
    /// If true, session creation returns empty UUID (simulates malformed response)
    no_session_uuid: bool,
    /// If true, successful execution returns no stats (simulates malformed response)
    no_status_stats: bool,
    /// If true, input upload endpoint returns 500 error
    fail_input_upload: bool,
    /// If true, image exists check endpoint returns 500 error
    fail_image_check: bool,
    /// If true, session creation endpoint returns 500 error
    fail_create_session: bool,
    /// If true, status check endpoint returns 500 error
    fail_status: bool,
    /// Final execution status to return via status API ("SUCCEEDED", "FAILED", or "RUNNING")
    execution_status: String,
    /// Number of times to return "RUNNING" before returning the final execution_status
    running_responses: usize,
}

impl Default for BentoMockConfig {
    fn default() -> BentoMockConfig {
        BentoMockConfig {
            expected_cycles: 0,
            expected_total_cycles: 0,
            image_exists: true,
            no_input_uuid: false,
            no_session_uuid: false,
            no_status_stats: false,
            fail_input_upload: false,
            fail_image_check: false,
            fail_create_session: false,
            fail_status: false,
            execution_status: "SUCCEEDED".to_string(),
            running_responses: 0,
        }
    }
}

/// Custom responder for session status endpoint.
///
/// Returns "RUNNING" for a configurable number of calls, then returns the final status.
/// This simulates the real Bento API behavior where executions take time to complete.
struct SessionStatusResponder {
    /// Tracks how many times this responder has been called
    call_count: Arc<AtomicUsize>,
    /// Number of "RUNNING" responses before returning final_status
    running_responses: usize,
    /// Status to return after running_responses calls ("SUCCEEDED" or "FAILED")
    final_status: String,
    /// Cycle count included in successful responses
    expected_cycles: u64,
    /// Total cycle count included in successful responses
    expected_total_cycles: u64,
    /// If true, omit stats from successful responses
    no_status_stats: bool,
    /// If true, return 500 error instead of status
    fail_status: bool,
}

impl Respond for SessionStatusResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);

        // Return failure if requested
        if self.fail_status {
            return ResponseTemplate::new(500);
        }

        let status = if count < self.running_responses { "RUNNING" } else { &self.final_status };

        let body = if status == "FAILED" {
            serde_json::json!({
                "status": "FAILED",
                "state": null,
                "error_msg": "Execution failed",
                "elapsed_time": null,
                "stats": null
            })
        } else if status == "SUCCEEDED" {
            if self.no_status_stats {
                serde_json::json!({
                    "status": "SUCCEEDED",
                    "state": null,
                    "error_msg": null,
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
                        "cycles": self.expected_cycles,
                        "total_cycles": self.expected_total_cycles,
                        "user_cycles": self.expected_cycles,
                        "segments": 1
                    }
                })
            }
        } else {
            serde_json::json!({
                "status": "RUNNING",
                "state": null,
                "error_msg": null,
                "elapsed_time": null,
                "stats": null
            })
        };

        ResponseTemplate::new(200).set_body_json(body)
    }
}

/// Setup mock responses for the Bento API
async fn setup_bento_mocks(mock_server: &MockServer, config: BentoMockConfig) {
    let mock_url = mock_server.uri();

    // Mock GET /inputs/upload - return URL and uuid (or no uuid if so instructed)
    Mock::given(method("GET"))
        .and(path("/inputs/upload"))
        .respond_with(if config.fail_input_upload {
            ResponseTemplate::new(500)
        } else {
            ResponseTemplate::new(200).set_body_json(if config.no_input_uuid {
                serde_json::json!({
                "url": format!("{}/put-input", mock_url),
                "uuid": "",
                    })
            } else {
                serde_json::json!({
                    "url": format!("{}/put-input", mock_url),
                    "uuid": Uuid::new_v4().to_string(),
                })
            })
        })
        .mount(mock_server)
        .await;

    // Mock PUT to URL for input upload
    // This is generally called after the above GET as part of upload_input, so we shouldn't need
    // to mock it failing independently for purposes of our testing
    Mock::given(method("PUT"))
        .and(path("/put-input"))
        .respond_with(ResponseTemplate::new(200))
        .mount(mock_server)
        .await;

    // Mock GET /images/upload/{image_id}
    // return 204 to indicate image exists, 200 with JSON body to indicate it doesn't, 500 for failure
    Mock::given(method("GET"))
        .and(path_regex(r"/images/upload/.+"))
        .respond_with(if config.fail_image_check {
            ResponseTemplate::new(500)
        } else if config.image_exists {
            ResponseTemplate::new(204)
        } else {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "url": format!("{}/images/upload/test-image", mock_url)
            }))
        })
        .mount(mock_server)
        .await;

    // Mock POST /sessions/create
    // return session uuid (or no uuid if so instructed)
    Mock::given(method("POST"))
        .and(path("/sessions/create"))
        .respond_with(if config.fail_create_session {
            ResponseTemplate::new(500)
        } else {
            ResponseTemplate::new(200).set_body_json(if config.no_session_uuid {
                serde_json::json!({
                "uuid": "",
                })
            } else {
                serde_json::json!({
                    "uuid": Uuid::new_v4().to_string(),
                })
            })
        })
        .mount(mock_server)
        .await;

    // Mock GET /sessions/status/{uuid}
    // return status with stats
    // Uses a custom responder to return RUNNING for a configurable number of calls before the final status
    let status_responder = SessionStatusResponder {
        call_count: Arc::new(AtomicUsize::new(0)),
        running_responses: config.running_responses,
        final_status: config.execution_status.clone(),
        expected_cycles: config.expected_cycles,
        expected_total_cycles: config.expected_total_cycles,
        no_status_stats: config.no_status_stats,
        fail_status: config.fail_status,
    };

    Mock::given(method("GET"))
        .and(path_regex(r"/sessions/status/.+"))
        .respond_with(status_responder)
        .mount(mock_server)
        .await;
}

/// Set up a test fixture with test DB, mock Bento server, and a number of proof requests for testing
async fn setup_test_fixture(
    pool: PgPool,
    bento_mock_config: Option<BentoMockConfig>,
    num_requests: u32,
) -> (TestDb, Vec<FixedBytes<32>>, MockServer) {
    // Set up test database
    let db_url = get_db_url_from_pool(&pool).await;
    let test_db = TestDb::from_pool(db_url, pool).await.unwrap();

    // Set up mock Bento API server
    let mock_server = MockServer::start().await;
    // Only set up the Bento API mock if there's a corresponding config. Some tests don't need it
    if let Some(bento_mock_config) = bento_mock_config {
        setup_bento_mocks(&mock_server, bento_mock_config).await;
    }

    // Create test request with PENDING cycle count
    let mut requests = Vec::with_capacity(num_requests as usize);
    let mut digests = Vec::with_capacity(num_requests as usize);
    for i in 0..num_requests {
        requests.push(generate_request(i, &Address::ZERO));
        digests.push(B256::from([i as u8; 32]));
    }
    let statuses = vec!["PENDING"; num_requests as usize];
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

/// Spawns `execute_requests` in a background task and returns a handle to await completion.
async fn setup_test_task(db: DbObj, server_uri: String, max_iterations: u32) -> JoinHandle<()> {
    let config = test_config(server_uri, max_iterations);
    let execution_handle = tokio::spawn(async move {
        execute_requests(db, config).await;
    });

    execution_handle
}

/// Asserts that all requests have the expected cycle_status in the database.
async fn verify_request_status(test_db: &TestDb, digests: &[B256], expected_status: &str) {
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

/// Helper to run a single-iteration test and verify final status.
///
/// This is the common pattern for most error handling tests:
/// 1. Set up fixture with given config
/// 2. Run execute_requests for one iteration
/// 3. Verify all requests have the expected status
async fn run_single_iteration_test(
    pool: PgPool,
    config: Option<BentoMockConfig>,
    num_requests: u32,
    expected_status: &str,
) {
    let (test_db, digests, mock_server) = setup_test_fixture(pool, config, num_requests).await;
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri(), 1).await;
    execution_handle.await.unwrap();
    verify_request_status(&test_db, &digests, expected_status).await;
}

/// Helper to patch a field in proof_requests for a specific digest.
///
/// Used to simulate invalid or malformed request data in tests.
async fn patch_request_field(pool: &PgPool, digest: &B256, field: &str, value: &str) {
    sqlx::query(&format!("UPDATE proof_requests SET {field} = $1 WHERE request_digest = $2"))
        .bind(value)
        .bind(format!("{:x}", digest))
        .execute(pool)
        .await
        .unwrap();
}

// =============================================================================
// Happy Path Tests
// =============================================================================

/// Test the happy path of processing pending cycle counts. Over a number of iterations, this
/// test will verify that cycle count requests are correctly updated from PENDING to COMPLETED
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_processes_pending_cycle_counts(pool: PgPool) {
    let expected_cycles = 50_000_000u64;
    let expected_total_cycles = 51_000_000u64;

    let (test_db, _digests, mock_server) = setup_test_fixture(
        pool,
        Some(BentoMockConfig {
            running_responses: 3, // Return RUNNING 3 times, then SUCCEEDED
            execution_status: "SUCCEEDED".to_string(),
            expected_cycles,
            expected_total_cycles,
            ..Default::default()
        }),
        2,
    )
    .await;

    // Start the test task for 3 iterations
    // With 2 requests and running_responses set to 3, this means that the task will find the following:
    // - on iteration 1, both requests are RUNNING (EXECUTING in the DB)
    // - on iteration 2, the first request is RUNNING, the second is SUCCEEDED (COMPLETED in the DB)
    // - on iteration 3, the second request is also SUCCEEDED
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri(), 3).await;
    execution_handle.await.unwrap();

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

/// Test that still-running requests are correctly marked as EXECUTING
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_processes_executing_cycle_counts(pool: PgPool) {
    // With running_responses: 2, the mock returns RUNNING on the first call,
    // so after 1 iteration requests remain in EXECUTING state
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { running_responses: 2, ..Default::default() }),
        2,
        "EXECUTING",
    )
    .await;
}

// =============================================================================
// Error Handling Tests
// =============================================================================

/// Test that invalid request params result in FAILED requests
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_invalid_request_params(pool: PgPool) {
    // Create the test fixture with 3 requests, which we'll patch with invalid parameters for testing.
    // Any of input_type, input_data or image_id if empty should trigger a failed request.
    // No Bento mocks needed since we fail before API calls
    let (test_db, digests, mock_server) = setup_test_fixture(pool, None, 3).await;

    // Patch each request with a different invalid field
    patch_request_field(&test_db.pool, &digests[0], "input_type", "").await;
    patch_request_field(&test_db.pool, &digests[1], "input_data", "").await;
    patch_request_field(&test_db.pool, &digests[2], "image_id", "").await;

    // Start the test task for 1 iteration and wait for it to complete
    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri(), 1).await;
    execution_handle.await.unwrap();

    // Verify final state
    verify_request_status(&test_db, &digests, "FAILED").await;
}

/// Test that a failed execution results in FAILED requests
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_failed_execution(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { execution_status: "FAILED".to_string(), ..Default::default() }),
        2,
        "FAILED",
    )
    .await;
}

/// Test that an input decode error results in FAILED requests
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_input_decode_error(pool: PgPool) {
    // No Bento mocks needed since we fail before API calls
    let (test_db, digests, mock_server) = setup_test_fixture(pool, None, 1).await;

    // Patch input_data to invalid hex, causing download_or_decode_input to fail
    patch_request_field(&test_db.pool, &digests[0], "input_data", "not-valid-hex").await;

    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri(), 1).await;
    execution_handle.await.unwrap();

    verify_request_status(&test_db, &digests, "FAILED").await;
}

/// Test that an error when checking if an image exists leaves the requests in the PENDING state
/// (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_image_check_error(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { fail_image_check: true, ..Default::default() }),
        1,
        "PENDING",
    )
    .await;
}

/// Test that an image download error results in FAILED requests
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_image_download_error(pool: PgPool) {
    // The Bento API will return that the image doesn't exist, forcing a download
    let (test_db, digests, mock_server) = setup_test_fixture(
        pool,
        Some(BentoMockConfig { image_exists: false, ..Default::default() }),
        2,
    )
    .await;

    // Patch image_url to a non-existent file, causing fetch_url to fail
    patch_request_field(
        &test_db.pool,
        &digests[0],
        "image_url",
        "file:///nonexistent/path/to/image.elf",
    )
    .await;

    patch_request_field(
        &test_db.pool,
        &digests[1],
        "image_url",
        "file:///nonexistent/path/to/image.elf",
    )
    .await;

    let execution_handle = setup_test_task(test_db.db.clone(), mock_server.uri(), 1).await;
    execution_handle.await.unwrap();

    verify_request_status(&test_db, &digests, "FAILED").await;
}

/// Test that an input upload error leaves the requests PENDING (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_input_upload_error(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { fail_input_upload: true, ..Default::default() }),
        2,
        "PENDING",
    )
    .await;
}

/// Test that an error in a create session request leaves the requests PENDING (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_create_session_error(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { fail_create_session: true, ..Default::default() }),
        2,
        "PENDING",
    )
    .await;
}

/// Test that an error returned by status calls leaves the requests in the EXECUTING state
/// (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_status_error(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { fail_status: true, ..Default::default() }),
        2,
        "EXECUTING",
    )
    .await;
}

// =============================================================================
// Edge Case Tests
// =============================================================================

/// Test that a successful input upload that returns no UUID leaves the requests PENDING (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_input_upload_no_uuid(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { no_input_uuid: true, ..Default::default() }),
        2,
        "PENDING",
    )
    .await;
}

/// Test that a successful create session request that returns no UUID leaves the requests PENDING (to be retried on the next iteration)
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_create_session_no_uuid(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { no_session_uuid: true, ..Default::default() }),
        2,
        "PENDING",
    )
    .await;
}

/// Test that a successful execution that returns no stats marks the requests as FAILED
#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_execute_requests_handles_no_stats(pool: PgPool) {
    run_single_iteration_test(
        pool,
        Some(BentoMockConfig { no_status_stats: true, ..Default::default() }),
        2,
        "FAILED",
    )
    .await;
}
