// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::{
    node_bindings::Anvil,
    primitives::{aliases::U96, utils, B256, U256},
    providers::Provider,
};
use httpmock::prelude::*;
use risc0_zkvm::sha::Digest;
use tempfile::NamedTempFile;
// use broker::Broker;
use crate::{config::Config, provers::encode_input, Args, Broker};
use aggregation_set::SET_BUILDER_GUEST_PATH;
use boundless_market::contracts::{
    test_utils::TestCtx, Input, InputType, Offer, Predicate, PredicateType, ProvingRequest,
    Requirements,
};
use guest_assessor::ASSESSOR_GUEST_PATH;
use guest_util::{ECHO_ELF, ECHO_ID};
use tokio::time::Duration;
use tracing_test::traced_test;

#[tokio::test]
#[traced_test]
async fn simple_e2e() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    // Setup signers / providers
    let ctx = TestCtx::new(&anvil).await.unwrap();

    // Deposit prover / customer balances
    ctx.prover_market.deposit(utils::parse_ether("2").unwrap()).await.unwrap();
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
    config.prover.set_builder_guest_path = Some(SET_BUILDER_GUEST_PATH.into());
    config.prover.assessor_set_guest_path = Some(ASSESSOR_GUEST_PATH.into());
    config.market.mcycle_price = "0.00001".into();
    config.batcher.batch_size = Some(1);
    config.write(config_file.path()).await.unwrap();

    let args = Args {
        db_url: "sqlite::memory:".into(),
        config_file: config_file.path().to_path_buf(),
        proof_market_addr: ctx.proof_market_addr,
        set_verifier_addr: ctx.set_verifier_addr,
        rpc_url: anvil.endpoint_url(),
        order_stream_url: None,
        private_key: ctx.prover_signer,
        bento_api_url: None,
        bonsai_api_key: None,
        bonsai_api_url: None,
        deposit_amount: None,
    };
    let broker = Broker::new(args, ctx.prover_provider).await.unwrap();
    let broker_task = tokio::spawn(async move {
        broker.start_service().await.unwrap();
    });

    // Submit a order

    let request = ProvingRequest::new(
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
        Input {
            inputType: InputType::Inline,
            data: encode_input(&vec![0x41, 0x41, 0x41, 0x41]).unwrap().into(),
        },
        Offer {
            minPrice: U96::from(20000000000000u64),
            maxPrice: U96::from(40000000000000u64),
            biddingStart: ctx.customer_provider.get_block_number().await.unwrap(),
            timeout: 100,
            rampUpPeriod: 1,
            lockinStake: U96::from(10),
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
    get_mock.assert();
}
