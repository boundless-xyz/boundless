// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{process::Command, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, PrimitiveSignature, U256},
    providers::Provider,
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use boundless_cli::{DefaultProver, OrderFulfilled};
use boundless_indexer::test_utils::TestDb;
use boundless_market::{
    contracts::{Input, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements},
    order_stream_client::Order,
};
use boundless_market_test_utils::create_test_ctx;
use guest_assessor::{ASSESSOR_GUEST_ELF, ASSESSOR_GUEST_ID, ASSESSOR_GUEST_PATH};
use guest_set_builder::{SET_BUILDER_ELF, SET_BUILDER_ID, SET_BUILDER_PATH};
use guest_util::{ECHO_ID, ECHO_PATH};
use sqlx::Row;

async fn create_order(
    signer: &impl Signer,
    signer_addr: Address,
    order_id: u32,
    contract_addr: Address,
    chain_id: u64,
    now: u64,
) -> (ProofRequest, Bytes) {
    let req = ProofRequest::new(
        RequestId::new(signer_addr, order_id),
        Requirements::new(
            ECHO_ID,
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        ),
        format!("file://{ECHO_PATH}"),
        Input::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1),
            biddingStart: now - 3,
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockStake: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

#[tokio::test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_e2e() {
    let test_db = TestDb::new().await.unwrap();
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(
        &anvil,
        SET_BUILDER_ID,
        format!("file://{SET_BUILDER_PATH}"),
        ASSESSOR_GUEST_ID,
        format!("file://{ASSESSOR_GUEST_PATH}"),
    )
    .await
    .unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-indexer");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--boundless-market-address",
        &ctx.boundless_market_address.to_string(),
        "--db",
        &test_db.db_url,
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{} {:?}", exe_path, args);

    let prover = DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.prover_signer.address(),
        ctx.customer_market.eip712_domain().await.unwrap(),
    )
    .unwrap();

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap()
        .header
        .timestamp;

    let (request, client_sig) = create_order(
        &ctx.customer_signer,
        ctx.customer_signer.address(),
        1,
        ctx.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.customer_market.submit_request_with_signature_bytes(&request, &client_sig).await.unwrap();
    ctx.prover_market.lock_request(&request, &client_sig, None).await.unwrap();

    let (fill, root_receipt, assessor_receipt) = prover
        .fulfill(&[Order {
            request: request.clone(),
            request_digest: request
                .signing_hash(ctx.boundless_market_address, anvil.chain_id())
                .unwrap(),
            signature: PrimitiveSignature::try_from(client_sig.as_ref()).unwrap(),
        }])
        .await
        .unwrap();
    let order_fulfilled =
        OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    ctx.prover_market
        .submit_merkle_and_fulfill(
            ctx.set_verifier_address,
            order_fulfilled.root,
            order_fulfilled.seal,
            order_fulfilled.fills,
            order_fulfilled.assessorReceipt,
        )
        .await
        .unwrap();

    // Wait for the events to be indexed
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Check that the request was indexed
    let result = sqlx::query("SELECT * FROM proof_requests WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // check that the requestSubmitted event was indexed
    let result = sqlx::query("SELECT * FROM request_submitted_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the request was locked
    let result = sqlx::query("SELECT * FROM request_locked_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the proof was delivered
    let result = sqlx::query("SELECT * FROM proof_delivered_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the fulfillment was indexed
    let result = sqlx::query("SELECT * FROM fulfillments WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    // Check that the proof was fulfilled
    let result = sqlx::query("SELECT * FROM request_fulfilled_events WHERE request_id == $1")
        .bind(format!("{:x}", request.id))
        .fetch_one(&test_db.pool)
        .await
        .unwrap();
    let request_id = result.get::<String, _>("request_id");
    assert_eq!(request_id, format!("{:x}", request.id));

    cli_process.kill().unwrap();
}
