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

mod common;

use std::str::FromStr;

use alloy::{
    primitives::{Bytes, U256, utils::parse_ether},
    providers::{Provider, ext::AnvilApi},
    rpc::types::BlockNumberOrTag,
};
use boundless_cli::{DefaultProver, OrderFulfilled};
use boundless_indexer::{
    db::{market::{RequestStatusType, SlashedStatus, SortDirection}, DbError},
    test_utils::TestDb,
};
use boundless_market::contracts::{
    boundless_market::FulfillmentTx, Offer, Predicate, ProofRequest, RequestId, RequestInput,
    Requirements,
};
use boundless_test_utils::{
    guests::{ASSESSOR_GUEST_ELF, ECHO_ID, ECHO_PATH, SET_BUILDER_ELF},
    market::{create_test_ctx, TestCtx},
};
use sqlx::{AnyPool, Row};
use tracing_test::traced_test;

use common::*;

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_e2e() {
    let fixture = common::new_market_test_fixture().await.unwrap();
    
    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .start_block("0")
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    tracing::info!("Starting test at timestamp: {}", now);

    // Create, submit, lock, and fulfill request
    let (request, client_sig) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;

    fixture.ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    
    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request, client_sig.clone()).await.unwrap();

    wait_for_indexer().await;

    // Verify all events were indexed
    let request_id_str = format!("{:x}", request.id);
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_requests").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_submitted_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_locked_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_delivered_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proofs").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_fulfilled_events").await;

    // Verify hourly aggregation
    let summary_count = count_table_rows(&fixture.test_db.pool, "hourly_market_summary").await;
    assert!(summary_count >= 1, "Expected at least one hourly summary, got {}", summary_count);

    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    verify_hour_boundaries(&summaries);
    
    // Verify totals across all summaries (events might be in different hours)
    verify_summary_totals(&summaries, SummaryExpectations {
        total_requests_submitted: Some(1),
        total_requests_onchain: Some(1),
        total_requests_offchain: Some(0),
        total_locked: Some(1),
        total_slashed: Some(0),
        total_fulfilled: Some(1),
        total_expired: Some(0),
        total_locked_and_expired: Some(0),
        total_locked_and_fulfilled: Some(1),
    });

    let request_summary = summaries.iter().find(|s| s.total_requests_submitted > 0)
        .expect("Expected to find a summary with submitted requests");
    assert_eq!(request_summary.total_requests_submitted, 1, "Expected 1 submitted request");
    assert_eq!(request_summary.unique_requesters_submitting_requests, 1, "Expected 1 unique requester");
    assert_eq!(request_summary.total_requests_submitted_onchain, 1, "Expected 1 onchain request");
    
    // Find the summary with fulfilled requests and verify it
    let fulfilled_summary = summaries.iter().find(|s| s.total_fulfilled > 0)
        .expect("Expected to find a summary with fulfilled requests");
    assert_eq!(fulfilled_summary.total_fulfilled, 1, "Expected 1 fulfilled request");
    assert_eq!(fulfilled_summary.unique_provers_locking_requests, 1, "Expected 1 unique prover");

    // Verify fees and collateral are present
    assert!(!fulfilled_summary.total_fees_locked.is_empty(), "total_fees_locked should not be empty");
    assert!(!fulfilled_summary.total_collateral_locked.is_empty(), "total_collateral_locked should not be empty");
    
    assert_eq!(fulfilled_summary.locked_orders_fulfillment_rate, 100.0, "Expected 100% fulfillment rate");
    
    // Verify cycle counts (will be 0 since test uses random address)
    assert_eq!(fulfilled_summary.total_program_cycles, 0, "Expected 0 total_program_cycles for non-hardcoded requestor");
    assert_eq!(fulfilled_summary.total_cycles, 0, "Expected 0 total_cycles for non-hardcoded requestor");

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_monitoring() {
    let fixture = common::new_market_test_fixture().await.unwrap();
    
    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    let monitor = indexer_monitor::monitor::Monitor::new(&fixture.test_db.db_url).await.unwrap();

    // Create first request and let it expire
    let mut now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let (request, client_sig) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    let first_request_id = format!("{:x}", request.id);

    fixture.ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    fixture.ctx.prover_market.lock_request(&request, client_sig, None).await.unwrap();

    // Verify no expired requests yet
    let expired = monitor
        .fetch_requests_expired((now - 30) as i64, now as i64)
        .await
        .unwrap();
    assert_eq!(expired.len(), 0);

    // Wait for the request to expire
    loop {
        now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
        if now > request.expires_at() {
            break;
        }
        wait_for_indexer().await;
    }

    // Verify expired requests
    let expired = monitor.fetch_requests_expired((now - 30) as i64, now as i64).await.unwrap();
    assert_eq!(expired.len(), 1);
    let expired = monitor
        .fetch_requests_expired_from((now - 30) as i64, now as i64, fixture.ctx.customer_signer.address())
        .await
        .unwrap();
    assert_eq!(expired.len(), 1);

    // Slash the expired request
    fixture.ctx.prover_market.slash(request.id).await.unwrap();

    // Create and fulfill second request
    now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let (request, client_sig) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        2,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;

    fixture.ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request, client_sig.clone()).await.unwrap();

    wait_for_indexer().await;

    // Verify monitor metrics
    now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    assert_eq!(monitor.total_requests().await.unwrap(), 2);
    assert_eq!(monitor.fetch_requests(0, now as i64).await.unwrap().len(), 2);
    assert_eq!(monitor.total_requests_from_client(fixture.ctx.customer_signer.address()).await.unwrap(), 2);
    assert_eq!(monitor.total_proofs().await.unwrap(), 1);
    assert_eq!(monitor.total_proofs_from_client(fixture.ctx.customer_signer.address()).await.unwrap(), 1);
    assert_eq!(monitor.total_slashed().await.unwrap(), 1);
    assert_eq!(monitor.total_slashed_by_prover(fixture.ctx.prover_signer.address()).await.unwrap(), 1);
    assert_eq!(monitor.total_success_rate_from_client(fixture.ctx.customer_signer.address()).await.unwrap(), Some(0.5));
    assert_eq!(monitor.total_success_rate_by_prover(fixture.ctx.prover_signer.address()).await.unwrap(), Some(0.5));

    // Verify aggregation
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(!summaries.is_empty(), "Expected at least one hourly summary");
    
    verify_hour_boundaries(&summaries);
    verify_summary_totals(&summaries, SummaryExpectations {
        total_requests_submitted: Some(2),
        total_requests_onchain: Some(2),
        total_requests_offchain: Some(0),
        total_locked: Some(2),
        total_slashed: Some(1),
        total_fulfilled: Some(1),
        total_expired: Some(1),
        total_locked_and_expired: Some(1),
        total_locked_and_fulfilled: Some(1),
    });

    // Verify collateral tracking
    let expired_request_collateral = get_lock_collateral(&fixture.test_db.pool, &first_request_id).await;
    let total_locked_and_expired_collateral: U256 = summaries
        .iter()
        .map(|s| U256::from_str(&s.total_locked_and_expired_collateral).unwrap_or(U256::ZERO))
        .sum();
    let expected_collateral = U256::from_str(&expired_request_collateral).unwrap_or(U256::ZERO);
    assert_eq!(
        total_locked_and_expired_collateral,
        expected_collateral,
        "total_locked_and_expired_collateral should match the lock_collateral from the expired request"
    );

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_across_hours() {
    let fixture = common::new_market_test_fixture().await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let one_eth = parse_ether("1").unwrap().into();

    // Create and fulfill first order with cycle counts
    let request1 = ProofRequest::new(
        RequestId::new(fixture.ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: one_eth,
            maxPrice: one_eth,
            rampUpStart: now - 3,
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig1 = request1.sign_request(&fixture.ctx.customer_signer, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).await.unwrap();

    fixture.ctx.customer_market.deposit(one_eth * U256::from(2)).await.unwrap();
    fixture.ctx.customer_market
        .submit_request_with_signature(&request1, Bytes::from(client_sig1.as_bytes()))
        .await
        .unwrap();

    let request1_digest = request1.signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).unwrap();
    let program_cycles1 = 50_000_000; // 50M cycles
    let total_cycles1 = (program_cycles1 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&fixture.test_db, request1_digest, program_cycles1).await.unwrap();

    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request1, client_sig1.as_bytes().into()).await.unwrap();

    // Advance time by more than an hour
    advance_time_and_mine(&fixture.ctx.customer_provider, 3700, 1).await.unwrap();
    let now2 = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and fulfill second order in a different hour
    let request2 = ProofRequest::new(
        RequestId::new(fixture.ctx.customer_signer.address(), 2),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: one_eth,
            maxPrice: one_eth,
            rampUpStart: now2 - 3,
            timeout: 12,
            rampUpPeriod: 1,
            lockTimeout: 12,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig2 = request2.sign_request(&fixture.ctx.customer_signer, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).await.unwrap();

    fixture.ctx.customer_market
        .submit_request_with_signature(&request2, Bytes::from(client_sig2.as_bytes()))
        .await
        .unwrap();

    let request2_digest = request2.signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).unwrap();
    let program_cycles2 = 100_000_000; // 100M cycles
    let total_cycles2 = (program_cycles2 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&fixture.test_db, request2_digest, program_cycles2).await.unwrap();

    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request2, client_sig2.as_bytes().into()).await.unwrap();

    let now3 = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    advance_time_and_mine(&fixture.ctx.customer_provider, 3700, 1).await.unwrap();

    wait_for_indexer().await;

    // Check hourly aggregation across multiple hours
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(summaries.len() >= 2, "Expected at least 2 different hours of data");

    verify_hour_boundaries(&summaries);
    
    // Verify hour timestamps are different and at least 1 hour apart
    let hour1 = summaries[0].period_timestamp;
    let hour2 = summaries[1].period_timestamp;
    assert_ne!(hour1, hour2, "Expected different hour timestamps");
    assert!(hour2.saturating_sub(hour1) >= 3600, "Expected at least 1 hour difference");

    // Verify summary totals
    verify_summary_totals(&summaries, SummaryExpectations {
        total_requests_submitted: Some(2),
        total_requests_onchain: Some(2),
        total_requests_offchain: Some(0),
        total_locked: Some(2),
        total_slashed: Some(0),
        total_fulfilled: Some(2),
        total_expired: Some(0),
        total_locked_and_expired: Some(0),
        total_locked_and_fulfilled: Some(2),
    });

    // Verify hour boundary formula
    let expected_hour1 = (now / 3600) * 3600;
    let expected_hour2 = (now2 / 3600) * 3600;
    assert!(summaries.iter().any(|s| s.period_timestamp == expected_hour1));
    assert!(summaries.iter().any(|s| s.period_timestamp == expected_hour2));

    // Verify fee metrics are populated for fulfilled requests
    let summaries_with_fulfilled: Vec<_> = summaries.iter().filter(|s| s.total_fulfilled > 0).collect();
    assert!(!summaries_with_fulfilled.is_empty());
    for summary in summaries_with_fulfilled {
        assert_ne!(summary.total_fees_locked.trim_start_matches('0'), "");
        assert_ne!(summary.p50_lock_price_per_cycle.trim_start_matches('0'), "");
    }

    // Verify cycle count aggregations
    let total_cycles_across_hours: u64 = summaries.iter().map(|s| s.total_cycles).sum();
    let total_program_cycles_across_hours: u64 = summaries.iter().map(|s| s.total_program_cycles).sum();
    let expected_total_cycles = total_cycles1 + total_cycles2;
    let expected_total_program_cycles = program_cycles1 + program_cycles2;
    assert_eq!(total_cycles_across_hours, expected_total_cycles);
    assert_eq!(total_program_cycles_across_hours, expected_total_program_cycles);

    // Verify per-hour cycle counts
    let hours_with_fulfilled: Vec<_> = summaries.iter().filter(|s| s.total_fulfilled > 0).collect();
    assert_eq!(hours_with_fulfilled.len(), 2, "Expected exactly 2 hours with fulfilled requests");
    assert_eq!(hours_with_fulfilled[0].total_program_cycles, program_cycles1);
    assert_eq!(hours_with_fulfilled[0].total_cycles, total_cycles1);
    assert_eq!(hours_with_fulfilled[1].total_program_cycles, program_cycles2);
    assert_eq!(hours_with_fulfilled[1].total_cycles, total_cycles2);

    // Verify p50_lock_price_per_cycle calculation
    let expected_p50_request1 = U256::from(one_eth) / U256::from(program_cycles1);
    let expected_p50_request2 = U256::from(one_eth) / U256::from(program_cycles2);
    let p50_hour1: U256 = hours_with_fulfilled[0].p50_lock_price_per_cycle.trim_start_matches('0').parse().expect("Failed to parse p50");
    let p50_hour2: U256 = hours_with_fulfilled[1].p50_lock_price_per_cycle.trim_start_matches('0').parse().expect("Failed to parse p50");
    assert_eq!(p50_hour1, expected_p50_request1, "First hour p50 mismatch");
    assert_eq!(p50_hour2, expected_p50_request2, "Second hour p50 mismatch");

    tracing::info!("Hour 1 p50: {} (expected {}), Hour 2 p50: {} (expected {})", p50_hour1, expected_p50_request1, p50_hour2, expected_p50_request2);

    cli_process.kill().unwrap();
}

