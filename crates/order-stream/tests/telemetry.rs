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

use std::env;
use std::str::FromStr;
use std::time::Duration;

use alloy::primitives::Address;
use alloy::signers::local::PrivateKeySigner;
use boundless_market::order_stream_client::OrderStreamClient;
use boundless_market::telemetry::{
    BrokerHeartbeat, CommitmentOutcome, CompletionOutcome, EvalOutcome, RequestCompleted,
    RequestEvaluated, RequestHeartbeat,
};
use chrono::Utc;
use sqlx::postgres::PgPoolOptions;
use sqlx::Row;
use url::Url;

struct TestEnv {
    order_stream_url: Url,
    signer: PrivateKeySigner,
    market_address: Address,
    chain_id: u64,
    redshift_url: String,
}

fn load_test_env() -> Option<TestEnv> {
    let vars = ["ORDER_STREAM_URL", "PRIVATE_KEY", "BOUNDLESS_ADDRESS", "CHAIN_ID", "REDSHIFT_URL"];
    for var in vars {
        if env::var(var).unwrap_or_default().is_empty() {
            eprintln!("Skipping test: {var} not set");
            return None;
        }
    }
    Some(TestEnv {
        order_stream_url: Url::parse(&env::var("ORDER_STREAM_URL").unwrap())
            .expect("invalid ORDER_STREAM_URL"),
        signer: env::var("PRIVATE_KEY").unwrap().parse().expect("invalid PRIVATE_KEY"),
        market_address: env::var("BOUNDLESS_ADDRESS")
            .unwrap()
            .parse()
            .expect("invalid BOUNDLESS_ADDRESS"),
        chain_id: u64::from_str(&env::var("CHAIN_ID").unwrap()).expect("invalid CHAIN_ID"),
        redshift_url: env::var("REDSHIFT_URL").unwrap(),
    })
}

fn run_id() -> String {
    use rand::Rng;
    let bytes: [u8; 16] = rand::rng().random();
    hex::encode(bytes)
}

