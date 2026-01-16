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

use std::sync::Arc;

use alloy::{
    node_bindings::Anvil,
    primitives::{aliases::U160, utils::parse_ether, Address, U256},
    providers::Provider,
    sol_types::eip712_domain,
};
use alloy_primitives::Bytes;
use boundless_market::{
    client::{ClientBuilder, FundingMode},
    contracts::{
        boundless_market::{FulfillmentTx, UnlockedRequest},
        hit_points::default_allowance,
        AssessorReceipt, FulfillmentData, FulfillmentDataType, Offer, Predicate, ProofRequest,
        RequestId, RequestStatus, Requirements,
    },
    indexer_client::IndexerClient,
    input::GuestEnv,
    price_provider::{PricePercentiles, PriceProviderArc},
    request_builder::{OfferParams, RequestParams},
    storage::MockStorageProvider,
    test_helpers::create_mock_indexer_client,
};
use boundless_test_utils::{
    guests::{ECHO_ELF, ECHO_ID},
    market::{create_test_ctx, mock_singleton, TestCtx},
};
use risc0_zkvm::{
    sha::{Digest, Digestible},
    ReceiptClaim,
};
use tracing_test::traced_test;
use url::Url;

fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

async fn new_request<P: Provider>(idx: u32, ctx: &TestCtx<P>) -> ProofRequest {
    ProofRequest::new(
        RequestId::new(ctx.customer_signer.address(), idx),
        Requirements::new(Predicate::prefix_match(Digest::from(ECHO_ID), Bytes::default())),
        "http://image_uri.null",
        GuestEnv::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(20000000000000u64),
            maxPrice: U256::from(40000000000000u64),
            rampUpStart: now_timestamp(),
            timeout: 100,
            rampUpPeriod: 1,
            lockCollateral: U256::from(10),
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
    ctx.prover_market = ctx
        .prover_market
        .with_collateral_balance_alert(&Some(U256::from(10)), &Some(U256::from(5)));

    // Approve and deposit stake
    ctx.prover_market.approve_deposit_collateral(deposit).await.unwrap();
    ctx.prover_market.deposit_collateral(deposit).await.unwrap();

    // Deposit stake with permit
    ctx.prover_market.deposit_collateral_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    assert_eq!(
        ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await.unwrap(),
        U256::from(20)
    );

    // Withdraw prover balances in chunks to observe alerts

    ctx.prover_market.withdraw_collateral(U256::from(11)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await.unwrap(),
        U256::from(9)
    );

    ctx.prover_market.withdraw_collateral(U256::from(5)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await.unwrap(),
        U256::from(4)
    );

    ctx.prover_market.withdraw_collateral(U256::from(4)).await.unwrap();
    assert_eq!(
        ctx.prover_market.balance_of_collateral(ctx.prover_signer.address()).await.unwrap(),
        U256::ZERO
    );

    // Withdraw when balance is zero
    assert!(ctx.prover_market.withdraw_collateral(U256::from(20)).await.is_err());
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
async fn test_funding_mode() {
    use boundless_market::client::ClientBuilder;
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();
    // Setup client with Always funding mode as it is the default mode
    let client = ClientBuilder::new()
        .with_signer(ctx.customer_signer.clone())
        .with_deployment(ctx.deployment.clone())
        .with_rpc_url(Url::parse(&anvil.endpoint()).unwrap())
        .build()
        .await
        .unwrap();

    // Test Always funding mode: balance after submission should increase by maxPrice
    let balance_before =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    let request = new_request(1, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == balance_before + request.offer.maxPrice);

    // Test AvailableBalance funding mode: balance after submission should remain the same
    let client = client.with_funding_mode(FundingMode::AvailableBalance);
    let balance_before =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    let request = new_request(2, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == balance_before);

    // Test BelowThreshold funding mode: balance after submission should be equal to threshold
    // since we set the threshold above the current balance
    let threshold = request.offer.maxPrice * U256::from(5);
    let client = client.with_funding_mode(FundingMode::BelowThreshold(threshold));
    let request = new_request(3, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == threshold);

    // Withdraw all balance
    ctx.customer_market.withdraw(balance_after).await.unwrap();

    // Test Never funding mode: balance after submission should remain the same
    let client = client.with_funding_mode(FundingMode::Never);
    let request = new_request(4, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == U256::ZERO);

    // Test MinMaxBalance funding mode: balance after submission should be equal to max
    // since we start from zero balance after the withdrawal above
    let min = request.offer.maxPrice;
    let max = request.offer.maxPrice * U256::from(10);
    let client =
        client.with_funding_mode(FundingMode::MinMaxBalance { min_balance: min, max_balance: max });
    let request = new_request(5, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == max);

    // Test MinMaxBalance funding mode: balance after submission should remain equal to max
    // since we are above the min balance
    let request = new_request(6, &ctx).await;
    let _ = client.submit_request(&request).await.unwrap();
    let balance_after =
        ctx.customer_market.balance_of(ctx.customer_signer.address()).await.unwrap();
    assert!(balance_after == max);
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

    let (event, _) = logs.first().unwrap();
    let request = &event.request;
    let customer_sig = event.clientSignature.clone();

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_collateral_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(request, customer_sig).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
        request,
        eip712_domain,
        ctx.prover_signer.address(),
        FulfillmentDataType::ImageIdAndJournal,
    );

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

    // retrieve fulfillment data data and seal from the fulfilled request
    let fulfillment_result = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();
    let expected_fulfillment_data = FulfillmentData::decode_with_type(
        fulfillment.fulfillmentDataType,
        fulfillment.fulfillmentData.clone(),
    )
    .unwrap();
    let fulfillment_data = fulfillment_result.data().unwrap();
    assert_eq!(fulfillment_data, expected_fulfillment_data);
    assert_eq!(fulfillment_result.seal, fulfillment.seal);
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

    let (event, _) = logs.first().unwrap();
    let request = &event.request;
    let customer_sig = event.clientSignature.clone();

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_collateral_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(request, customer_sig).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
        request,
        eip712_domain,
        ctx.prover_signer.address(),
        FulfillmentDataType::ImageIdAndJournal,
    );

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
            ctx.deployment.set_verifier_address,
            root,
            set_verifier_seal,
        ))
        .await
        .unwrap();

    // retrieve fulfillment data  and seal from the fulfilled request
    let fulfillment_result = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();
    let expected_fulfillment_data = FulfillmentData::decode_with_type(
        fulfillments[0].fulfillmentDataType,
        fulfillments[0].fulfillmentData.clone(),
    )
    .unwrap();
    let fulfillment_data = fulfillment_result.data().unwrap();
    assert_eq!(fulfillment_data, expected_fulfillment_data);
    assert_eq!(fulfillment_result.seal, fulfillments[0].seal);
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

    let (event, _) = logs.first().unwrap();
    let request = &event.request;
    let customer_sig = &event.clientSignature;

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
        request,
        eip712_domain,
        ctx.prover_signer.address(),
        FulfillmentDataType::ImageIdAndJournal,
    );

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
                .with_submit_root(ctx.deployment.set_verifier_address, root, set_verifier_seal)
                .with_unlocked_request(UnlockedRequest::new(request.clone(), customer_sig.clone())),
        )
        .await
        .unwrap();

    // retrieve callback data and seal from the fulfilled request
    let fulfillment_result = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    let expected_fulfillment_data = FulfillmentData::decode_with_type(
        fulfillments[0].fulfillmentDataType,
        fulfillments[0].fulfillmentData.clone(),
    )
    .unwrap();
    let fulfillment_data = fulfillment_result.data().unwrap();
    assert_eq!(fulfillment_data, expected_fulfillment_data);
    assert_eq!(fulfillment_result.seal, fulfillments[0].seal);
}

