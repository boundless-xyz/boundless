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

use std::{future::Future, path::PathBuf};

use crate::{
    config::{Config, ConfigWatcher},
    now_timestamp, Args, Broker,
};
use alloy::{
    node_bindings::Anvil,
    primitives::{
        aliases::U96,
        utils::{self, parse_ether},
        Address, Bytes, FixedBytes, U256,
    },
    providers::{Provider, WalletProvider},
    signers::local::PrivateKeySigner,
};
use boundless_market::{
    contracts::{
        hit_points::default_allowance, Callback, FulfillmentData, Offer, Predicate, ProofRequest,
        RequestId, RequestInput, Requirements,
    },
    selector::{is_blake3_groth16_selector, is_groth16_selector, ProofType},
    storage::{MockStorageProvider, StorageProvider},
    Deployment,
};
use boundless_test_utils::{
    guests::{ASSESSOR_GUEST_PATH, ECHO_ELF, ECHO_ID, SET_BUILDER_PATH},
    market::{create_test_ctx, deploy_mock_callback, get_mock_callback_count},
};
use risc0_zkvm::{
    sha::{Digest, Digestible},
    ReceiptClaim,
};
use tempfile::NamedTempFile;
use tokio::{task::JoinSet, time::Duration};
use tracing_test::traced_test;
use url::Url;

fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[allow(clippy::too_many_arguments)]
fn generate_request(
    id: u32,
    addr: &Address,
    proof_type: ProofType,
    image_url: impl Into<String>,
    callback: Option<Callback>,
    offer: Option<Offer>,
    predicate: Option<Predicate>,
    input: Option<RequestInput>,
) -> ProofRequest {
    let mut requirements = Requirements::new(
        predicate
            .unwrap_or_else(|| Predicate::prefix_match(Digest::from(ECHO_ID), Bytes::default())),
    );

    if proof_type == ProofType::Groth16 {
        requirements = requirements.with_groth16_proof();
    } else if proof_type == ProofType::Blake3Groth16 {
        requirements = requirements.with_blake3_groth16_proof();
        let blake3_claim_digest =
            blake3_groth16::Blake3Groth16ReceiptClaim::ok(ECHO_ID, [255u8; 32]).digest();
        requirements =
            requirements.with_predicate(Predicate::claim_digest_match(blake3_claim_digest));
    }
    if let Some(callback) = callback {
        requirements = requirements.with_callback(callback);
    }
    ProofRequest::new(
        RequestId::new(*addr, id),
        requirements,
        image_url,
        input.unwrap_or_else(|| {
            RequestInput::builder().write_slice(&[255u8; 32]).build_inline().unwrap()
        }),
        offer.unwrap_or(Offer {
            minPrice: parse_ether("0.02").unwrap(),
            maxPrice: parse_ether("0.04").unwrap(),
            rampUpStart: now_timestamp(),
            timeout: 120,
            lockTimeout: 120,
            rampUpPeriod: 1,
            lockCollateral: U256::from(10),
        }),
    )
}

async fn new_config(min_batch_size: u32) -> NamedTempFile {
    new_config_with_min_deadline(min_batch_size, 100).await
}

async fn new_config_with_min_deadline(min_batch_size: u32, min_deadline: u64) -> NamedTempFile {
    let config_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let mut config = Config::default();
    config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    if !is_dev_mode() {
        config.prover.bonsai_r0_zkvm_ver = Some(risc0_zkvm::VERSION.to_string());
    }
    config.prover.status_poll_ms = 1000;
    config.prover.req_retry_count = 3;
    config.market.min_mcycle_price = "0.00001".into();
    config.market.min_mcycle_price_collateral_token = "0.0".into();
    config.market.min_deadline = min_deadline;
    config.batcher.min_batch_size = min_batch_size;
    config.write(config_file.path()).await.unwrap();
    config_file
}

fn broker_args(
    config_file: PathBuf,
    deployment: Deployment,
    rpc_url: Url,
    private_key: PrivateKeySigner,
) -> Args {
    let (bonsai_api_url, bonsai_api_key) = match is_dev_mode() {
        true => (None, None),
        false => (
            Some(
                Url::parse(&std::env::var("BONSAI_API_URL").expect("BONSAI_API_URL must be set"))
                    .unwrap(),
            ),
            Some(std::env::var("BONSAI_API_KEY").expect("BONSAI_API_KEY must be set")),
        ),
    };

    Args {
        db_url: "sqlite::memory:".into(),
        config_file,
        deployment: Some(deployment),
        rpc_url: Some(rpc_url.to_string()),
        rpc_urls: Vec::new(),
        private_key: Some(private_key),
        bento_api_url: None,
        bonsai_api_key,
        bonsai_api_url,
        deposit_amount: None,
        rpc_retry_max: 0,
        rpc_retry_backoff: 200,
        rpc_retry_cu: 1000,
        log_json: false,
    }
}

async fn run_with_broker<P, F, T>(broker: Broker<P>, f: F) -> T
where
    P: Provider + WalletProvider + Clone + 'static,
    F: Future<Output = T>,
{
    // A JoinSet automatically aborts all its tasks when dropped
    let mut tasks = JoinSet::new();
    // Spawn the broker
    tasks.spawn(async move { broker.start_service().await });

    tokio::select! {
        result = f => result,
        broker_task_result = tasks.join_next() => {
            panic!("Broker exited unexpectedly: {:?}", broker_task_result.unwrap());
        },
    }
}

