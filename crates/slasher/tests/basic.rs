// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{process::Command, time::Duration};

use alloy::{
    node_bindings::Anvil,
    primitives::{Address, Bytes, U256},
    providers::Provider,
    rpc::types::{BlockNumberOrTag, BlockTransactionsKind},
    signers::Signer,
};
use boundless_market::contracts::{
    test_utils::create_test_ctx, Input, Offer, Predicate, PredicateType, ProofRequest, RequestId, Requirements
};
use futures_util::StreamExt;
use guest_assessor::ASSESSOR_GUEST_ID;
use guest_set_builder::SET_BUILDER_ID;
use risc0_zkvm::sha::Digest;

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
            Digest::ZERO,
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        ),
        "https://dev.null".to_string(),
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
async fn test_basic_usage() {
    let anvil = Anvil::new().spawn();
    let rpc_url = anvil.endpoint_url();
    let ctx = create_test_ctx(&anvil, SET_BUILDER_ID, ASSESSOR_GUEST_ID).await.unwrap();

    let exe_path = env!("CARGO_BIN_EXE_boundless-slasher");
    let args = [
        "--rpc-url",
        rpc_url.as_str(),
        "--private-key",
        &hex::encode(ctx.customer_signer.clone().to_bytes()),
        "--boundless-market-address",
        &ctx.boundless_market_address.to_string(),
        "--db",
        "sqlite::memory:",
        "--interval",
        "1",
        "--retries",
        "1",
    ];

    println!("{} {:?}", exe_path, args);

    let mut cli_process = Command::new(exe_path).args(args).spawn().unwrap();

    // Subscribe to slash events before operations
    let slash_event = ctx.customer_market.instance().ProverSlashed_filter().watch().await.unwrap();
    let mut stream = slash_event.into_stream();
    println!("Subscribed to ProverSlashed event");

    // Use the chain's timestamps to avoid inconsistencies with system time.
    let now = ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest, BlockTransactionsKind::Hashes)
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

    // Do the operations that should trigger the slash
    ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    ctx.prover_market.lock_request(&request, &client_sig, None).await.unwrap();

    // Wait for the slash event with timeout
    tokio::select! {
        Some(event) = stream.next() => {
            let request_slashed = event.unwrap().0;
            println!("Detected prover slashed for request {:?}", request_slashed.requestId);
            // Check that the stake recipient is the market treasury address
            assert_eq!(request_slashed.stakeRecipient, ctx.boundless_market_address);
            cli_process.kill().unwrap();
        }
        _ = tokio::time::sleep(Duration::from_secs(20)) => {
            panic!("Test timed out waiting for slash event");
        }
    }
}