#[tokio::test]
#[traced_test]
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

    let (event, _) = logs.first().unwrap();
    let request = &event.request;
    let customer_sig = event.clientSignature.clone();

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_collateral_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(request, customer_sig).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // Test behavior when payment requirements are not met.
    {
        // mock the fulfillment, using the wrong prover address. Address::from(3) arbitrary.
        let some_other_address = Address::from(U160::from(3));
        let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
            request,
            eip712_domain.clone(),
            some_other_address,
            FulfillmentDataType::ImageIdAndJournal,
        );

        // publish the committed root
        ctx.set_verifier.submit_merkle_root(root, set_verifier_seal).await.unwrap();

        let assessor_fill = AssessorReceipt {
            seal: assessor_seal,
            selectors: vec![],
            prover: some_other_address,
            callbacks: vec![],
        };

        let balance_before = ctx.prover_market.balance_of(some_other_address).await.unwrap();
        // fulfill the request. This call emits a PaymentRequirementsFailed log since the lock
        // belongs to a different prover, but the request itself still becomes fulfilled on-chain.
        ctx.prover_market
            .fulfill(FulfillmentTx::new(vec![fulfillment.clone()], assessor_fill.clone()))
            .await
            .expect("fulfillment should succeed even if payment requirements fail");
        assert!(logs_contain("Payment requirements failed for at least one fulfillment"));
        assert!(ctx.customer_market.is_fulfilled(request_id).await.unwrap());
        let balance_after = ctx.prover_market.balance_of(some_other_address).await.unwrap();
        assert!(balance_before == balance_after);

        // retrieve fulfillment data and seal from the fulfilled request
        let fulfillment_result =
            ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();
        let expected_fulfillment_data = FulfillmentData::decode_with_type(
            fulfillment.fulfillmentDataType,
            fulfillment.fulfillmentData.clone(),
        )
        .unwrap();
        let fulfillment_data = fulfillment_result.data().unwrap();
        assert_eq!(fulfillment_data, expected_fulfillment_data);
        assert_eq!(fulfillment_result.seal, fulfillment.seal);
    }

    // mock the fulfillment, this time using the right prover address.
    let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
        request,
        eip712_domain,
        ctx.prover_signer.address(),
        FulfillmentDataType::ImageIdAndJournal,
    );

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
    let _fulfillment = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();

    // TODO: Instead of checking that this is the same seal, check if this is some valid seal.
    // When there are multiple fulfillments one order, there will be multiple ProofDelivered
    // events. All proofs will be valid though.
    //assert_eq!(journal, fulfillment.journal);
    //assert_eq!(seal, fulfillment.seal);
}