#[tokio::test]
#[traced_test]
async fn simple_e2e() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    // Submit an order
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

    run_with_broker(broker, async move {
        // Submit the request
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        ctx.customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[traced_test]
async fn simple_e2e_with_callback() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Deploy MockCallback contract
    let callback_address = deploy_mock_callback(
        &ctx.prover_provider,
        ctx.deployment.verifier_router_address.expect("verifier_router_address should be set"),
        ctx.deployment.boundless_market_address,
        ECHO_ID,
        U256::ZERO,
    )
    .await
    .unwrap();

    let callback = Callback { addr: callback_address, gasLimit: U96::from(100000) };

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider.clone(),
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    // Submit an order with callback
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        image_url,
        Some(callback),
        None,
        None,
        None,
    );

    run_with_broker(broker, async move {
        // Submit the request
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        ctx.customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();

        // Check for callback failures
        let event_filter = ctx
            .customer_market
            .instance()
            .CallbackFailed_filter()
            .topic1(request.id)
            .from_block(0)
            .to_block(ctx.prover_provider.get_block_number().await.unwrap());
        let logs = event_filter.query().await.unwrap();
        assert!(logs.is_empty(), "Found unexpected callback failure logs");

        // Verify callback count
        let count = get_mock_callback_count(&ctx.prover_provider, callback_address).await.unwrap();
        assert_eq!(count, U256::from(1), "Expected exactly one callback");
    })
    .await;
}

#[tokio::test]
#[traced_test]
async fn e2e_fulfill_after_lock_expiry() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let locker_market = ctx.customer_market.clone();
    let locker_signer = ctx.customer_signer.clone();
    let prover_signer = ctx.prover_signer.clone();

    ctx.hit_points_service.mint(locker_signer.address(), default_allowance()).await.unwrap();
    ctx.hit_points_service.mint(prover_signer.address(), default_allowance()).await.unwrap();

    // Deposit locker balances
    locker_market
        .deposit_collateral_with_permit(default_allowance(), &locker_signer)
        .await
        .unwrap();
    locker_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    let config = new_config_with_min_deadline(1, 0).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    // Submit an order
    let request = generate_request(
        locker_market.index_from_nonce().await.unwrap(),
        &locker_signer.address(),
        ProofType::Any,
        image_url,
        None,
        Some(Offer {
            minPrice: parse_ether("0.0").unwrap(),
            maxPrice: parse_ether("0.000000001").unwrap(),
            rampUpStart: now_timestamp(),
            rampUpPeriod: 40,
            lockTimeout: 40,
            timeout: 120,
            lockCollateral: U256::from(5),
        }),
        None,
        None,
    );

    run_with_broker(broker, async move {
        let request_id = locker_market.submit_request(&request, &locker_signer).await.unwrap();
        let (_, client_sig) = locker_market.get_submitted_request(request_id, None).await.unwrap();
        locker_market.lock_request(&request, client_sig).await.unwrap();

        // Wait for fulfillment
        ctx.customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(3),
                request.expires_at(),
            )
            .await
            .unwrap();
    })
    .await;
}

#[tokio::test]
#[traced_test]
async fn e2e_with_selector() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Groth16,
        image_url,
        None,
        None,
        None,
        None,
    );

    run_with_broker(broker, async move {
        // Submit the request
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        let fulfillment = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let seal = fulfillment.seal;
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(is_groth16_selector(selector));
    })
    .await;
}

#[tokio::test]
#[traced_test]
async fn e2e_with_blake3_groth16_selector() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();
    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Blake3Groth16,
        image_url,
        None,
        None,
        None,
        None,
    );

    run_with_broker(broker, async move {
        // Submit the request
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        let fulfillment = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let seal = fulfillment.seal;
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(is_blake3_groth16_selector(selector));
    })
    .await;
}

#[tokio::test]
#[traced_test]
#[ignore = "runs a proof; requires BONSAI if RISC0_DEV_MODE=FALSE"]
async fn e2e_with_multiple_requests() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(2).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap().to_string();

    // Submit the first order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        &image_url,
        None,
        None,
        None,
        None,
    );

    run_with_broker(broker, async move {
        // Submit the first order
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        let request_groth16 = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
            ProofType::Groth16,
            &image_url,
            None,
            None,
            None,
            None,
        );

        // Submit the second (groth16) order
        ctx.customer_market.submit_request(&request_groth16, &ctx.customer_signer).await.unwrap();

        let fulfillment = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let seal = fulfillment.seal;
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(!is_groth16_selector(selector));

        let fulfillment = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request_groth16.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let seal = fulfillment.seal;
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(is_groth16_selector(selector));
    })
    .await;
}

#[tokio::test]
#[traced_test]
async fn e2e_with_claim_digest_match() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.deployment.clone(),
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(
        args,
        ctx.prover_provider,
        ConfigWatcher::new(config.path()).await.unwrap(),
        Default::default(),
    )
    .await
    .unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_program(ECHO_ELF).await.unwrap();

    let input_bytes = vec![0x41, 0x41, 0x41, 0x41];
    let input = RequestInput::builder().write_slice(&input_bytes).build_inline().unwrap();

    let correct_claim_digest = ReceiptClaim::ok(ECHO_ID, input_bytes).digest();

    let predicate = Predicate::claim_digest_match(correct_claim_digest);

    run_with_broker(broker, async move {
        // Request 1: Regular valid request
        let good_request = generate_request(
            ctx.customer_market.index_from_nonce().await.unwrap(),
            &ctx.customer_signer.address(),
            ProofType::Groth16,
            image_url.clone(),
            None,
            None,
            Some(predicate),
            Some(input.clone()),
        );
        ctx.customer_market.submit_request(&good_request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        let fulfillment = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(good_request.id),
                Duration::from_secs(1),
                good_request.expires_at(),
            )
            .await
            .unwrap();
        let fulfillment_data = fulfillment.data().unwrap();

        // When claim digest match is used without a callback, fulfillment data is empty
        assert_eq!(fulfillment_data, FulfillmentData::None);
    })
    .await;
}
