// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{future::Future, path::PathBuf};

use crate::{config::Config, now_timestamp, Args, Broker};
use alloy::{
    node_bindings::Anvil,
    primitives::{aliases::U96, utils, utils::parse_ether, Address, FixedBytes, U256},
    providers::{Provider, WalletProvider},
    signers::local::PrivateKeySigner,
};
use boundless_market::{
    contracts::{
        hit_points::default_allowance, Callback, Input, Offer, Predicate, PredicateType,
        ProofRequest, RequestId, Requirements,
    },
    selector::{is_groth16_selector, ProofType},
    storage::{MockStorageProvider, StorageProvider},
};
use boundless_market_test_utils::{create_test_ctx, deploy_mock_callback, get_mock_callback_count};
use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
use guest_util::{ECHO_ELF, ECHO_ID};
use risc0_zkvm::{is_dev_mode, sha::Digest};
use tempfile::NamedTempFile;
use tokio::{task::JoinSet, time::Duration};
use tracing_test::traced_test;
use url::Url;

fn generate_request(
    id: u32,
    addr: &Address,
    proof_type: ProofType,
    image_url: impl Into<String>,
    callback: Option<Callback>,
    offer: Option<Offer>,
) -> ProofRequest {
    let mut requirements = Requirements::new(
        Digest::from(ECHO_ID),
        Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
    );
    if proof_type == ProofType::Groth16 {
        requirements = requirements.with_groth16_proof();
    }
    if let Some(callback) = callback {
        requirements = requirements.with_callback(callback);
    }
    ProofRequest::new(
        RequestId::new(*addr, id),
        requirements,
        image_url,
        Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
        offer.unwrap_or(Offer {
            minPrice: parse_ether("0.02").unwrap(),
            maxPrice: parse_ether("0.04").unwrap(),
            biddingStart: now_timestamp(),
            timeout: 120,
            lockTimeout: 120,
            rampUpPeriod: 1,
            lockStake: U256::from(10),
        }),
    )
}

async fn new_config(min_batch_size: u64) -> NamedTempFile {
    new_config_with_min_deadline(min_batch_size, 100).await
}

async fn new_config_with_min_deadline(min_batch_size: u64, min_deadline: u64) -> NamedTempFile {
    let config_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    let mut config = Config::default();
    config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    if !is_dev_mode() {
        config.prover.bonsai_r0_zkvm_ver = Some(risc0_zkvm::VERSION.to_string());
    }
    config.prover.status_poll_ms = 1000;
    config.prover.req_retry_count = 3;
    config.market.mcycle_price = "0.00001".into();
    config.market.mcycle_price_stake_token = "0.0".into();
    config.market.min_deadline = min_deadline;
    config.batcher.min_batch_size = Some(min_batch_size);
    config.write(config_file.path()).await.unwrap();
    config_file
}

fn broker_args(
    config_file: PathBuf,
    boundless_market_address: Address,
    set_verifier_address: Address,
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
        boundless_market_address,
        set_verifier_address,
        rpc_url,
        order_stream_url: None,
        private_key,
        bento_api_url: None,
        bonsai_api_key,
        bonsai_api_url,
        deposit_amount: None,
        rpc_retry_max: 0,
        rpc_retry_backoff: 200,
        rpc_retry_cu: 1000,
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
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.boundless_market_address,
        ctx.set_verifier_address,
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_image(ECHO_ELF).await.unwrap();

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        image_url,
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
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Deploy MockCallback contract
    let callback_address = deploy_mock_callback(
        &ctx.prover_provider,
        ctx.verifier_address,
        ctx.boundless_market_address,
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
        ctx.boundless_market_address,
        ctx.set_verifier_address,
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(args, ctx.prover_provider.clone()).await.unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_image(ECHO_ELF).await.unwrap();

    // Submit an order with callback
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        image_url,
        Some(callback),
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
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    let locker_market = ctx.customer_market.clone();
    let locker_signer = ctx.customer_signer.clone();
    let prover_signer = ctx.prover_signer.clone();

    ctx.hit_points_service.mint(locker_signer.address(), default_allowance()).await.unwrap();
    ctx.hit_points_service.mint(prover_signer.address(), default_allowance()).await.unwrap();

    // Deposit locker balances
    locker_market.deposit_stake_with_permit(default_allowance(), &locker_signer).await.unwrap();
    locker_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    let config = new_config_with_min_deadline(1, 0).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.boundless_market_address,
        ctx.set_verifier_address,
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_image(ECHO_ELF).await.unwrap();

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
            biddingStart: now_timestamp(),
            rampUpPeriod: 40,
            lockTimeout: 40,
            timeout: 120,
            lockStake: U256::from(5),
        }),
    );

    run_with_broker(broker, async move {
        let request_id = locker_market.submit_request(&request, &locker_signer).await.unwrap();
        let (_, client_sig) = locker_market.get_submitted_request(request_id, None).await.unwrap();
        locker_market.lock_request(&request, &client_sig, None).await.unwrap();

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
#[ignore = "runs a proof; requires BONSAI if RISC0_DEV_MODE=FALSE"]
async fn e2e_with_selector() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(1).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.boundless_market_address,
        ctx.set_verifier_address,
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_image(ECHO_ELF).await.unwrap();

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Groth16,
        image_url,
        None,
        None,
    );

    run_with_broker(broker, async move {
        // Submit the request
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

        // Wait for fulfillment
        let (_, seal) = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(is_groth16_selector(selector));
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
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Start broker
    let config = new_config(2).await;
    let args = broker_args(
        config.path().to_path_buf(),
        ctx.boundless_market_address,
        ctx.set_verifier_address,
        anvil.endpoint_url(),
        ctx.prover_signer,
    );
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();

    // Provide URL for ECHO program
    let storage = MockStorageProvider::start();
    let image_url = storage.upload_image(ECHO_ELF).await.unwrap().to_string();

    // Submit the first order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        ProofType::Any,
        &image_url,
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
        );

        // Submit the second (groth16) order
        ctx.customer_market.submit_request(&request_groth16, &ctx.customer_signer).await.unwrap();

        let (_, seal) = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(!is_groth16_selector(selector));

        let (_, seal) = ctx
            .customer_market
            .wait_for_request_fulfillment(
                U256::from(request_groth16.id),
                Duration::from_secs(1),
                request.expires_at(),
            )
            .await
            .unwrap();
        let selector = FixedBytes(seal[0..4].try_into().unwrap());
        assert!(is_groth16_selector(selector));
    })
    .await;
}
