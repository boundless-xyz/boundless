// Copyright 2025 RISC Zero, Inc.
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

use alloy::{
    consensus::Transaction,
    node_bindings::Anvil,
    primitives::{aliases::U160, utils::parse_ether, Address, U256},
    providers::Provider,
    sol_types::eip712_domain,
};
use alloy_sol_types::SolCall;
use boundless_market::{
    contracts::{
        boundless_market::{FulfillmentTx, UnlockedRequest},
        hit_points::default_allowance,
        AssessorReceipt, IBoundlessMarket, Offer, Predicate, PredicateType, ProofRequest,
        RequestId, RequestStatus, Requirements,
    },
    input::GuestEnv,
};
use boundless_market_test_utils::{create_test_ctx, mock_singleton, TestCtx, ECHO_ID};
use risc0_zkvm::sha::Digest;
use tracing_test::traced_test;

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn new_request<P: Provider>(idx: u32, ctx: &TestCtx<P>) -> ProofRequest {
    ProofRequest::new(
        RequestId::new(ctx.customer_signer.address(), idx),
        Requirements::new(
            Digest::from(ECHO_ID),
            Predicate { predicateType: PredicateType::PrefixMatch, data: Default::default() },
        ),
        "http://image_uri.null",
        GuestEnv::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(20000000000000u64),
            maxPrice: U256::from(40000000000000u64),
            biddingStart: now_timestamp(),
            timeout: 100,
            rampUpPeriod: 1,
            lockStake: U256::from(10),
            lockTimeout: 100,
        },
    )
}

#[tokio::test]
async fn test_deposit_withdraw() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Deposit prover balances
    ctx.prover_market.deposit(parse_ether("2").unwrap()).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap(),
        parse_ether("2").unwrap()
    );

    // Withdraw prover balances
    ctx.prover_market.withdraw(parse_ether("2").unwrap()).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of(ctx.prover_signer.address()).await.unwrap(),
        U256::ZERO
    );

    // Withdraw when balance is zero
    assert!(ctx.prover_market.withdraw(parse_ether("2").unwrap()).await.is_err());
}

#[tokio::test]
#[traced_test]
async fn test_deposit_withdraw_stake() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let mut ctx = create_test_ctx(&anvil).await.unwrap();

    let deposit = U256::from(10);

    // set stake balance alerts
    ctx.prover_market =
        ctx.prover_market.with_stake_balance_alert(&Some(U256::from(10)), &Some(U256::from(5)));

    // Approve and deposit stake
    ctx.prover_market.approve_deposit_stake(deposit).await.unwrap();
    ctx.prover_market.deposit_stake(deposit).await.unwrap();

    // Deposit stake with permit
    ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    assert_eq!(
        ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
        U256::from(20)
    );

    // Withdraw prover balances in chunks to observe alerts

    ctx.prover_market.withdraw_stake(U256::from(11)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
        U256::from(9)
    );

    ctx.prover_market.withdraw_stake(U256::from(5)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
        U256::from(4)
    );

    ctx.prover_market.withdraw_stake(U256::from(4)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_stake(ctx.prover_signer.address()).await.unwrap(),
        U256::ZERO
    );

    // Withdraw when balance is zero
    assert!(ctx.prover_market.withdraw_stake(U256::from(20)).await.is_err());
}

#[tokio::test]
async fn test_submit_request() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    let request = new_request(1, &ctx).await;

    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs and check if the event was emitted
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (log, _) = logs.first().unwrap();
    assert!(log.requestId == request_id);
}

#[tokio::test]
#[traced_test]
async fn test_e2e() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    let eip712_domain = eip712_domain! {
        name: "IBoundlessMarket",
        version: "1",
        chain_id: anvil.chain_id(),
        verifying_contract: *ctx.customer_market.instance().address(),
    };

    let request = new_request(1, &ctx).await;
    let expires_at = request.expires_at();

    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs to retrieve the customer signature from the event
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (_, log) = logs.first().unwrap();
    let tx_hash = log.transaction_hash.unwrap();
    let tx_data = ctx
        .customer_market
        .instance()
        .provider()
        .get_transaction_by_hash(tx_hash)
        .await
        .unwrap()
        .unwrap();
    let inputs = tx_data.input();
    let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs).unwrap();

    let request = calldata.request;
    let customer_sig = calldata.clientSignature;

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) =
        mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

    // publish the committed root
    ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

    let assessor_fill = AssessorReceipt {
        seal: assessor_seal,
        selectors: vec![],
        prover: ctx.prover_signer.address(),
        callbacks: vec![],
    };
    // fulfill the request
    ctx.prover_market
        .fulfill(FulfillmentTx::new(vec![fulfillment.clone()], assessor_fill.clone()))
        .await
        .unwrap();
    assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());

    // retrieve journal and seal from the fulfilled request
    let (journal, seal) = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    assert_eq!(journal, fulfillment.journal);
    assert_eq!(seal, fulfillment.seal);
}