#[tokio::test]
#[traced_test]
async fn test_e2e_claim_digest_no_fulfillment_data() {
    // Setup anvil
    let anvil = Anvil::new().spawn();

    let ctx = create_test_ctx(&anvil).await.unwrap();

    let eip712_domain = eip712_domain! {
        name: "IBoundlessMarket",
        version: "1",
        chain_id: anvil.chain_id(),
        verifying_contract: *ctx.customer_market.instance().address(),
    };

    let claim_digest = ReceiptClaim::ok(ECHO_ID, vec![0x41, 0x41, 0x41, 0x41]).digest();

    let request = {
        let mut request = new_request(1, &ctx).await;
        request.requirements =
            request.requirements.with_predicate(Predicate::claim_digest_match(claim_digest));

        request
    };
    let expires_at = request.expires_at();

    let request_id =
        ctx.customer_market.submit_request(&request, &ctx.customer_signer).await.unwrap();

    // fetch logs to retrieve the customer signature from the event
    let logs = ctx.customer_market.instance().RequestSubmitted_filter().query().await.unwrap();

    let (event, _) = logs.first().unwrap();
    let request = &event.request;
    let customer_sig = event.clientSignature.clone();

    // Deposit prover balances
    let deposit = default_allowance();
    ctx.prover_market.deposit_collateral_with_permit(deposit, &ctx.prover_signer).await.unwrap();

    // Lock the request
    ctx.prover_market.lock_request(request, customer_sig).await.unwrap();
    assert!(ctx.customer_market.is_locked(request_id).await.unwrap());
    assert!(
        ctx.customer_market.get_status(request_id, Some(expires_at)).await.unwrap()
            == RequestStatus::Locked
    );

    // mock the fulfillment
    let (root, set_verifier_seal, fulfillment, assessor_seal) = mock_singleton(
        request,
        eip712_domain,
        ctx.prover_signer.address(),
        FulfillmentDataType::None,
    );

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

    // retrieve fulfillment data and seal from the fulfilled request
    let fulfillment_result = ctx.customer_market.get_request_fulfillment(request_id).await.unwrap();
    let expected_fulfillment_data = FulfillmentData::decode_with_type(
        fulfillment.fulfillmentDataType,
        fulfillment.fulfillmentData.clone(),
    )
    .unwrap();
    let fulfillment_data = fulfillment_result.data().unwrap();
    assert_eq!(fulfillment_data, expected_fulfillment_data);
    assert_eq!(fulfillment_result.seal, fulfillment.seal);
}