#[tokio::test]
#[traced_test]
#[ignore = "Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_percentiles() {
    // Test multiple requests with different prices to validate percentile calculations
    // Creates 10 requests with prices from 0.1 ETH to 1.0 ETH
    // All requests use 100M cycles for simplicity

    let fixture = common::new_market_test_fixture().await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    wait_for_indexer().await;

    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let num_requests = 10;
    let base_price = U256::from(100_000_000_000_000_000u128); // 0.1 ETH
    let cycles_per_request = 100_000_000u64; // 100M cycles for all requests
    let total_cycles_per_request = (cycles_per_request as f64 * 1.0158) as u64;

    // Deposit enough to cover all requests
    let total_deposit = U256::from(1_000_000_000_000_000_000u128) * U256::from(num_requests);
    fixture.ctx.customer_market.deposit(total_deposit).await.unwrap();

    let mut request_digests = Vec::new();

    for i in 0..num_requests {
        let request_id = i + 1;
        let max_price = base_price * U256::from(request_id); // 0.1 ETH, 0.2 ETH, ..., 1.0 ETH

        let request = ProofRequest::new(
            RequestId::new(fixture.ctx.customer_signer.address(), request_id as u32),
            Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
            format!("file://{ECHO_PATH}"),
            RequestInput::builder().build_inline().unwrap(),
            Offer {
                minPrice: max_price,
                maxPrice: max_price,
                rampUpStart: now - 3,
                timeout: 300,
                rampUpPeriod: 1,
                lockTimeout: 300,
                lockCollateral: U256::from(0),
            },
        );

        let client_sig = request.sign_request(&fixture.ctx.customer_signer, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).await.unwrap();

        fixture.ctx.customer_market
            .submit_request_with_signature(&request, Bytes::from(client_sig.as_bytes()))
            .await
            .unwrap();

        let digest = request.signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).unwrap();
        insert_cycle_counts_with_overhead(&fixture.test_db, digest, cycles_per_request).await.unwrap();
        
        fixture.ctx.prover_market.lock_request(&request, Bytes::from(client_sig.as_bytes()), None).await.unwrap();
        request_digests.push((request.clone(), client_sig, digest));
    }

    // Fulfill all requests
    let fulfillment_requests: Vec<_> = request_digests.iter()
        .map(|(req, sig, _)| (req.clone(), sig.as_bytes().to_vec().into()))
        .collect();

    for chunk in fulfillment_requests.chunks(5) {
        let (fill, root_receipt, assessor_receipt) = fixture.prover.fulfill(chunk).await.unwrap();
        let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
        fixture.ctx.prover_market
            .fulfill(
                FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                    .with_submit_root(
                        fixture.ctx.deployment.set_verifier_address,
                        order_fulfilled.root,
                        order_fulfilled.seal,
                    )
            )
            .await
            .unwrap();
    }

    wait_for_indexer().await;

    // Verify percentile calculations
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    tracing::info!("Retrieved {} hourly summaries", summaries.len());

    let fulfilled_summary = summaries.iter()
        .find(|s| s.total_fulfilled == num_requests as u64)
        .expect("Should have exactly one hour with all 10 fulfilled requests");

    tracing::info!(
        "Found hour with {} fulfilled requests: p10={}, p50={}, p90={}",
        fulfilled_summary.total_fulfilled,
        fulfilled_summary.p10_lock_price_per_cycle,
        fulfilled_summary.p50_lock_price_per_cycle,
        fulfilled_summary.p90_lock_price_per_cycle
    );

    // Parse percentile values
    let actual_p10: U256 = fulfilled_summary.p10_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);
    let actual_p50: U256 = fulfilled_summary.p50_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);
    let actual_p90: U256 = fulfilled_summary.p90_lock_price_per_cycle.trim_start_matches('0').parse().unwrap_or(U256::ZERO);

    // Verify percentiles are in expected ranges
    let min_price_per_cycle = U256::from(1_000_000_000u64); // 0.1 ETH / 100M
    let max_price_per_cycle = U256::from(10_000_000_000u64); // 1.0 ETH / 100M

    assert!(actual_p10 >= min_price_per_cycle && actual_p10 <= U256::from(2_000_000_000u64), "p10 out of range: {}", actual_p10);
    assert!(actual_p50 >= U256::from(4_000_000_000u64) && actual_p50 <= U256::from(6_000_000_000u64), "p50 out of range: {}", actual_p50);
    assert!(actual_p90 >= U256::from(8_000_000_000u64) && actual_p90 <= max_price_per_cycle, "p90 out of range: {}", actual_p90);

    // Verify total cycles
    let expected_total_cycles = total_cycles_per_request * num_requests as u64;
    assert_eq!(fulfilled_summary.total_cycles, expected_total_cycles, "Total cycles mismatch");

    tracing::info!("All percentiles are in expected ranges: p10={}, p50={}, p90={}", actual_p10, actual_p50, actual_p90);

    cli_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_indexer_with_order_stream(pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();

    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, pool.clone()).await;

    let (block_timestamp, _) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 3 orders to order stream
    let (req1, _) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    order_stream_client.submit_request(&req1, &fixture.ctx.customer_signer).await.unwrap();
    wait_for_indexer().await;

    let (req2, _) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 2, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    order_stream_client.submit_request(&req2, &fixture.ctx.customer_signer).await.unwrap();
    wait_for_indexer().await;

    let (req3, _) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 3, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    order_stream_client.submit_request(&req3, &fixture.ctx.customer_signer).await.unwrap();

    update_order_timestamps(&pool, block_timestamp).await;

    let orders = order_stream_client.list_orders_v2(None, None, None, None, None).await.unwrap();
    assert_eq!(orders.orders.len(), 3, "Expected 3 orders to be indexed");

    advance_time_and_mine(&fixture.ctx.customer_provider, 10, 1).await.unwrap();

    // Start market indexer with order stream URL
    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .start_block("1")
    .order_stream_url(order_stream_url.as_str())
    .spawn()
    .unwrap();

    wait_for_indexer().await;

    // Verify all 3 orders were indexed as offchain
    assert_eq!(count_table_rows(&fixture.test_db.pool, "proof_requests").await, 3);
    let offchain_count = sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
        .fetch_one(&fixture.test_db.pool)
        .await
        .unwrap()
        .get::<i64, _>("count");
    assert_eq!(offchain_count, 3, "Expected all 3 requests to be offchain");

    // Verify order stream state and tx_hash
    let timestamp: Option<String> = sqlx::query("SELECT last_processed_timestamp FROM order_stream_state")
        .fetch_one(&fixture.test_db.pool)
        .await
        .unwrap()
        .get("last_processed_timestamp");
    assert!(timestamp.is_some());

    let tx_hash: String = sqlx::query("SELECT tx_hash FROM proof_requests WHERE request_id = $1")
        .bind(format!("{:x}", req1.id))
        .fetch_one(&fixture.test_db.pool)
        .await
        .unwrap()
        .get("tx_hash");
    assert_eq!(tx_hash, "0000000000000000000000000000000000000000000000000000000000000000");

    // Verify aggregation includes offchain requests
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(!summaries.is_empty());

    verify_summary_totals(&summaries, SummaryExpectations {
        total_requests_submitted: Some(3),
        total_requests_onchain: Some(0),
        total_requests_offchain: Some(3),
        ..Default::default()
    });

    let unique_requesters: u64 = summaries.iter().map(|s| s.unique_requesters_submitting_requests).max().unwrap_or(0);
    assert_eq!(unique_requesters, 1);

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_offchain_and_onchain_mixed_aggregation(pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();

    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, pool.clone()).await;

    let (block_timestamp, start_block) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 2 onchain requests
    let (req1_onchain, sig1) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req1_onchain, sig1).await.unwrap();

    let (req2_onchain, sig2) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 2, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    fixture.ctx.customer_market.submit_request_with_signature(&req2_onchain, sig2).await.unwrap();

    // Submit 3 offchain requests
    for id in [10, 11, 12] {
        let (req, _) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), id, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
        order_stream_client.submit_request(&req, &fixture.ctx.customer_signer).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    update_order_timestamps(&pool, block_timestamp).await;

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .start_block(&start_block.to_string())
    .order_stream_url(order_stream_url.as_str())
    .spawn()
    .unwrap();

    wait_for_indexer().await;

    // Verify all 5 orders were indexed (2 onchain + 3 offchain)
    assert_eq!(count_table_rows(&fixture.test_db.pool, "proof_requests").await, 5);
    
    let onchain_count = sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'onchain'")
        .fetch_one(&fixture.test_db.pool).await.unwrap().get::<i64, _>("count");
    let offchain_count = sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
        .fetch_one(&fixture.test_db.pool).await.unwrap().get::<i64, _>("count");
    assert_eq!(onchain_count, 2);
    assert_eq!(offchain_count, 3);

    // Verify aggregation
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(!summaries.is_empty());

    verify_summary_totals(&summaries, SummaryExpectations {
        total_requests_submitted: Some(5),
        total_requests_onchain: Some(2),
        total_requests_offchain: Some(3),
        ..Default::default()
    });

    let unique_requesters: u64 = summaries.iter().map(|s| s.unique_requesters_submitting_requests).max().unwrap_or(0);
    assert_eq!(unique_requesters, 1);

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_submission_timestamp_field(pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();

    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, pool.clone()).await;

    let (block_timestamp, start_block) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 1 onchain request
    let (req_onchain, sig_onchain) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req_onchain, sig_onchain).await.unwrap();

    let onchain_block_timestamp = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 1 offchain request
    let (req_offchain, _) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 10, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    order_stream_client.submit_request(&req_offchain, &fixture.ctx.customer_signer).await.unwrap();

    update_order_timestamps(&pool, block_timestamp).await;

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .start_block(&start_block.to_string())
    .order_stream_url(order_stream_url.as_str())
    .spawn()
    .unwrap();

    wait_for_indexer().await;

    // Verify onchain request timestamps
    let result = sqlx::query("SELECT block_timestamp, submission_timestamp, source FROM proof_requests WHERE request_id = $1")
        .bind(format!("{:x}", req_onchain.id))
        .fetch_one(&fixture.test_db.pool)
        .await
        .unwrap();
    let onchain_block_ts: i64 = result.get("block_timestamp");
    let onchain_submission_ts: i64 = result.get("submission_timestamp");
    let onchain_source: String = result.get("source");
    assert_eq!(onchain_source, "onchain");
    assert_eq!(onchain_submission_ts, onchain_block_ts);
    assert_eq!(onchain_block_ts, onchain_block_timestamp as i64);

    // Verify offchain request timestamps
    let result = sqlx::query("SELECT block_timestamp, submission_timestamp, source FROM proof_requests WHERE request_id = $1")
        .bind(format!("{:x}", req_offchain.id))
        .fetch_one(&fixture.test_db.pool)
        .await
        .unwrap();
    let offchain_block_ts: i64 = result.get("block_timestamp");
    let offchain_submission_ts: i64 = result.get("submission_timestamp");
    let offchain_source: String = result.get("source");
    assert_eq!(offchain_source, "offchain");
    assert_eq!(offchain_block_ts, 0);
    assert!(offchain_submission_ts > 0);

    // Verify aggregation includes both
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    let total_requests_submitted: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    assert_eq!(total_requests_submitted, 2);

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[tokio::test]
#[traced_test]
async fn test_both_tx_fetch_strategies_produce_same_results() {
    let fixture = common::new_market_test_fixture().await.unwrap();
    let test_db_receipts = TestDb::new().await.unwrap();
    let test_db_tx_by_hash = TestDb::new().await.unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit and fulfill 1 request
    let (request, client_sig) = create_order(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now).await;
    
    fixture.ctx.customer_market.deposit(U256::from(1)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&request, client_sig.clone()).await.unwrap();
    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request, client_sig.clone()).await.unwrap();

    // Mine blocks and get end block
    fixture.ctx.customer_provider.anvil_mine(Some(2), None).await.unwrap();
    let end_block = fixture.ctx.customer_provider.get_block_number().await.unwrap().to_string();

    // Run indexer with block-receipts strategy
    let mut cli_process_receipts = IndexerCliBuilder::new(
        test_db_receipts.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("3")
    .batch_size("100")
    .tx_fetch_strategy("block-receipts")
    .start_block("0")
    .end_block(&end_block)
    .spawn()
    .unwrap();

    wait_for_indexer().await;
    cli_process_receipts.kill().ok();
    wait_for_indexer().await;

    // Run indexer with tx-by-hash strategy
    let mut cli_process_tx_by_hash = IndexerCliBuilder::new(
        test_db_tx_by_hash.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("3")
    .batch_size("100")
    .tx_fetch_strategy("tx-by-hash")
    .start_block("0")
    .end_block(&end_block)
    .spawn()
    .unwrap();

    wait_for_indexer().await;
    cli_process_tx_by_hash.kill().ok();
    wait_for_indexer().await;

    // Compare results from both databases
    let count_receipts = count_table_rows(&test_db_receipts.pool, "transactions").await;
    let count_tx_by_hash = count_table_rows(&test_db_tx_by_hash.pool, "transactions").await;
    assert_eq!(count_receipts, count_tx_by_hash, "Transaction counts should match");
    assert!(count_receipts > 0);

    // 2. Compare transactions table data (tx_hash, from_address, block_number, block_timestamp, transaction_index)
    let txs_receipts: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
    )
    .fetch_all(&test_db_receipts.pool)
    .await
    .unwrap();

    let txs_tx_by_hash: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
        "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
    )
    .fetch_all(&test_db_tx_by_hash.pool)
    .await
    .unwrap();

    assert_eq!(txs_receipts.len(), txs_tx_by_hash.len(), "Should have same number of transactions");

    // Compare each transaction
    for (i, (tx_receipts, tx_tx_by_hash)) in
        txs_receipts.iter().zip(txs_tx_by_hash.iter()).enumerate()
    {
        assert_eq!(tx_receipts.0, tx_tx_by_hash.0, "Transaction {} tx_hash should match", i);
        assert_eq!(tx_receipts.1, tx_tx_by_hash.1, "Transaction {} from_address should match", i);
        assert_eq!(tx_receipts.2, tx_tx_by_hash.2, "Transaction {} block_number should match", i);
        assert_eq!(
            tx_receipts.3, tx_tx_by_hash.3,
            "Transaction {} block_timestamp should match",
            i
        );
        assert_eq!(
            tx_receipts.4, tx_tx_by_hash.4,
            "Transaction {} transaction_index should match",
            i
        );
    }

    // Compare proof requests count
    let count_requests_receipts = count_table_rows(&test_db_receipts.pool, "proof_requests").await;
    let count_requests_tx_by_hash = count_table_rows(&test_db_tx_by_hash.pool, "proof_requests").await;
    assert_eq!(count_requests_receipts, count_requests_tx_by_hash, "Proof request counts should match");
    assert_eq!(count_requests_receipts, 1);
}

