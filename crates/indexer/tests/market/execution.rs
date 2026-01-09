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

/// Extract the database connection string from a sqlx::test PgPool.
/// sqlx::test creates an isolated database per test with a unique name.
async fn get_db_url_from_pool(pool: &sqlx::PgPool) -> String {
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

struct BentoMockConfig {
    input_uuid: String,
    session_uuid: String,
    expected_cycles: u64,
    expected_total_cycles: u64,
    fail_execution: bool,
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
        .expect(1..)
        .mount(mock_server)
        .await;

    // Mock PUT to URL for input upload
    Mock::given(method("PUT"))
        .and(path("/put-input"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1..)
        .mount(mock_server)
        .await;

    // Mock GET /images/upload/{image_id} - return 204 to indicate image exists
    Mock::given(method("GET"))
        .and(path_regex(r"/images/upload/.+"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1..)
        .mount(mock_server)
        .await;

    // Mock POST /sessions/create - return session uuid
    Mock::given(method("POST"))
        .and(path("/sessions/create"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "uuid": config.session_uuid
        })))
        .expect(1..)
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
        .expect(1..)
        .mount(mock_server)
        .await;
}

async fn setup_test_fixture(
    pool: sqlx::PgPool,
    bento_mock_config: BentoMockConfig,
) -> (TestDb, JoinHandle<()>, Vec<FixedBytes<32>>, MockServer) {
    // Set up test database
    let db_url = get_db_url_from_pool(&pool).await;
    let test_db = TestDb::from_pool(db_url, pool).await.unwrap();

    // Set up mock Bento API server
    let mock_server = MockServer::start().await;
    setup_bento_mocks(&mock_server, bento_mock_config).await;

    // Create test request with PENDING cycle count
    let requests = vec![generate_request(1, &Address::ZERO), generate_request(2, &Address::ZERO)];
    let digests = vec![B256::from([1; 32]), B256::from([2; 32])];
    test_db.setup_requests_and_cycles(&digests, &requests, &["PENDING", "PENDING"]).await;

    // Verify initial state
    let pending_count = test_db.db.get_cycle_counts_pending(10).await.unwrap().len();
    assert_eq!(pending_count, 2, "Expected 2 pending cycle counts initially");

    // Create execution config
    let config = test_config(mock_server.uri());

    // Spawn execute_requests as a background task
    let db_clone = test_db.db.clone();
    let execution_handle = tokio::spawn(async move {
        execute_requests(db_clone, config).await;
    });

    // Return mock_server to keep it alive for the duration of the test
    (test_db, execution_handle, digests, mock_server)
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Slow - run with --ignored"]
async fn test_execute_requests_processes_pending_cycle_counts(pool: sqlx::PgPool) {
    let expected_cycles = 50_000_000u64;
    let expected_total_cycles = 51_000_000u64;

    let (test_db, execution_handle, _digests, _mock_server) = setup_test_fixture(
        pool,
        BentoMockConfig {
            input_uuid: "test-input-uuid".to_string(),
            session_uuid: "test-session-uuid".to_string(),
            expected_cycles,
            expected_total_cycles,
            fail_execution: false,
        },
    )
    .await;

    // Wait for cycle counts to transition to COMPLETED (with timeout)
    let timeout = Duration::from_secs(10);
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);

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

        if start.elapsed() > timeout {
            // Get current state for debugging
            let (pending, executing, failed) =
                test_db.db.count_cycle_counts_by_status().await.unwrap();

            panic!(
                "Timeout waiting for cycle counts to complete. Current state: pending={}, executing={}, failed={}, completed={}",
                pending, executing, failed, completed_count
            );
        }

        tokio::time::sleep(poll_interval).await;
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

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Slow - run with --ignored"]
async fn test_execute_requests_handles_failed_execution(pool: sqlx::PgPool) {
    let (test_db, execution_handle, digests, _mock_server) = setup_test_fixture(
        pool,
        BentoMockConfig {
            input_uuid: "test-input-uuid".to_string(),
            session_uuid: "test-session-uuid".to_string(),
            expected_cycles: 0,
            expected_total_cycles: 0,
            fail_execution: true,
        },
    )
    .await;

    // Wait for cycle count to transition to FAILED
    let timeout = Duration::from_secs(10);
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(100);

    loop {
        // Get current state
        let (pending, executing, failed_count) =
            test_db.db.count_cycle_counts_by_status().await.unwrap();

        if failed_count >= 1 {
            tracing::info!("Cycle count marked as failed");
            break;
        }

        if start.elapsed() > timeout {
            panic!(
                "Timeout waiting for cycle count to fail. Current state: pending={}, executing={}, failed={}",
                pending, executing, failed_count
            );
        }

        tokio::time::sleep(poll_interval).await;
    }

    // Abort the background task
    execution_handle.abort();

    // Verify final state
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
        assert_eq!(status, "FAILED", "Expected FAILED status");
    }
}
