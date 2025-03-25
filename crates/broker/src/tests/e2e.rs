// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::path::PathBuf;

use crate::{config::Config, now_timestamp, Args, Broker};
use alloy::{
    node_bindings::Anvil,
    primitives::{utils, Address, FixedBytes, U256},
    signers::local::PrivateKeySigner,
};
use boundless_market::{
    contracts::{
        hit_points::default_allowance, test_utils::create_test_ctx, Input, Offer, Predicate,
        PredicateType, ProofRequest, Requirements,
    },
    selector::is_unaggregated_selector,
};
use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
use guest_util::{ECHO_ID, ECHO_PATH};
use risc0_zkvm::{is_dev_mode, sha::Digest};
use tempfile::NamedTempFile;
use tokio::time::Duration;
use tracing_test::traced_test;
use url::Url;

fn generate_request(id: u32, addr: &Address, unaggregated: bool) -> ProofRequest {
    let mut requirements = Requirements::new(
        Digest::from(ECHO_ID),
        Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
    );
    if unaggregated {
        requirements = requirements.with_unaggregated_proof();
    }
    ProofRequest::new(
        id,
        addr,
        requirements,
        format!("file://{ECHO_PATH}"),
        Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
        Offer {
            minPrice: U256::from(20000000000000u64),
            maxPrice: U256::from(40000000000000u64),
            biddingStart: now_timestamp(),
            timeout: 420,
            lockTimeout: 420,
            rampUpPeriod: 1,
            lockStake: U256::from(10),
        },
    )
}

async fn new_config(min_batch_size: u64) -> NamedTempFile {
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
    config.market.min_deadline = 100;
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
        nocache: true,
        cache_dir: None,
    }
}

#[tokio::test]
#[traced_test]
async fn simple_e2e() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

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
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        false,
    );

    ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    ctx.customer_market
        .wait_for_request_fulfillment(
            U256::from(request.id),
            Duration::from_secs(1),
            request.expires_at(),
        )
        .await
        .unwrap();

    // Check for a broker panic
    if broker_task.is_finished() {
        broker_task.await.unwrap();
    } else {
        broker_task.abort();
    }
}

#[tokio::test]
#[traced_test]
#[ignore = "runs a proof; requires BONSAI if RISC0_DEV_MODE=FALSE"]
async fn e2e_with_selector() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

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
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit an order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        true,
    );

    ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    let (_journal, seal) = ctx
        .customer_market
        .wait_for_request_fulfillment(
            U256::from(request.id),
            Duration::from_secs(1),
            request.expires_at(),
        )
        .await
        .unwrap();

    let selector = FixedBytes(seal[0..4].try_into().unwrap());
    assert!(is_unaggregated_selector(selector));

    // Check for a broker panic
    if broker_task.is_finished() {
        broker_task.await.unwrap();
    } else {
        broker_task.abort();
    }
}

#[tokio::test]
#[traced_test]
#[ignore = "runs a proof; requires BONSAI if RISC0_DEV_MODE=FALSE"]
async fn e2e_with_multiple_requests() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

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
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit the first order
    let request = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        false,
    );

    ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // Submit the second (unaggregated) order
    let request_unaggregated = generate_request(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        true,
    );

    ctx.customer_market.submit_request(&request_unaggregated, &ctx.customer_signer).await.unwrap();

    let (_journal, seal) = ctx
        .customer_market
        .wait_for_request_fulfillment(
            U256::from(request.id),
            Duration::from_secs(1),
            request.expires_at(),
        )
        .await
        .unwrap();

    let selector = FixedBytes(seal[0..4].try_into().unwrap());
    assert!(!is_unaggregated_selector(selector));

    let (_journal, seal) = ctx
        .customer_market
        .wait_for_request_fulfillment(
            U256::from(request_unaggregated.id),
            Duration::from_secs(1),
            request.expires_at(),
        )
        .await
        .unwrap();

    let selector = FixedBytes(seal[0..4].try_into().unwrap());
    assert!(is_unaggregated_selector(selector));

    // Check for a broker panic
    if broker_task.is_finished() {
        broker_task.await.unwrap();
    } else {
        broker_task.abort();
    }
}