// Helper struct for request status data
#[derive(Debug)]
struct RequestStatusRow {
    request_digest: String,
    request_id: String,
    request_status: String,
    slashed_status: String,
    source: String,
    created_at: i64,
    updated_at: i64,
    locked_at: Option<i64>,
    fulfilled_at: Option<i64>,
    slashed_at: Option<i64>,
    lock_end: i64,
    slash_recipient: Option<String>,
    slash_transferred_amount: Option<String>,
    slash_burned_amount: Option<String>,
}

async fn get_lock_collateral(pool: &AnyPool, request_id: &str) -> String {
    let row = sqlx::query(
        "SELECT lock_collateral FROM request_status WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .unwrap();
    row.get("lock_collateral")
}

async fn get_request_status(pool: &AnyPool, request_id: &str) -> RequestStatusRow {
    let row = sqlx::query(
        "SELECT request_digest, request_id, request_status, slashed_status, source, created_at, updated_at,
                locked_at, fulfilled_at, slashed_at, lock_end, slash_recipient,
                slash_transferred_amount, slash_burned_amount
         FROM request_status
         WHERE request_id = $1",
    )
    .bind(request_id)
    .fetch_one(pool)
    .await
    .unwrap();

    RequestStatusRow {
        request_digest: row.get("request_digest"),
        request_id: row.get("request_id"),
        request_status: row.get("request_status"),
        slashed_status: row.get("slashed_status"),
        source: row.get("source"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        locked_at: row.get("locked_at"),
        fulfilled_at: row.get("fulfilled_at"),
        slashed_at: row.get("slashed_at"),
        lock_end: row.get("lock_end"),
        slash_recipient: row.get("slash_recipient"),
        slash_transferred_amount: row.get("slash_transferred_amount"),
        slash_burned_amount: row.get("slash_burned_amount"),
    }
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_happy_path(_pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();
    
    let block = fixture.ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("3")
    .start_block(&start_block.to_string())
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and submit request
    let (req, sig) = create_order_with_timeouts(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now, 1000000, 1000000).await;
    
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();
    wait_for_indexer().await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());
    assert_eq!(status.source, "onchain");
    assert!(status.locked_at.is_none());
    assert!(status.fulfilled_at.is_none());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig.clone(), None).await.unwrap();
    wait_for_indexer().await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());
    assert!(status.locked_at.is_some());
    assert_eq!(status.lock_end, req.offer.rampUpStart as i64 + req.offer.lockTimeout as i64);

    // Fulfill request (already locked, so use fulfill_request instead of lock_and_fulfill_request)
    fulfill_request(&fixture.ctx, &fixture.prover, &req, sig.clone()).await.unwrap();
    wait_for_indexer().await;

    // Verify fulfilled status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());

    indexer_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_locked_then_expired(_pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();
    
    let block = fixture.ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("3")
    .start_block(&start_block.to_string())
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and submit request with short timeout
    let (req, sig) = create_order_with_timeouts(&fixture.ctx.customer_signer, fixture.ctx.customer_signer.address(), 1, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id(), now, 1000, 1000).await;
    
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();
    wait_for_indexer().await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig.clone(), None).await.unwrap();
    wait_for_indexer().await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());

    // Advance time past request expiration
    let expires_at = req.expires_at();
    tracing::info!("Request expires at: {}.", expires_at);
    fixture.ctx.customer_provider.anvil_set_next_block_timestamp(expires_at).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();
    wait_for_indexer().await;

    // Verify expired status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Expired.to_string());

    indexer_process.kill().unwrap();
}