#[tokio::test]
#[traced_test]
async fn test_offer_params_explicit_prices_override_provider() {
    let cycle_count = 1_000_000u64;

    // Test that explicit prices in OfferParams override the provider
    let explicit_min = U256::from(2000u64) * U256::from(cycle_count);
    let explicit_max = U256::from(6000u64) * U256::from(cycle_count);
    let offer_params_explicit: OfferParams =
        OfferParams::builder().min_price(explicit_min).max_price(explicit_max).into();

    // When prices are explicitly set, they should be used regardless of provider
    assert_eq!(offer_params_explicit.min_price.unwrap(), explicit_min);
    assert_eq!(offer_params_explicit.max_price.unwrap(), explicit_max);
}

#[tokio::test]
#[traced_test]
async fn test_client_builder_with_price_provider() {
    // Setup anvil
    let anvil = Anvil::new().spawn();
    let ctx = create_test_ctx(&anvil).await.unwrap();

    // Use a storage provider to avoid URL fetching issues
    let storage = Arc::new(MockStorageProvider::start());

    // Create a mock IndexerClient that returns specific prices
    // This allows us to test the price provider integration without a real indexer
    let price_percentiles = PricePercentiles {
        p10: U256::from(1000u64),
        p25: U256::from(2000u64),
        p50: U256::from(3000u64),
        p75: U256::from(4000u64),
        p90: U256::from(5000u64),
        p95: U256::from(6000u64),
        p99: U256::from(7000u64),
    };
    let (mock_server, _mock_indexer_client) = create_mock_indexer_client(&price_percentiles);

    // Create an IndexerClient from the mock server URL and use it as a price provider
    let mock_indexer_url = Url::parse(mock_server.base_url().as_str()).unwrap();
    let mock_indexer_client = IndexerClient::new(mock_indexer_url).unwrap();
    let price_provider: PriceProviderArc = Arc::new(mock_indexer_client);

    // Test that ClientBuilder accepts a price_provider parameter and uses it
    let client = ClientBuilder::new()
        .with_signer(ctx.customer_signer.clone())
        .with_deployment(ctx.deployment.clone())
        .with_rpc_url(Url::parse(&anvil.endpoint()).unwrap())
        .with_storage_provider(Some(storage.clone()))
        .with_price_provider(Some(price_provider))
        .build()
        .await
        .unwrap();

    // Build a request WITHOUT explicit prices - should use prices from the mock indexer
    let request_params = RequestParams::new().with_program(ECHO_ELF).with_stdin(b"test");

    // Build the request - price provider should succeed and use mock prices
    let request = client.build_request(request_params).await.unwrap();
    // Verify that prices were set using the mock price provider
    assert!(request.offer.minPrice > price_percentiles.p10);
    assert!(request.offer.maxPrice > price_percentiles.p90);
}
