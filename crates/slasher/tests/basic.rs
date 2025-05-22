// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{process::Command, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, Signature, U256},
    providers::Provider,
    rpc::types::BlockNumberOrTag,
    signers::Signer,
};
use boundless_cli::OrderFulfilled;
use boundless_market::{
    contracts::{
        Offer, Predicate, PredicateType, ProofRequest, RequestId, RequestInput, Requirements,
    },
    order_stream_client::Order,
};
use boundless_market_test_utils::create_test_ctx;
use boundless_market_test_utils::{ASSESSOR_GUEST_ELF, ECHO_ID, ECHO_PATH, SET_BUILDER_ELF};
use futures_util::StreamExt;
use risc0_ethereum_contracts::set_verifier::SetVerifierService;

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
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1),
            biddingStart: now - 3,
            timeout: 15,
            rampUpPeriod: 1,
            lockTimeout: 10,
            lockStake: U256::from(0),
        },
    );

    let client_sig = req.sign_request(signer, contract_addr, chain_id).await.unwrap();

    (req, client_sig.as_bytes().into())
}

#[tokio::test]
async fn test_basic_usage() {
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-slasher");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--private-key",
        &hex::encode(ctx.customer_signer.clone().to_bytes()),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        "sqlite::memory:",
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{} {:?}", exe_path, args);

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Subscribe to slash events before operations
    let slash_event = ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
    let mut stream = slash_event.into_stream();
    println!("Subscribed to ProverSlashed event");

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
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    // Do the operations that should trigger the slash
    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.prover_market.lock_request(&request, &client_sig, None).await.unwrap();

    // Wait for the slash event with timeout
    tokio::select! {
        Some(event) = stream.next() => {
            let request_slashed = event.unwrap().0;
            println!("Detected prover slashed for request {:?}", request_slashed.requestId);
            // Check that the stake recipient is the market treasury address
            assert_eq!(request_slashed.stakeRecipient, ctx.deployment.boundless_market_address);
            cli_process.kill().unwrap();
        }
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            panic!("Test timed out waiting for slash event");
        }
    }
}

#[tokio::test]
#[ignore = "Generate proofs. Slow without dev mode"]
async fn test_slash_fulfilled() {
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-slasher");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--private-key",
        &hex::encode(ctx.customer_signer.clone().to_bytes()),
        "--boundless-market-address",
        &ctx.deployment.boundless_market_address.to_string(),
        "--db",
        "sqlite::memory:",
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{} {:?}", exe_path, args);

    #[allow(clippy::zombie_processes)]
    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Subscribe to slash events before operations
    let slash_event = ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
    let mut stream = slash_event.into_stream();
    println!("Subscribed to ProverSlashed event");

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
        ctx.deployment.boundless_market_address,
        anvil.chain_id(),
        now,
    )
    .await;

    // Do the operations that should trigger the slash
    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.prover_market.lock_request(&request, &client_sig, None).await.unwrap();
    let domain = ctx.customer_market.eip712_domain().await.unwrap();
    let order = Order::new(
        request.clone(),
        request.signing_hash(ctx.deployment.boundless_market_address, anvil.chain_id()).unwrap(),
        Signature::try_from(client_sig.as_ref()).unwrap(),
    );
    let prover = boundless_cli::DefaultProver::new(
        SET_BUILDER_ELF.to_vec(),
        ASSESSOR_GUEST_ELF.to_vec(),
        ctx.customer_signer.address(),
        domain,
    )
    .unwrap();
    let (fill, root_receipt, assessor_receipt) = prover.fulfill(&[order]).await.unwrap();
    let order_fulfilled = OrderFulfilled::new(fill, root_receipt, assessor_receipt).unwrap();
    let expires_at = request.offer.biddingStart + request.offer.timeout as u64;
    let lock_expires_at = request.offer.biddingStart + request.offer.lockTimeout as u64;
    let set_verifier = SetVerifierService::new(
        ctx.deployment.set_verifier_address,
        ctx.customer_provider.clone(),
        ctx.customer_signer.address(),
    );
    set_verifier.submit_merkle_root(order_fulfilled.root, order_fulfilled.seal).await.unwrap();

    // Wait for the lock to expire
    loop {
        let ts = ctx
            .customer_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        if ts > lock_expires_at {
            break;
        }
        println!(
            "Waiting for lock to expire...{} < {} - Expires at {}",
            ts, lock_expires_at, expires_at
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Fulfill the order
    ctx.customer_market
        .price_and_fulfill_batch(
            vec![request],
            vec![client_sig],
            order_fulfilled.fills,
            order_fulfilled.assessorReceipt,
            None,
        )
        .await
        .unwrap();

    // Wait for the slash event with timeout
    tokio::select! {
        Some(event) = stream.next() => {
            let request_slashed = event.unwrap().0;
            println!("Detected prover slashed for request {:?}", request_slashed.requestId);
            // Check that the stake recipient is the market treasury address
            assert_eq!(request_slashed.stakeRecipient, ctx.customer_signer.address());
            cli_process.kill().unwrap();
        }
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            panic!("Test timed out waiting for slash event");
        }
    }
}