#[tokio::test]
async fn telemetry_end_to_end() {
    let Some(env) = load_test_env() else {
        return;
    };

    let broker_address = env.signer.address();
    let run_id = run_id();
    eprintln!("Test run_id: {run_id}");
    eprintln!("Broker address: {broker_address}");

    let client = OrderStreamClient::new(env.order_stream_url, env.market_address, env.chain_id);

    // Send BrokerHeartbeat
    let heartbeat = BrokerHeartbeat {
        broker_address,
        config: serde_json::json!({"test_run_id": run_id}),
        committed_orders_count: 3,
        pending_preflight_count: 7,
        version: format!("integration-test-{}", Utc::now().format("%Y%m%d-%H%M%S")),
        uptime_secs: 42,
        events_dropped: 0,
        timestamp: Utc::now(),
    };
    client.submit_heartbeat(&heartbeat, &env.signer).await.expect("submit_heartbeat failed");
    eprintln!("BrokerHeartbeat sent successfully");

    // Send RequestHeartbeat with one evaluation and one completion
    let eval_request_id = format!("0x{run_id}e");
    let comp_request_id = format!("0x{run_id}c");

    let request_heartbeat = RequestHeartbeat {
        broker_address,
        evaluated: vec![RequestEvaluated {
            broker_address,
            order_id: format!("{eval_request_id}-0x00-LockAndFulfill"),
            request_id: eval_request_id.clone(),
            request_digest: "0x00".to_string(),
            requestor: Address::ZERO,
            outcome: EvalOutcome::Locked,
            skip_code: None,
            skip_reason: None,
            total_cycles: Some(2_000_000),
            fulfillment_type: "LockAndFulfill".to_string(),
            queue_duration_ms: Some(150),
            preflight_duration_ms: Some(800),
            received_at_timestamp: 0,
            evaluated_at: Utc::now(),
            commitment_outcome: Some(CommitmentOutcome::Committed),
            commitment_skip_code: None,
            commitment_skip_reason: None,
            estimated_proving_time_secs: Some(60),
            estimated_proving_time_no_load_secs: Some(20),
            monitor_wait_duration_ms: Some(300),
            peak_prove_khz: Some(100),
            max_capacity: Some(4),
            pending_commitment_count: Some(1),
            concurrent_proving_jobs: Some(1),
            lock_duration_secs: Some(2),
        }],
        completed: vec![RequestCompleted {
            broker_address,
            order_id: format!("{comp_request_id}-0x00-LockAndFulfill"),
            request_id: comp_request_id.clone(),
            request_digest: "0x00".to_string(),
            proof_type: "Merkle".to_string(),
            outcome: CompletionOutcome::Fulfilled,
            error_code: None,
            error_reason: None,
            lock_duration_secs: Some(5),
            proving_duration_secs: Some(30),
            aggregation_duration_secs: Some(12),
            submission_duration_secs: Some(4),
            total_duration_secs: 55,
            estimated_proving_time_secs: Some(60),
            actual_total_proving_time_secs: Some(30),
            concurrent_proving_jobs_start: Some(2),
            concurrent_proving_jobs_end: Some(2),
            total_cycles: Some(2_000_000),
            fulfillment_type: "LockAndFulfill".to_string(),
            stark_proving_secs: None,
            proof_compression_secs: None,
            set_builder_proving_secs: None,
            assessor_proving_secs: None,
            assessor_compression_proof_secs: None,
            received_at_timestamp: 0,
            completed_at: Utc::now(),
        }],
        events_dropped: 0,
        timestamp: Utc::now(),
    };
    client
        .submit_request_heartbeat(&request_heartbeat, &env.signer)
        .await
        .expect("submit_request_heartbeat failed");
    eprintln!("RequestHeartbeat sent successfully");

    // Connect to Redshift and poll for the data
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&env.redshift_url)
        .await
        .expect("failed to connect to Redshift");

    // Verify that Redshift migrations have been applied before polling.
    let schema_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM information_schema.schemata WHERE schema_name = 'telemetry')",
    )
    .fetch_one(&pool)
    .await
    .expect("failed to query information_schema");
    assert!(
        schema_exists,
        "Redshift schema 'telemetry' does not exist. \
         Run the migration script first: infra/order-stream/redshift-migrations/apply.sh <stack>"
    );

    let broker_addr_str = format!("{broker_address:#x}");
    let max_attempts = 18;
    let poll_interval = Duration::from_secs(5);

    // Poll for heartbeat row
    eprintln!("Polling Redshift for heartbeat data (up to {max_attempts} attempts)...");
    let mut heartbeat_found = false;
    for attempt in 1..=max_attempts {
        let row = sqlx::query(
            "SELECT broker_address, version FROM telemetry.broker_heartbeats \
             WHERE broker_address = $1 \
             AND JSON_SERIALIZE(config) LIKE $2 \
             LIMIT 1",
        )
        .bind(&broker_addr_str)
        .bind(format!("%{run_id}%"))
        .fetch_optional(&pool)
        .await;

        match row {
            Ok(Some(r)) => {
                let db_broker: String = r.get("broker_address");
                let db_version: String = r.get("version");
                assert_eq!(db_broker, broker_addr_str, "broker_address mismatch");
                assert!(
                    db_version.starts_with("integration-test-"),
                    "version mismatch: {db_version}"
                );
                eprintln!("  Heartbeat found on attempt {attempt}");
                heartbeat_found = true;
                break;
            }
            Ok(None) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: not yet visible");
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: query error: {e}");
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
    assert!(heartbeat_found, "Heartbeat row not found in Redshift after {max_attempts} attempts");

    // Poll for evaluation row
    eprintln!("Polling Redshift for evaluation data...");
    let mut eval_found = false;
    for attempt in 1..=max_attempts {
        let row = sqlx::query(
            "SELECT broker_address, outcome FROM telemetry.request_evaluations \
             WHERE request_id = $1 \
             LIMIT 1",
        )
        .bind(&eval_request_id)
        .fetch_optional(&pool)
        .await;

        match row {
            Ok(Some(r)) => {
                let db_broker: String = r.get("broker_address");
                let db_outcome: String = r.get("outcome");
                assert_eq!(db_broker, broker_addr_str, "eval broker_address mismatch");
                assert_eq!(db_outcome, "Locked", "eval outcome mismatch");
                eprintln!("  Evaluation found on attempt {attempt}");
                eval_found = true;
                break;
            }
            Ok(None) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: not yet visible");
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: query error: {e}");
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
    assert!(eval_found, "Evaluation row not found in Redshift after {max_attempts} attempts");

    // Poll for completion row
    eprintln!("Polling Redshift for completion data...");
    let mut comp_found = false;
    for attempt in 1..=max_attempts {
        let row = sqlx::query(
            "SELECT broker_address, outcome, total_duration_secs FROM telemetry.request_completions \
             WHERE request_id = $1 \
             LIMIT 1",
        )
        .bind(&comp_request_id)
        .fetch_optional(&pool)
        .await;

        match row {
            Ok(Some(r)) => {
                let db_broker: String = r.get("broker_address");
                let db_outcome: String = r.get("outcome");
                let db_total_dur: i64 = r.get("total_duration_secs");
                assert_eq!(db_broker, broker_addr_str, "comp broker_address mismatch");
                assert_eq!(db_outcome, "Fulfilled", "comp outcome mismatch");
                assert_eq!(db_total_dur, 55, "comp total_duration_secs mismatch");
                eprintln!("  Completion found on attempt {attempt}");
                comp_found = true;
                break;
            }
            Ok(None) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: not yet visible");
                tokio::time::sleep(poll_interval).await;
            }
            Err(e) => {
                eprintln!("  Attempt {attempt}/{max_attempts}: query error: {e}");
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
    assert!(comp_found, "Completion row not found in Redshift after {max_attempts} attempts");

    eprintln!("All telemetry data verified in Redshift. Test passed.");
}
