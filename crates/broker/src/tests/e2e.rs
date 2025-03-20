// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    node_bindings::Anvil,
    primitives::{utils, U256},
};
use risc0_zkvm::{is_dev_mode, sha::Digest};
use tempfile::NamedTempFile;
use url::Url;
// use broker::Broker;
use crate::{config::Config, now_timestamp, Args, Broker};
use boundless_market::contracts::{
    hit_points::default_allowance, test_utils::create_test_ctx, Input, Offer, Predicate,
    PredicateType, ProofRequest, Requirements,
};
use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
use guest_util::{ECHO_ID, ECHO_PATH};
use tokio::time::Duration;
use tracing_test::traced_test;

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

    let image_uri = format!("file://{ECHO_PATH}");

    // Start broker
    let config_file = NamedTempFile::new().unwrap();
    let mut config = Config::default();
    // - modify config here
    config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    config.market.mcycle_price = "0.00001".into();
    config.batcher.batch_size = Some(1);
    config.write(config_file.path()).await.unwrap();

    let args = Args {
        db_url: "sqlite::memory:".into(),
        config_file: config_file.path().to_path_buf(),
        boundless_market_address: ctx.boundless_market_address,
        set_verifier_address: ctx.set_verifier_address,
        rpc_url: anvil.endpoint_url(),
        order_stream_url: None,
        private_key: ctx.prover_signer,
        bento_api_url: None,
        bonsai_api_key: None,
        bonsai_api_url: None,
        deposit_amount: None,
        rpc_retry_max: 0,
        rpc_retry_backoff: 200,
        rpc_retry_cu: 1000,
    };
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit an order

    let request = ProofRequest::new(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        ),
        &image_uri,
        Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
        Offer {
            minPrice: U256::from(20000000000000u64),
            maxPrice: U256::from(40000000000000u64),
            biddingStart: now_timestamp(),
            timeout: 1200,
            lockTimeout: 1200,
            rampUpPeriod: 1,
            lockStake: U256::from(10),
        },
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

    let image_uri = format!("file://{ECHO_PATH}");

    // Start broker
    let config_file = NamedTempFile::new().unwrap();
    let mut config = Config::default();
    // - modify config here
    config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    if !is_dev_mode() {
        config.prover.bonsai_r0_zkvm_ver = Some(risc0_zkvm::VERSION.to_string());
    }
    config.prover.status_poll_ms = 1000;
    config.prover.req_retry_count = 3;
    config.market.mcycle_price = "0.00001".into();
    config.market.min_deadline = 100;
    config.batcher.batch_size = Some(1);
    config.write(config_file.path()).await.unwrap();

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

    let args = Args {
        db_url: "sqlite::memory:".into(),
        config_file: config_file.path().to_path_buf(),
        boundless_market_address: ctx.boundless_market_address,
        set_verifier_address: ctx.set_verifier_address,
        rpc_url: anvil.endpoint_url(),
        order_stream_url: None,
        private_key: ctx.prover_signer,
        bento_api_url: None,
        bonsai_api_key,
        bonsai_api_url,
        deposit_amount: None,
        rpc_retry_max: 0,
        rpc_retry_backoff: 200,
        rpc_retry_cu: 1000,
    };
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit an order
    let request = ProofRequest::new(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        )
        .with_unaggregated_proof(),
        &image_uri,
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

    let image_uri = format!("file://{ECHO_PATH}");

    // Start broker
    let config_file = NamedTempFile::new().unwrap();
    let mut config = Config::default();
    // - modify config here
    config.prover.set_builder_guest_path = Some(SET_BUILDER_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    if !is_dev_mode() {
        config.prover.bonsai_r0_zkvm_ver = Some(risc0_zkvm::VERSION.to_string());
    }
    config.prover.status_poll_ms = 1000;
    config.prover.req_retry_count = 3;
    config.market.mcycle_price = "0.00001".into();
    config.market.min_deadline = 100;
    config.batcher.batch_size = Some(2);
    config.write(config_file.path()).await.unwrap();

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

    let args = Args {
        db_url: "sqlite::memory:".into(),
        config_file: config_file.path().to_path_buf(),
        boundless_market_address: ctx.boundless_market_address,
        set_verifier_address: ctx.set_verifier_address,
        rpc_url: anvil.endpoint_url(),
        order_stream_url: None,
        private_key: ctx.prover_signer,
        bento_api_url: None,
        bonsai_api_key,
        bonsai_api_url,
        deposit_amount: None,
        rpc_retry_max: 0,
        rpc_retry_backoff: 200,
        rpc_retry_cu: 1000,
    };
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit the first order
    let request = ProofRequest::new(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        ),
        &image_uri,
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
    );

    ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // Submit the second (unaggregated) order
    let request_unaggregated = ProofRequest::new(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        )
        .with_unaggregated_proof(),
        &image_uri,
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
    );

    ctx.customer_market.submit_request(&request_unaggregated, &ctx.customer_signer).await.unwrap();

    ctx.customer_market
        .wait_for_request_fulfillment(
            U256::from(request.id),
            Duration::from_secs(1),
            request.expires_at(),
        )
        .await
        .unwrap();

    ctx.customer_market
        .wait_for_request_fulfillment(
            U256::from(request_unaggregated.id),
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
