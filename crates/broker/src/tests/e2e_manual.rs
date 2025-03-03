// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    node_bindings::Anvil,
    primitives::{utils, B256, U256},
    providers::Provider,
};
use httpmock::prelude::*;
use risc0_zkvm::sha::Digest;
use tempfile::NamedTempFile;
use url::Url;
// use broker::Broker;
use crate::{config::Config, Args, Broker, Order};
use boundless_market::contracts::{
    hit_points::default_allowance, test_utils::TestCtx, Input, Offer, Predicate, PredicateType,
    ProofRequest, Requirements,
};
use guest_assessor::{ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
use guest_set_builder::{SET_BUILDER_ID, SET_BUILDER_PATH};
use guest_util::{ECHO_ELF, ECHO_ID};
use tokio::time::Duration;
use tracing_test::traced_test;

const BROKER_URL: &str = "localhost:5312";

#[tokio::test]
#[traced_test]
async fn e2e_manual() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
        .await
        .unwrap();

    // Deposit prover / customer balances
    ctx.prover_market
        .deposit_stake_with_permit(default_allowance(), &ctx.prover_signer)
        .await
        .unwrap();
    ctx.customer_market.deposit(utils::parse_ether("0.5").unwrap()).await.unwrap();

    // Stand up a local http server for image delivery
    // TODO: move to TestCtx
    let server = MockServer::start();
    let get_mock = server.mock(|when, then| {
        when.method(GET).path("/image");
        then.status(200).body(ECHO_ELF);
    });
    let image_uri = format!("http://{}/image", server.address());

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
        boundless_market_addr: ctx.boundless_market_addr,
        set_verifier_addr: ctx.set_verifier_addr,
        rpc_url: anvil.endpoint_url(),
        broker_api: Some(Url::parse(BROKER_URL).unwrap()),
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

    let client = reqwest::Client::new();

    // Poll the latest batches endpoint with a maximum of 5 attempts
    const MAX_ATTEMPTS: u32 = 20;

    // Wait for the broker API to start up
    for _ in 0..MAX_ATTEMPTS {
        let res = client.get(format!("http://{}/v1/batches/latest", BROKER_URL)).send().await;
        if res.as_ref().map(|r| r.status().is_success()).unwrap_or(false) {
            // println!("batch: {:?}", res.unwrap().json::<Batch>().await.unwrap());

            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // Submit an order.
    let request = ProofRequest::new(
        ctx.customer_market.index_from_nonce().await.unwrap(),
        &ctx.customer_signer.address(),
        Requirements {
            imageId: B256::from_slice(Digest::from(ECHO_ID).as_bytes()),
            predicate: Predicate {
                predicateType: PredicateType::PrefixMatch,
                data: Default::default(),
            },
        },
        &image_uri,
        Input::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
        Offer {
            minPrice: U256::from(20000000000000u64),
            maxPrice: U256::from(40000000000000u64),
            biddingStart: ctx.customer_provider.get_block_number().await.unwrap(),
            timeout: 100,
            lockTimeout: 100,
            rampUpPeriod: 1,
            lockStake: U256::from(10),
        },
    );
    let signature = request
        .sign_request(&ctx.customer_signer, ctx.boundless_market_addr, anvil.chain_id())
        .await
        .unwrap();

    let order = Order::new(request.clone(), signature.as_bytes().into());

    let res = client
        .post(format!("http://{}/v1/orders/new", BROKER_URL))
        .json(&order)
        .send()
        .await
        .unwrap();
    println!("res: {:?}", res);

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
    get_mock.assert();
}
