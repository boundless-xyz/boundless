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

use alloy::node_bindings::Anvil;
use alloy::primitives::{utils, U256};
use boundless_market::contracts::hit_points::default_allowance;
use boundless_market::selector::ProofType;
use boundless_market::storage::{MockStorageUploader, StorageUploader};
use boundless_test_utils::guests::ECHO_ELF;
use boundless_test_utils::market::create_test_ctx;
use tokio::time::Duration;
use tracing_test::traced_test;

use crate::config::TelemetryMode;
use crate::Config;

use super::e2e::{
    broker_args, build_test_chain, generate_request, new_config_with_min_deadline, run_with_broker,
    TestConfig,
};

async fn new_config_with_telemetry(
    min_batch_size: u32,
    mode: TelemetryMode,
    heartbeat_interval_secs: u64,
) -> TestConfig {
    let test_config = new_config_with_min_deadline(min_batch_size, 100).await;
    let mut config = Config::load(test_config.base.path()).await.unwrap();
    config.market.request_heartbeat_interval_secs = heartbeat_interval_secs;
    config.market.broker_heartbeat_interval_secs = heartbeat_interval_secs;
    config.market.telemetry_mode = mode;
    config.write(test_config.base.path()).await.unwrap();
    test_config
}

#[tokio::test]
#[traced_test("debug")]
async fn e2e_telemetry_events() {
    let anvil = Anvil::new().spawn();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    let config = new_config_with_telemetry(1, TelemetryMode::LogsOnly, 2).await;
    let config_watcher = config.watcher().await;
    let args = broker_args(
        config.base_path(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer.clone(),
        Some(ctx.version_registry_address),
    );
    let db_dir = tempfile::tempdir().unwrap();
    let chain = build_test_chain(
        &ctx.prover_provider,
        &ctx.prover_signer,
        &ctx.deployment,
        anvil.endpoint_url(),
        &config_watcher.config,
        db_dir.path(),
    )
    .await;
    let broker = crate::Broker::new(args, config_watcher).await.unwrap();

    let storage = MockStorageUploader::new();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        image_url,
        None,
        None,
        None,
        None,
    );
    let expected_request_id_log = format!("\"request_id\":\"0x{:x}\"", request.id);

    run_with_broker(broker, vec![chain], async move {
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        ctx.customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();

        for _ in 0..30 {
            if logs_contain("(Telemetry) Request Completed:")
                && logs_contain(&expected_request_id_log)
            {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        for _ in 0..30 {
            if logs_contain("(Telemetry) Request Heartbeat") {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        for _ in 0..30 {
            if logs_contain("(Telemetry) Broker Heartbeat") {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        assert!(logs_contain("(Telemetry) Request Completed:"));
        assert!(logs_contain(&expected_request_id_log));
        assert!(logs_contain("\"outcome\":\"Fulfilled\""));
        assert!(logs_contain("\"fulfillment_type\":\"LockAndFulfill\""));
        assert!(logs_contain("\"proof_type\":\"Merkle\""));
        assert!(logs_contain("\"error_code\":null"));
        assert!(logs_contain("(Telemetry) Request Heartbeat"));
        assert!(logs_contain("(Telemetry) Broker Heartbeat"));
    })
    .await;
}