#[tokio::test]
async fn test_e2e_merged_submit_fulfill() {
    // Setup anvil
    let anvil = Anvil::new().spawn();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    let eip712_domain = eip712_domain! {
        name: "IBoundlessMarket",
        version: "1",
        chain_id: anvil.chain_id(),
        verifying_contract: *ctx.customer_market.instance().address(),
    };

    let request = new_request(1, &ctx).await;
    let expires_at = request.expires_at();

    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs to retrieve the customer signature from the event
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (_, log) = logs.first().unwrap();
    let tx_hash = log.transaction_hash.unwrap();
    let tx_data = ctx
        .customer_market
        .instance()
        .provider()
        .get_transaction_by_hash(tx_hash)
        .await
        .unwrap()
        .unwrap();
    let inputs = tx_data.input();
    let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs).unwrap();

    let request = calldata.request;
    let customer_sig = calldata.clientSignature;

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) =
        mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

    let fulfillments = vec![fulfillment];
    let assessor_fill = AssessorReceipt {
        seal: assessor_seal,
        selectors: vec![],
        prover: ctx.prover_signer.address(),
        callbacks: vec![],
    };
    // publish the committed root + fulfillments
    ctx.prover_market
        .fulfill(FulfillmentTx::new(fulfillments.clone(), assessor_fill.clone()).with_submit_root(
            ctx.set_verifier_address,
            root,
            set_verifier_seal,
        ))
        .await
        .unwrap();

    // retrieve journal and seal from the fulfilled request
    let (journal, seal) = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    assert_eq!(journal, fulfillments[0].journal);
    assert_eq!(seal, fulfillments[0].seal);
}

#[tokio::test]
async fn test_e2e_price_and_fulfill_batch() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    let eip712_domain = eip712_domain! {
        name: "IBoundlessMarket",
        version: "1",
        chain_id: anvil.chain_id(),
        verifying_contract: *ctx.customer_market.instance().address(),
    };

    let request = new_request(1, &ctx).await;
    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs to retrieve the customer signature from the event
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (_, log) = logs.first().unwrap();
    let tx_hash = log.transaction_hash.unwrap();
    let tx_data = ctx
        .customer_market
        .instance()
        .provider()
        .get_transaction_by_hash(tx_hash)
        .await
        .unwrap()
        .unwrap();
    let inputs = tx_data.input();
    let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs).unwrap();

    let request = calldata.request;
    let customer_sig = calldata.clientSignature;

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) =
        mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

    let fulfillments = vec![fulfillment];
    let assessor_fill = AssessorReceipt {
        seal: assessor_seal,
        selectors: vec![],
        prover: ctx.prover_signer.address(),
        callbacks: vec![],
    };

    // Price and fulfill the request
    ctx.prover_market
        .fulfill(
            FulfillmentTx::new(fulfillments.clone(), assessor_fill.clone())
                .with_submit_root(ctx.set_verifier_address, root, set_verifier_seal)
                .with_unlocked_request(UnlockedRequest::new(request, customer_sig)),
        )
        .await
        .unwrap();

    // retrieve journal and seal from the fulfilled request
    let (journal, seal) = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    assert_eq!(journal, fulfillments[0].journal);
    assert_eq!(seal, fulfillments[0].seal);
}

#[tokio::test]
async fn test_e2e_no_payment() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    let eip712_domain = eip712_domain! {
        name: "IBoundlessMarket",
        version: "1",
        chain_id: anvil.chain_id(),
        verifying_contract: *ctx.customer_market.instance().address(),
    };

    let request = new_request(1, &ctx).await;
    let expires_at = request.expires_at();

    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs to retrieve the customer signature from the event
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (_, log) = logs.first().unwrap();
    let tx_hash = log.transaction_hash.unwrap();
    let tx_data = ctx
        .customer_market
        .instance()
        .provider()
        .get_transaction_by_hash(tx_hash)
        .await
        .unwrap()
        .unwrap();
    let inputs = tx_data.input();
    let calldata = IBoundlessMarket::submitRequestCall::abi_decode(inputs).unwrap();

    let request = calldata.request;
    let customer_sig = calldata.clientSignature;

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_stake_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(&request, &customer_sig, None).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // Test behavior when payment requirements are not met.
    {
        // mock the fulfillment, using the wrong prover address. Address::from(3) arbitrary.
        let some_other_address = Address::from(U160::from(3));
        let (root, set_verifier_seal, fulfillment, assessor_seal) =
            mock_singleton(&request, eip712_domain.clone(), some_other_address);

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: some_other_address,
            callbacks: vec![],
        };

        let balance_before = ctx.prover_market.balance_of(some_other_address).await.unwrap();
        // fulfill the request.
        ctx.prover_market
            .fulfill(FulfillmentTx::new(vec![fulfillment.clone()], assessor_fill.clone()))
            .await
            .unwrap();
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());
        let balance_after = ctx.prover_market.balance_of(some_other_address).await.unwrap();
        assert!(balance_before == balance_after);

        // retrieve journal and seal from the fulfilled request
        let (journal, seal) =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

        assert_eq!(journal, fulfillment.journal);
        assert_eq!(seal, fulfillment.seal);
    }

    // mock the fulfillment, this time using the right prover address.
    let (root, set_verifier_seal, fulfillment, assessor_seal) =
        mock_singleton(&request, eip712_domain, ctx.prover_signer.address());

    // publish the committed root
    ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

    let assessor_fill = AssessorReceipt {
        seal: assessor_seal,
        selectors: vec![],
        prover: ctx.prover_signer.address(),
        callbacks: vec![],
    };

    // fulfill the request, this time getting paid.
    ctx.prover_market
        .fulfill(FulfillmentTx::new(vec![fulfillment.clone()], assessor_fill.clone()))
        .await
        .unwrap();
    assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());

    // retrieve journal and seal from the fulfilled request
    let (_journal, _seal) = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    // TODO: Instead of checking that this is the same seal, check if this is some valid seal.
    // When there are multiple fulfillments one order, there will be multiple ProofDelivered
    // events. All proofs will be valid though.
    //assert_eq!(journal, fulfillment.journal);
    //assert_eq!(seal, fulfillment.seal);
}