#[sqlx::test(migrations = "../order-stream/migrations")]
#[traced_test]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_lock_expired_then_slashed(_pool: sqlx::PgPool) {
    let fixture = common::new_market_test_fixture().await.unwrap();
    
    let block = fixture.ctx.customer_provider.get_block_by_number(BlockNumberOrTag::Latest).await.unwrap().unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("3")
    .start_block(&start_block.to_string())
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create request with short lock timeout
    let req = ProofRequest::new(
        RequestId::new(fixture.ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: U256::from(0),
            maxPrice: U256::from(1),
            rampUpStart: now - 3,
            timeout: 7200,
            rampUpPeriod: 1,
            lockTimeout: 1800,
            lockCollateral: U256::from(0),
        },
    );
    let sig = req.sign_request(&fixture.ctx.customer_signer, fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id()).await.unwrap();
    let sig_bytes: Bytes = sig.as_bytes().into();

    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig_bytes.clone()).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(3), None).await.unwrap();
    wait_for_indexer().await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig_bytes.clone(), None).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(2), None).await.unwrap();
    wait_for_indexer().await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());
    let lock_end = status.lock_end;

    // Advance time past lock expiration and fulfill late
    fixture.ctx.customer_provider.anvil_set_next_block_timestamp(lock_end as u64 + 1).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();
    wait_for_indexer().await;

    // Fulfill request (late fulfillment)
    let (fill, root_receipt, assessor_receipt) = fixture.prover.fulfill(&[(req.clone(), sig_bytes.clone())]).await.unwrap();
    let order_fulfilled = OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    fixture.ctx.prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(fixture.ctx.deployment.set_verifier_address, order_fulfilled.root, order_fulfilled.seal),
        )
        .await
        .unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(2), None).await.unwrap();
    wait_for_indexer().await;

    // Verify fulfilled status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());

    // Advance time and slash
    fixture.ctx.customer_provider.anvil_set_next_block_timestamp(req.expires_at() + 1).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();
    wait_for_indexer().await;
    fixture.ctx.prover_market.slash(req.id).await.unwrap();
    wait_for_indexer().await;

    // Verify slashed status (fulfilled takes priority)
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert_eq!(status.slashed_status, SlashedStatus::Slashed.to_string());
    assert!(status.fulfilled_at.is_some());
    assert!(status.slashed_at.is_some());
    assert!(status.slash_recipient.is_some());
    assert!(status.slash_transferred_amount.is_some());
    assert!(status.slash_burned_amount.is_some());

    indexer_process.kill().unwrap();
}
