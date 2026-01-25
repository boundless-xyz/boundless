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

#![allow(clippy::zombie_processes)]

use std::str::FromStr;

use alloy::{
    primitives::{utils::parse_ether, Bytes, B256, U256},
    providers::{ext::AnvilApi, Provider},
    rpc::types::BlockNumberOrTag,
};
use boundless_cli::OrderFulfilled;
use boundless_indexer::db::{
    market::{RequestStatusType, SlashedStatus, SortDirection},
    IndexerDb, ProversDb, RequestorDb,
};
use boundless_market::contracts::{
    boundless_market::FulfillmentTx, Offer, Predicate, ProofRequest, RequestId, RequestInput,
    Requirements,
};
use boundless_test_utils::guests::{ECHO_ID, ECHO_PATH};
use sqlx::{PgPool, Row};

use super::common::*;

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_e2e(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

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

    // Create, submit, lock, and fulfill request with non-zero collateral
    let collateral_amount = U256::from(1000);
    let (request, client_sig) = create_order_with_collateral(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
        collateral_amount,
    )
    .await;

    // Submit request with automatic deposit if needed
    submit_request_with_deposit(&fixture.ctx, &request, client_sig.clone()).await.unwrap();

    // Use helper that handles collateral deposits
    lock_and_fulfill_request_with_collateral(
        &fixture.ctx,
        &fixture.prover,
        &request,
        client_sig.clone(),
    )
    .await
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify all events were indexed
    let request_id_str = format!("{:x}", request.id);
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_requests").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_submitted_events")
        .await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_locked_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_delivered_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proofs").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_fulfilled_events")
        .await;

    // Verify hourly aggregation
    let summary_count = count_table_rows(&fixture.test_db.pool, "hourly_market_summary").await;
    assert!(summary_count >= 1, "Expected at least one hourly summary, got {}", summary_count);

    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    verify_hour_boundaries(&summaries);

    // Verify totals across all summaries (events might be in different hours)
    verify_summary_totals(
        &summaries,
        SummaryExpectations {
            total_requests_submitted: Some(1),
            total_requests_onchain: Some(1),
            total_requests_offchain: Some(0),
            total_locked: Some(1),
            total_slashed: Some(0),
            total_fulfilled: Some(1),
            total_expired: Some(0),
            total_locked_and_expired: Some(0),
            total_locked_and_fulfilled: Some(1),
            total_secondary_fulfillments: Some(0),
        },
    );

    let request_summary = summaries
        .iter()
        .find(|s| s.total_requests_submitted > 0)
        .expect("Expected to find a summary with submitted requests");
    assert_eq!(request_summary.total_requests_submitted, 1, "Expected 1 submitted request");
    assert_eq!(
        request_summary.unique_requesters_submitting_requests, 1,
        "Expected 1 unique requester"
    );
    assert_eq!(request_summary.total_requests_submitted_onchain, 1, "Expected 1 onchain request");

    // Find the summary with fulfilled requests and verify it
    let fulfilled_summary = summaries
        .iter()
        .find(|s| s.total_fulfilled > 0)
        .expect("Expected to find a summary with fulfilled requests");
    assert_eq!(fulfilled_summary.total_fulfilled, 1, "Expected 1 fulfilled request");
    assert_eq!(fulfilled_summary.unique_provers_locking_requests, 1, "Expected 1 unique prover");

    // Verify fees and collateral are present
    assert_ne!(
        fulfilled_summary.total_fees_locked,
        U256::ZERO,
        "total_fees_locked should not be zero"
    );
    assert_eq!(
        fulfilled_summary.total_collateral_locked, collateral_amount,
        "total_collateral_locked should match the request's lockCollateral"
    );

    assert_eq!(
        fulfilled_summary.locked_orders_fulfillment_rate, 100.0,
        "Expected 100% fulfillment rate"
    );

    // Verify cycle counts (will be 0 since test uses random address)
    assert_eq!(
        fulfilled_summary.total_program_cycles,
        U256::ZERO,
        "Expected 0 total_program_cycles for non-hardcoded requestor"
    );
    assert_eq!(
        fulfilled_summary.total_cycles,
        U256::ZERO,
        "Expected 0 total_cycles for non-hardcoded requestor"
    );

    // Verify collateral across all aggregation periods
    // Sum collateral from all hourly summaries
    let total_hourly_collateral: U256 = summaries.iter().map(|s| s.total_collateral_locked).sum();
    assert_eq!(
        total_hourly_collateral, collateral_amount,
        "Sum of hourly collateral should match request collateral"
    );

    // Verify daily aggregation
    let daily_summaries = fixture
        .test_db
        .db
        .get_daily_market_summaries(None, 10000, SortDirection::Asc, None, None)
        .await
        .unwrap();
    let total_daily_collateral: U256 =
        daily_summaries.iter().map(|s| s.total_collateral_locked).sum();
    assert_eq!(
        total_daily_collateral, collateral_amount,
        "Sum of daily collateral should match request collateral"
    );

    // Verify all-time aggregation
    let all_time_summaries = get_all_time_summaries(&fixture.test_db.pool).await;
    assert!(!all_time_summaries.is_empty(), "Expected at least one all-time summary");

    // Check that all-time collateral matches the sum of hourly collateral
    let latest_all_time = all_time_summaries.last().unwrap();
    assert_eq!(
        latest_all_time.total_collateral_locked, total_hourly_collateral,
        "All-time total_collateral_locked should match sum of hourly collateral"
    );
    assert_eq!(
        latest_all_time.total_collateral_locked, collateral_amount,
        "All-time total_collateral_locked should match request collateral"
    );

    // Verify that locked_and_expired_collateral is zero (request was fulfilled, not expired)
    assert_eq!(
        latest_all_time.total_locked_and_expired_collateral,
        U256::ZERO,
        "total_locked_and_expired_collateral should be zero for fulfilled requests"
    );

    // Verify collateral is cumulative in all-time summaries
    for i in 1..all_time_summaries.len() {
        let prev = &all_time_summaries[i - 1];
        let curr = &all_time_summaries[i];
        assert!(
            curr.total_collateral_locked >= prev.total_collateral_locked,
            "All-time collateral should be cumulative"
        );
    }

    cli_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_monitoring(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
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
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request, client_sig.clone())
        .await
        .unwrap();
    fixture.ctx.prover_market.lock_request(&request, client_sig).await.unwrap();

    // Verify no expired requests yet
    let expired = monitor.fetch_requests_expired((now - 30) as i64, now as i64).await.unwrap();
    assert_eq!(expired.len(), 0);

    advance_time_to_and_mine(&fixture.ctx.customer_provider, request.expires_at() + 1, 1)
        .await
        .unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify expired requests
    now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let expired = monitor.fetch_requests_expired((now - 30) as i64, now as i64).await.unwrap();
    assert_eq!(expired.len(), 1);
    let expired = monitor
        .fetch_requests_expired_from(
            (now - 30) as i64,
            now as i64,
            fixture.ctx.customer_signer.address(),
        )
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
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request, client_sig.clone())
        .await
        .unwrap();
    lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request, client_sig.clone())
        .await
        .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify monitor metrics
    now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    assert_eq!(monitor.total_requests().await.unwrap(), 2);
    assert_eq!(monitor.fetch_requests(0, now as i64).await.unwrap().len(), 2);
    assert_eq!(
        monitor.total_requests_from_client(fixture.ctx.customer_signer.address()).await.unwrap(),
        2
    );
    assert_eq!(monitor.total_proofs().await.unwrap(), 1);
    assert_eq!(
        monitor.total_proofs_from_client(fixture.ctx.customer_signer.address()).await.unwrap(),
        1
    );
    assert_eq!(monitor.total_slashed().await.unwrap(), 1);
    assert_eq!(
        monitor.total_slashed_by_prover(fixture.ctx.prover_signer.address()).await.unwrap(),
        1
    );
    assert_eq!(
        monitor
            .total_success_rate_from_client(fixture.ctx.customer_signer.address())
            .await
            .unwrap(),
        Some(0.5)
    );
    assert_eq!(
        monitor.total_success_rate_by_prover(fixture.ctx.prover_signer.address()).await.unwrap(),
        Some(0.5)
    );

    // Verify aggregation
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(!summaries.is_empty(), "Expected at least one hourly summary");

    verify_hour_boundaries(&summaries);
    verify_summary_totals(
        &summaries,
        SummaryExpectations {
            total_requests_submitted: Some(2),
            total_requests_onchain: Some(2),
            total_requests_offchain: Some(0),
            total_locked: Some(2),
            total_slashed: Some(1),
            total_fulfilled: Some(1),
            total_expired: Some(1),
            total_locked_and_expired: Some(1),
            total_locked_and_fulfilled: Some(1),
            total_secondary_fulfillments: Some(0), // Not a secondary fulfillment
        },
    );

    // Verify collateral tracking
    let expired_request_collateral =
        get_lock_collateral(&fixture.test_db.pool, &first_request_id).await;
    let total_locked_and_expired_collateral: U256 =
        summaries.iter().map(|s| s.total_locked_and_expired_collateral).sum();
    let expected_collateral = U256::from_str(&expired_request_collateral).unwrap_or(U256::ZERO);
    assert_eq!(
        total_locked_and_expired_collateral,
        expected_collateral,
        "total_locked_and_expired_collateral should match the lock_collateral from the expired request"
    );

    cli_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_across_hours(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let one_eth = parse_ether("1").unwrap();

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
            timeout: 24,
            rampUpPeriod: 1,
            lockTimeout: 24,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig1 = request1
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();

    fixture.ctx.customer_market.deposit(one_eth * U256::from(2)).await.unwrap();
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request1, Bytes::from(client_sig1.as_bytes()))
        .await
        .unwrap();

    let request1_digest = request1
        .signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id())
        .unwrap();
    let program_cycles1 = 50_000_000; // 50M cycles
    let total_cycles1 = (program_cycles1 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&fixture.test_db, request1_digest, program_cycles1)
        .await
        .unwrap();

    lock_and_fulfill_request(
        &fixture.ctx,
        &fixture.prover,
        &request1,
        client_sig1.as_bytes().into(),
    )
    .await
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

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
    let client_sig2 = request2
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();

    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request2, Bytes::from(client_sig2.as_bytes()))
        .await
        .unwrap();

    let request2_digest = request2
        .signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id())
        .unwrap();
    let program_cycles2 = 100_000_000; // 100M cycles
    let total_cycles2 = (program_cycles2 as f64 * 1.0158) as u64;
    insert_cycle_counts_with_overhead(&fixture.test_db, request2_digest, program_cycles2)
        .await
        .unwrap();

    lock_and_fulfill_request(
        &fixture.ctx,
        &fixture.prover,
        &request2,
        client_sig2.as_bytes().into(),
    )
    .await
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    let _now3 = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    advance_time_and_mine(&fixture.ctx.customer_provider, 3700, 1).await.unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

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
    verify_summary_totals(
        &summaries,
        SummaryExpectations {
            total_requests_submitted: Some(2),
            total_requests_onchain: Some(2),
            total_requests_offchain: Some(0),
            total_locked: Some(2),
            total_slashed: Some(0),
            total_fulfilled: Some(2),
            total_expired: Some(0),
            total_locked_and_expired: Some(0),
            total_locked_and_fulfilled: Some(2),
            total_secondary_fulfillments: Some(0), // Not secondary fulfillments (fulfilled before lock_end)
        },
    );

    // Verify hour boundary formula
    let expected_hour1 = (now / 3600) * 3600;
    let expected_hour2 = (now2 / 3600) * 3600;
    assert!(summaries.iter().any(|s| s.period_timestamp == expected_hour1));
    assert!(summaries.iter().any(|s| s.period_timestamp == expected_hour2));

    // Verify fee metrics are populated for fulfilled requests
    let summaries_with_fulfilled: Vec<_> =
        summaries.iter().filter(|s| s.total_fulfilled > 0).collect();
    assert!(!summaries_with_fulfilled.is_empty());
    for summary in summaries_with_fulfilled {
        assert_ne!(summary.total_fees_locked, U256::ZERO);
        assert_ne!(summary.p50_lock_price_per_cycle, U256::ZERO);
    }

    // Verify cycle count aggregations
    let total_cycles_across_hours: U256 = summaries.iter().map(|s| s.total_cycles).sum();
    let total_program_cycles_across_hours: U256 =
        summaries.iter().map(|s| s.total_program_cycles).sum();
    let expected_total_cycles = U256::from(total_cycles1 + total_cycles2);
    let expected_total_program_cycles = U256::from(program_cycles1 + program_cycles2);
    assert_eq!(total_cycles_across_hours, expected_total_cycles);
    assert_eq!(total_program_cycles_across_hours, expected_total_program_cycles);

    // Verify per-hour cycle counts
    let hours_with_fulfilled: Vec<_> = summaries.iter().filter(|s| s.total_fulfilled > 0).collect();
    assert_eq!(hours_with_fulfilled.len(), 2, "Expected exactly 2 hours with fulfilled requests");
    assert_eq!(hours_with_fulfilled[0].total_program_cycles, U256::from(program_cycles1));
    assert_eq!(hours_with_fulfilled[0].total_cycles, U256::from(total_cycles1));
    assert_eq!(hours_with_fulfilled[1].total_program_cycles, U256::from(program_cycles2));
    assert_eq!(hours_with_fulfilled[1].total_cycles, U256::from(total_cycles2));

    // Verify p50_lock_price_per_cycle calculation
    let expected_p50_request1 = U256::from(one_eth) / U256::from(program_cycles1);
    let expected_p50_request2 = U256::from(one_eth) / U256::from(program_cycles2);
    let p50_hour1: U256 = hours_with_fulfilled[0].p50_lock_price_per_cycle;
    let p50_hour2: U256 = hours_with_fulfilled[1].p50_lock_price_per_cycle;
    assert_eq!(p50_hour1, expected_p50_request1, "First hour p50 mismatch");
    assert_eq!(p50_hour2, expected_p50_request2, "Second hour p50 mismatch");

    tracing::info!(
        "Hour 1 p50: {} (expected {}), Hour 2 p50: {} (expected {})",
        p50_hour1,
        expected_p50_request1,
        p50_hour2,
        expected_p50_request2
    );

    // Verify all-time aggregates
    let all_time_summaries = get_all_time_summaries(&fixture.test_db.pool).await;
    assert!(!all_time_summaries.is_empty(), "Expected at least one all-time summary");

    // Verify all-time aggregates are cumulative (each hour should have >= previous hour's values)
    for i in 1..all_time_summaries.len() {
        let prev = &all_time_summaries[i - 1];
        let curr = &all_time_summaries[i];

        assert!(
            curr.period_timestamp > prev.period_timestamp,
            "All-time summaries should be in chronological order"
        );
        assert!(
            curr.total_fulfilled >= prev.total_fulfilled,
            "Total fulfilled should be cumulative"
        );
        assert!(
            curr.total_requests_submitted >= prev.total_requests_submitted,
            "Total requests submitted should be cumulative"
        );
        assert!(
            curr.total_program_cycles >= prev.total_program_cycles,
            "Total program cycles should be cumulative"
        );
        assert!(curr.total_cycles >= prev.total_cycles, "Total cycles should be cumulative");
    }

    // Verify all-time aggregates match sum of hourly aggregates
    let latest_all_time = all_time_summaries.last().unwrap();
    let total_hourly_fulfilled: u64 = summaries.iter().map(|s| s.total_fulfilled).sum();
    let total_hourly_requests: u64 = summaries.iter().map(|s| s.total_requests_submitted).sum();
    let total_hourly_program_cycles: U256 = summaries.iter().map(|s| s.total_program_cycles).sum();
    let total_hourly_cycles: U256 = summaries.iter().map(|s| s.total_cycles).sum();
    let total_hourly_secondary_fulfillments: u64 =
        summaries.iter().map(|s| s.total_secondary_fulfillments).sum();

    assert_eq!(
        latest_all_time.total_fulfilled, total_hourly_fulfilled,
        "All-time total_fulfilled should match sum of hourly"
    );
    assert_eq!(
        latest_all_time.total_requests_submitted, total_hourly_requests,
        "All-time total_requests_submitted should match sum of hourly"
    );
    assert_eq!(
        latest_all_time.total_program_cycles, total_hourly_program_cycles,
        "All-time total_program_cycles should match sum of hourly"
    );
    assert_eq!(
        latest_all_time.total_cycles, total_hourly_cycles,
        "All-time total_cycles should match sum of hourly"
    );
    assert_eq!(
        latest_all_time.total_secondary_fulfillments, total_hourly_secondary_fulfillments,
        "All-time total_secondary_fulfillments should match sum of hourly"
    );

    // Verify total_secondary_fulfillments is cumulative (non-decreasing)
    for i in 1..all_time_summaries.len() {
        let prev = &all_time_summaries[i - 1];
        let curr = &all_time_summaries[i];
        assert!(
            curr.total_secondary_fulfillments >= prev.total_secondary_fulfillments,
            "Total secondary fulfillments should be cumulative (non-decreasing)"
        );
    }

    // Verify unique counts are correct (should be at least 1 for both provers and requesters)
    assert!(
        latest_all_time.unique_provers_locking_requests >= 1,
        "Should have at least 1 unique prover"
    );
    assert!(
        latest_all_time.unique_requesters_submitting_requests >= 1,
        "Should have at least 1 unique requester"
    );

    // Verify all-time summaries have entries for each hour that has hourly summaries
    // Note: Due to recompute window, we only create all-time aggregates for recent hours
    // So we check that at least the latest hourly summary has a corresponding all-time aggregate
    if let Some(latest_hourly) = summaries.last() {
        let all_time_for_latest_hour = all_time_summaries
            .iter()
            .find(|s| s.period_timestamp == latest_hourly.period_timestamp);
        assert!(
            all_time_for_latest_hour.is_some(),
            "All-time summary should exist for latest hour {}",
            latest_hourly.period_timestamp
        );
    }

    cli_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Slow without RISC0_DEV_MODE=1"]
async fn test_aggregation_percentiles(pool: sqlx::PgPool) {
    // Test multiple requests with different prices to validate percentile calculations
    // Creates 10 requests with prices from 0.1 ETH to 1.0 ETH
    // All requests use 100M cycles for simplicity

    let fixture = new_market_test_fixture(pool).await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

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

        let client_sig = request
            .sign_request(
                &fixture.ctx.customer_signer,
                fixture.ctx.deployment.boundless_market_address,
                fixture.anvil.chain_id(),
            )
            .await
            .unwrap();

        fixture
            .ctx
            .customer_market
            .submit_request_with_signature(&request, Bytes::from(client_sig.as_bytes()))
            .await
            .unwrap();

        let digest = request
            .signing_hash(fixture.ctx.deployment.boundless_market_address, fixture.anvil.chain_id())
            .unwrap();
        insert_cycle_counts_with_overhead(&fixture.test_db, digest, cycles_per_request)
            .await
            .unwrap();

        fixture
            .ctx
            .prover_market
            .lock_request(&request, Bytes::from(client_sig.as_bytes()))
            .await
            .unwrap();
        request_digests.push((request.clone(), client_sig, digest));
    }

    // Fulfill all requests
    let fulfillment_requests: Vec<_> = request_digests
        .iter()
        .map(|(req, sig, _)| (req.clone(), sig.as_bytes().to_vec().into()))
        .collect();

    for chunk in fulfillment_requests.chunks(5) {
        let (fill, root_receipt, assessor_receipt) = fixture.prover.fulfill(chunk).await.unwrap();
        let order_fulfilled =
            OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
        fixture
            .ctx
            .prover_market
            .fulfill(
                FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                    .with_submit_root(
                        fixture.ctx.deployment.set_verifier_address,
                        order_fulfilled.root,
                        order_fulfilled.seal,
                    ),
            )
            .await
            .unwrap();
    }

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify percentile calculations
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    tracing::info!("Retrieved {} hourly summaries", summaries.len());

    let fulfilled_summary = summaries
        .iter()
        .find(|s| s.total_fulfilled == num_requests as u64)
        .expect("Should have exactly one hour with all 10 fulfilled requests");

    tracing::info!(
        "Found hour with {} fulfilled requests: p10={}, p50={}, p90={}",
        fulfilled_summary.total_fulfilled,
        fulfilled_summary.p10_lock_price_per_cycle,
        fulfilled_summary.p50_lock_price_per_cycle,
        fulfilled_summary.p90_lock_price_per_cycle
    );

    // Get percentile values (now U256 directly)
    let actual_p10: U256 = fulfilled_summary.p10_lock_price_per_cycle;
    let actual_p50: U256 = fulfilled_summary.p50_lock_price_per_cycle;
    let actual_p90: U256 = fulfilled_summary.p90_lock_price_per_cycle;

    // Verify percentiles are in expected ranges
    let min_price_per_cycle = U256::from(1_000_000_000u64); // 0.1 ETH / 100M
    let max_price_per_cycle = U256::from(10_000_000_000u64); // 1.0 ETH / 100M

    assert!(
        actual_p10 >= min_price_per_cycle && actual_p10 <= U256::from(2_000_000_000u64),
        "p10 out of range: {}",
        actual_p10
    );
    assert!(
        actual_p50 >= U256::from(4_000_000_000u64) && actual_p50 <= U256::from(6_000_000_000u64),
        "p50 out of range: {}",
        actual_p50
    );
    assert!(
        actual_p90 >= U256::from(8_000_000_000u64) && actual_p90 <= max_price_per_cycle,
        "p90 out of range: {}",
        actual_p90
    );

    // Verify total cycles
    let expected_total_cycles = U256::from(total_cycles_per_request * num_requests as u64);
    assert_eq!(fulfilled_summary.total_cycles, expected_total_cycles, "Total cycles mismatch");

    tracing::info!(
        "All percentiles are in expected ranges: p10={}, p50={}, p90={}",
        actual_p10,
        actual_p50,
        actual_p90
    );

    cli_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_indexer_with_order_stream(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool.clone()).await.unwrap();

    let (_order_stream_db_url, order_stream_pool) = create_isolated_db_pool("order_stream").await;
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, order_stream_pool.clone()).await;

    let (block_timestamp, _) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 3 orders to order stream
    let (req1, _) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req1, &fixture.ctx.customer_signer).await.unwrap();

    let (req2, _) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        2,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req2, &fixture.ctx.customer_signer).await.unwrap();

    let (req3, _) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        3,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req3, &fixture.ctx.customer_signer).await.unwrap();

    update_order_timestamps(&order_stream_pool, block_timestamp).await;

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

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify all 3 orders were indexed as offchain
    assert_eq!(count_table_rows(&fixture.test_db.pool, "proof_requests").await, 3);
    let offchain_count =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
            .fetch_one(&fixture.test_db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
    assert_eq!(offchain_count, 3, "Expected all 3 requests to be offchain");

    // Verify created_at is populated for offchain requests in request_status table
    let status_rows = sqlx::query(
        "SELECT request_id, created_at, source FROM request_status WHERE source = 'offchain'",
    )
    .fetch_all(&fixture.test_db.pool)
    .await
    .unwrap();
    assert_eq!(status_rows.len(), 3, "Expected 3 offchain requests in request_status table");

    for row in status_rows {
        let request_id: String = row.get("request_id");
        let created_at: i64 = row.get("created_at");
        let source: String = row.get("source");
        assert_eq!(source, "offchain");
        assert!(
            created_at > 0,
            "created_at should be populated for offchain request {}, but got {}",
            request_id,
            created_at
        );
        assert!(
            created_at <= now as i64 + 100,
            "created_at should be reasonable for offchain request {}",
            request_id
        );
    }

    // Verify order stream state and tx_hash
    let timestamp: Option<String> =
        sqlx::query("SELECT last_processed_timestamp FROM order_stream_state")
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

    verify_summary_totals(
        &summaries,
        SummaryExpectations {
            total_requests_submitted: Some(3),
            total_requests_onchain: Some(0),
            total_requests_offchain: Some(3),
            ..Default::default()
        },
    );

    let unique_requesters: u64 =
        summaries.iter().map(|s| s.unique_requesters_submitting_requests).max().unwrap_or(0);
    assert_eq!(unique_requesters, 1);

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_offchain_and_onchain_mixed_aggregation(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool.clone()).await.unwrap();

    let (_order_stream_db_url, order_stream_pool) = create_isolated_db_pool("order_stream").await;
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, order_stream_pool.clone()).await;

    let (block_timestamp, start_block) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 2 onchain requests
    let (req1_onchain, sig1) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req1_onchain, sig1).await.unwrap();

    let (req2_onchain, sig2) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        2,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    fixture.ctx.customer_market.submit_request_with_signature(&req2_onchain, sig2).await.unwrap();

    // Submit 3 offchain requests
    for id in [10, 11, 12] {
        let (req, _) = create_order(
            &fixture.ctx.customer_signer,
            fixture.ctx.customer_signer.address(),
            id,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
            now,
        )
        .await;
        order_stream_client.submit_request(&req, &fixture.ctx.customer_signer).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    update_order_timestamps(&order_stream_pool, block_timestamp).await;

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

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify all 5 orders were indexed (2 onchain + 3 offchain)
    assert_eq!(count_table_rows(&fixture.test_db.pool, "proof_requests").await, 5);

    let onchain_count =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'onchain'")
            .fetch_one(&fixture.test_db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
    let offchain_count =
        sqlx::query("SELECT COUNT(*) as count FROM proof_requests WHERE source = 'offchain'")
            .fetch_one(&fixture.test_db.pool)
            .await
            .unwrap()
            .get::<i64, _>("count");
    assert_eq!(onchain_count, 2);
    assert_eq!(offchain_count, 3);

    // Verify aggregation
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    assert!(!summaries.is_empty());

    verify_summary_totals(
        &summaries,
        SummaryExpectations {
            total_requests_submitted: Some(5),
            total_requests_onchain: Some(2),
            total_requests_offchain: Some(3),
            ..Default::default()
        },
    );

    let unique_requesters: u64 =
        summaries.iter().map(|s| s.unique_requesters_submitting_requests).max().unwrap_or(0);
    assert_eq!(unique_requesters, 1);

    cli_process.kill().unwrap();
    order_stream_handle.abort();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Requires PostgreSQL for order stream. Slow without RISC0_DEV_MODE=1"]
async fn test_submission_timestamp_field(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool.clone()).await.unwrap();

    let (_order_stream_db_url, order_stream_pool) = create_isolated_db_pool("order_stream").await;
    let (order_stream_url, order_stream_client, order_stream_handle) =
        setup_order_stream(&fixture.anvil, &fixture.ctx, order_stream_pool.clone()).await;

    let (block_timestamp, start_block) = get_block_info(&fixture.ctx.customer_provider).await;
    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 1 onchain request
    let (req_onchain, sig_onchain) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&req_onchain, sig_onchain)
        .await
        .unwrap();

    let onchain_block_timestamp = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Submit 1 offchain request
    let (req_offchain, _) = create_order(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        10,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
    )
    .await;
    order_stream_client.submit_request(&req_offchain, &fixture.ctx.customer_signer).await.unwrap();

    update_order_timestamps(&order_stream_pool, block_timestamp).await;

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

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

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

// Fails in CI, succeeds locally. TODO.
// #[test_log::test(tokio::test)]
// async fn test_both_tx_fetch_strategies_produce_same_results() {
//     let fixture = new_market_test_fixture().await.unwrap();
//     let test_db_receipts = TestDb::new().await.unwrap();
//     let test_db_tx_by_hash = TestDb::new().await.unwrap();

//     // Verify databases are isolated (different URLs)
//     assert_ne!(
//         test_db_receipts.db_url, test_db_tx_by_hash.db_url,
//         "Databases should have different URLs"
//     );
//     assert_ne!(
//         test_db_receipts.db_url, fixture.test_db.db_url,
//         "Receipts database should be different from fixture database"
//     );
//     assert_ne!(
//         test_db_tx_by_hash.db_url, fixture.test_db.db_url,
//         "Tx-by-hash database should be different from fixture database"
//     );

//     let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

//     // Submit and fulfill 1 request
//     let (request, client_sig) = create_order(
//         &fixture.ctx.customer_signer,
//         fixture.ctx.customer_signer.address(),
//         1,
//         fixture.ctx.deployment.boundless_market_address,
//         fixture.anvil.chain_id(),
//         now,
//     )
//     .await;

//     fixture.ctx.customer_market.deposit(U256::from(1)).await.unwrap();
//     fixture
//         .ctx
//         .customer_market
//         .submit_request_with_signature(&request, client_sig.clone())
//         .await
//         .unwrap();
//     lock_and_fulfill_request(&fixture.ctx, &fixture.prover, &request, client_sig.clone())
//         .await
//         .unwrap();

//     // Mine blocks and get end block
//     fixture.ctx.customer_provider.anvil_mine(Some(2), None).await.unwrap();
//     let end_block = fixture.ctx.customer_provider.get_block_number().await.unwrap().to_string();

//     // Verify both databases are empty before indexing
//     let count_before_receipts = count_table_rows(&test_db_receipts.pool, "transactions").await;
//     let count_before_tx_by_hash = count_table_rows(&test_db_tx_by_hash.pool, "transactions").await;
//     assert_eq!(count_before_receipts, 0, "Receipts database should be empty before indexing");
//     assert_eq!(count_before_tx_by_hash, 0, "Tx-by-hash database should be empty before indexing");

//     // Run indexer with block-receipts strategy
//     let mut cli_process_receipts = IndexerCliBuilder::new(
//         test_db_receipts.db_url.clone(),
//         fixture.anvil.endpoint_url().to_string(),
//         fixture.ctx.deployment.boundless_market_address.to_string(),
//     )
//     .batch_size("100")
//     .tx_fetch_strategy("block-receipts")
//     .start_block("0")
//     .end_block(&end_block)
//     .spawn()
//     .unwrap();

//     // Wait for receipts indexer to finish (using correct database)
//     wait_for_indexer(&fixture.ctx.customer_provider, &test_db_receipts.pool).await;
//     cli_process_receipts.kill().ok();
//     wait_for_indexer(&fixture.ctx.customer_provider, &test_db_receipts.pool).await;

//     // Verify receipts database has data, tx-by-hash database is still empty
//     let count_after_receipts = count_table_rows(&test_db_receipts.pool, "transactions").await;
//     let count_after_tx_by_hash_before =
//         count_table_rows(&test_db_tx_by_hash.pool, "transactions").await;
//     assert!(count_after_receipts > 0, "Receipts database should have data after indexing");
//     assert_eq!(
//         count_after_tx_by_hash_before, 0,
//         "Tx-by-hash database should still be empty before its indexer runs"
//     );

//     // Run indexer with tx-by-hash strategy
//     let mut cli_process_tx_by_hash = IndexerCliBuilder::new(
//         test_db_tx_by_hash.db_url.clone(),
//         fixture.anvil.endpoint_url().to_string(),
//         fixture.ctx.deployment.boundless_market_address.to_string(),
//     )
//     .batch_size("100")
//     .tx_fetch_strategy("tx-by-hash")
//     .start_block("0")
//     .end_block(&end_block)
//     .spawn()
//     .unwrap();

//     // Wait for tx-by-hash indexer to finish (using correct database)
//     wait_for_indexer(&fixture.ctx.customer_provider, &test_db_tx_by_hash.pool).await;
//     cli_process_tx_by_hash.kill().ok();
//     wait_for_indexer(&fixture.ctx.customer_provider, &test_db_tx_by_hash.pool).await;

//     // Verify both databases now have data
//     let count_after_tx_by_hash = count_table_rows(&test_db_tx_by_hash.pool, "transactions").await;
//     assert!(count_after_tx_by_hash > 0, "Tx-by-hash database should have data after indexing");
//     assert_eq!(
//         count_after_receipts, count_after_tx_by_hash,
//         "Both databases should have the same number of transactions"
//     );

//     // Compare results from both databases
//     let count_receipts = count_table_rows(&test_db_receipts.pool, "transactions").await;
//     let count_tx_by_hash = count_table_rows(&test_db_tx_by_hash.pool, "transactions").await;
//     assert_eq!(count_receipts, count_tx_by_hash, "Transaction counts should match");
//     assert!(count_receipts > 0);

//     // 2. Compare transactions table data (tx_hash, from_address, block_number, block_timestamp, transaction_index)
//     let txs_receipts: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
//         "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
//     )
//     .fetch_all(&test_db_receipts.pool)
//     .await
//     .unwrap();

//     let txs_tx_by_hash: Vec<(String, String, i64, i64, i64)> = sqlx::query_as(
//         "SELECT tx_hash, from_address, block_number, block_timestamp, transaction_index FROM transactions ORDER BY tx_hash",
//     )
//     .fetch_all(&test_db_tx_by_hash.pool)
//     .await
//     .unwrap();

//     assert_eq!(txs_receipts.len(), txs_tx_by_hash.len(), "Should have same number of transactions");

//     // Compare each transaction
//     for (i, (tx_receipts, tx_tx_by_hash)) in
//         txs_receipts.iter().zip(txs_tx_by_hash.iter()).enumerate()
//     {
//         assert_eq!(tx_receipts.0, tx_tx_by_hash.0, "Transaction {} tx_hash should match", i);
//         assert_eq!(tx_receipts.1, tx_tx_by_hash.1, "Transaction {} from_address should match", i);
//         assert_eq!(tx_receipts.2, tx_tx_by_hash.2, "Transaction {} block_number should match", i);
//         assert_eq!(
//             tx_receipts.3, tx_tx_by_hash.3,
//             "Transaction {} block_timestamp should match",
//             i
//         );
//         assert_eq!(
//             tx_receipts.4, tx_tx_by_hash.4,
//             "Transaction {} transaction_index should match",
//             i
//         );
//     }

//     // Compare proof requests count
//     let count_requests_receipts = count_table_rows(&test_db_receipts.pool, "proof_requests").await;
//     let count_requests_tx_by_hash =
//         count_table_rows(&test_db_tx_by_hash.pool, "proof_requests").await;
//     assert_eq!(
//         count_requests_receipts, count_requests_tx_by_hash,
//         "Proof request counts should match"
//     );
//     assert_eq!(count_requests_receipts, 1);
// }

// Helper struct for request status data
#[derive(Debug)]
#[allow(dead_code)]
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
    total_cycles: Option<String>,
    effective_prove_mhz: Option<f64>,
    prover_effective_prove_mhz: Option<f64>,
}

async fn get_lock_collateral(pool: &PgPool, request_id: &str) -> String {
    let row = sqlx::query("SELECT lock_collateral FROM request_status WHERE request_id = $1")
        .bind(request_id)
        .fetch_one(pool)
        .await
        .unwrap();
    row.get("lock_collateral")
}

async fn get_request_status(pool: &PgPool, request_id: &str) -> RequestStatusRow {
    let row = sqlx::query(
        "SELECT request_digest, request_id, request_status, slashed_status, source, created_at, updated_at,
                locked_at, fulfilled_at, slashed_at, lock_end, slash_recipient,
                slash_transferred_amount, slash_burned_amount, total_cycles,
                effective_prove_mhz_v2 as effective_prove_mhz, prover_effective_prove_mhz
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
        total_cycles: row.try_get("total_cycles").ok(),
        effective_prove_mhz: row.try_get("effective_prove_mhz").ok(),
        prover_effective_prove_mhz: row.try_get("prover_effective_prove_mhz").ok(),
    }
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_happy_path(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let block = fixture
        .ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
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
    let (req, sig) = create_order_with_timeouts(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
        1000000,
        1000000,
    )
    .await;

    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());
    assert_eq!(status.source, "onchain");
    assert!(status.locked_at.is_none());
    assert!(status.fulfilled_at.is_none());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());
    assert!(status.locked_at.is_some());
    assert_eq!(status.lock_end, req.offer.rampUpStart as i64 + req.offer.lockTimeout as i64);

    // Fulfill request (already locked, so use fulfill_request instead of lock_and_fulfill_request)
    fulfill_request(&fixture.ctx, &fixture.prover, &req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify fulfilled status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());

    indexer_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_effective_prove_mhz_calculation(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let block = fixture
        .ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .start_block(&start_block.to_string())
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and submit request
    let (req, sig) = create_order_with_timeouts(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
        1000000,
        1000000,
    )
    .await;

    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Get the request digest and created_at
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    let request_digest = B256::from_str(&format!("0x{}", status.request_digest)).unwrap();
    let created_at = status.created_at as u64;

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Capture locked_at for prover_effective_prove_mhz calculation
    let status_after_lock =
        get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    let locked_at = status_after_lock.locked_at.unwrap() as u64;

    // Fulfill request WITHOUT cycle counts first
    fulfill_request(&fixture.ctx, &fixture.prover, &req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify fulfilled status but effective_prove_mhz should be None without cycle counts
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());
    assert!(
        status.effective_prove_mhz.is_none(),
        "effective_prove_mhz should be None without cycle counts"
    );
    assert!(
        status.prover_effective_prove_mhz.is_none(),
        "prover_effective_prove_mhz should be None without cycle counts"
    );

    // Now manually insert cycle counts AFTER fulfillment
    // Manually insert as we don't run the cycle count executor in these tests.
    let program_cycles = 100_000_000u64; // 100M cycles
    let total_cycles = (program_cycles as f64 * 1.0158) as u64; // ~101.58M cycles with overhead
    let cycle_count_updated_at =
        insert_cycle_counts_with_overhead(&fixture.test_db, request_digest, program_cycles)
            .await
            .unwrap();

    // Advance time and mine a block to trigger indexer to process cycle count updates
    // Ensure we mine beyond the cycle count updated_at timestamp
    let current_timestamp = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let advance_seconds = if current_timestamp >= cycle_count_updated_at {
        1
    } else {
        cycle_count_updated_at - current_timestamp + 1
    };
    advance_time_and_mine(&fixture.ctx.customer_provider, advance_seconds, 1).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify effective_prove_mhz is now populated
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());
    assert!(
        status.total_cycles.is_some(),
        "total_cycles should be populated after inserting cycle counts"
    );

    // Calculate expected effective_prove_mhz
    let fulfilled_at = status.fulfilled_at.unwrap() as u64;
    let proof_delivery_time = fulfilled_at - created_at;
    let expected_mhz = total_cycles.to_string().parse::<f64>().unwrap_or(0.0)
        / (proof_delivery_time as f64 * 1_000_000.0);
    let actual_mhz: f64 = status.effective_prove_mhz.unwrap();

    // Use floating point comparison with epsilon
    const EPSILON: f64 = 0.00000001; // 8 decimal places precision
    assert!(
        (actual_mhz - expected_mhz).abs() < EPSILON,
        "effective_prove_mhz should equal total_cycles / (proof_delivery_time * 1_000_000). \
         Expected: {}, Actual: {}, total_cycles: {}, proof_delivery_time: {}",
        expected_mhz,
        actual_mhz,
        total_cycles,
        proof_delivery_time
    );

    // Verify it's a positive number
    assert!(actual_mhz > 0.0, "effective_prove_mhz should be positive, got {}", actual_mhz);

    // Verify it has decimal precision
    let fractional_part = actual_mhz - actual_mhz.floor();
    assert!(
        fractional_part > EPSILON,
        "effective_prove_mhz should have decimal precision, but got whole number: {}",
        actual_mhz
    );

    // Verify prover_effective_prove_mhz (primary fulfillment case: lock holder fulfills)
    // Formula: total_cycles / (fulfilled_at - locked_at)
    let prover_prove_time = fulfilled_at - locked_at;
    let expected_prover_mhz = total_cycles.to_string().parse::<f64>().unwrap_or(0.0)
        / (prover_prove_time as f64 * 1_000_000.0);
    let actual_prover_mhz: f64 = status.prover_effective_prove_mhz.unwrap();

    assert!(
        (actual_prover_mhz - expected_prover_mhz).abs() < EPSILON,
        "prover_effective_prove_mhz should equal total_cycles / (fulfilled_at - locked_at). \
         Expected: {}, Actual: {}, total_cycles: {}, prover_prove_time: {}",
        expected_prover_mhz,
        actual_prover_mhz,
        total_cycles,
        prover_prove_time
    );

    assert!(
        actual_prover_mhz > 0.0,
        "prover_effective_prove_mhz should be positive, got {}",
        actual_prover_mhz
    );

    // prover_effective_prove_mhz should be higher than effective_prove_mhz since
    // (fulfilled_at - locked_at) < (fulfilled_at - created_at)
    assert!(
        actual_prover_mhz > actual_mhz,
        "prover_effective_prove_mhz ({}) should be greater than effective_prove_mhz ({}) \
         since prover's time window is shorter",
        actual_prover_mhz,
        actual_mhz
    );

    indexer_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_locked_then_expired(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let block = fixture
        .ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .start_block(&start_block.to_string())
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and submit request with short timeout
    let (req, sig) = create_order_with_timeouts(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
        1000,
        1000,
    )
    .await;

    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture.ctx.customer_market.submit_request_with_signature(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig.clone()).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());

    // Advance time past request expiration
    let expires_at = req.expires_at();
    tracing::info!("Request expires at: {}.", expires_at);
    fixture.ctx.customer_provider.anvil_set_next_block_timestamp(expires_at + 1).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify expired status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Expired.to_string());

    indexer_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_request_status_lock_expired_then_slashed(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let block = fixture
        .ctx
        .customer_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .await
        .unwrap()
        .unwrap();
    let start_block = block.header.number.saturating_sub(1);

    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
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
    let sig = req
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();
    let sig_bytes: Bytes = sig.as_bytes().into();

    fixture.ctx.customer_market.deposit(U256::from(10)).await.unwrap();
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&req, sig_bytes.clone())
        .await
        .unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(3), None).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify submitted status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Submitted.to_string());

    // Lock request
    fixture.ctx.prover_market.lock_request(&req, sig_bytes.clone()).await.unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(2), None).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify locked status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Locked.to_string());
    let lock_end = status.lock_end;

    // Advance time past lock expiration and fulfill late
    advance_time_to_and_mine(&fixture.ctx.customer_provider, lock_end as u64 + 1, 1).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Fulfill request (late fulfillment)
    let (fill, root_receipt, assessor_receipt) =
        fixture.prover.fulfill(&[(req.clone(), sig_bytes.clone())]).await.unwrap();
    let order_fulfilled =
        OrderFulfilled::new(fill.clone(), root_receipt, assessor_receipt).unwrap();
    fixture
        .ctx
        .prover_market
        .fulfill(
            FulfillmentTx::new(order_fulfilled.fills, order_fulfilled.assessorReceipt)
                .with_submit_root(
                    fixture.ctx.deployment.set_verifier_address,
                    order_fulfilled.root,
                    order_fulfilled.seal,
                ),
        )
        .await
        .unwrap();

    advance_time_and_mine(&fixture.ctx.customer_provider, 2, 1).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Verify fulfilled status
    let status = get_request_status(&fixture.test_db.pool, &format!("{:x}", req.id)).await;
    assert_eq!(status.request_status, RequestStatusType::Fulfilled.to_string());
    assert!(status.fulfilled_at.is_some());

    // Verify this is a secondary fulfillment (fulfilled after lock_end but before expires_at)
    let fulfilled_at = status.fulfilled_at.unwrap();
    assert!(fulfilled_at > lock_end, "Fulfillment should occur after lock_end");
    assert!(fulfilled_at < req.expires_at() as i64, "Fulfillment should occur before expires_at");

    // Verify secondary fulfillments are counted in aggregates
    let summaries = get_hourly_summaries(&fixture.test_db.db).await;
    tracing::info!("Hourly summaries: {:?}", summaries);
    let total_secondary_fulfillments: u64 =
        summaries.iter().map(|s| s.total_secondary_fulfillments).sum();
    assert_eq!(
        total_secondary_fulfillments, 1,
        "Should have 1 secondary fulfillment in aggregates"
    );

    // Verify all-time aggregates also include secondary fulfillments
    let all_time_summaries = get_all_time_summaries(&fixture.test_db.pool).await;
    let latest_all_time = all_time_summaries.last().unwrap();
    assert_eq!(
        latest_all_time.total_secondary_fulfillments, 1,
        "All-time aggregates should show 1 secondary fulfillment"
    );

    // Verify requestor aggregates also include secondary fulfillments
    let requestor_address = fixture.ctx.customer_signer.address();
    let current_block_timestamp = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let requestor_summaries = fixture
        .test_db
        .db
        .get_hourly_requestor_summaries_by_range(
            requestor_address,
            current_block_timestamp.saturating_sub(7200), // Start from 2 hours before
            current_block_timestamp + 36000,              // End 1 hour after
        )
        .await
        .unwrap();

    tracing::info!("Requestor summaries: {:?}", requestor_summaries);

    let total_requestor_secondary_fulfillments: u64 =
        requestor_summaries.iter().map(|s| s.total_secondary_fulfillments).sum();
    assert_eq!(
        total_requestor_secondary_fulfillments, 1,
        "Requestor hourly aggregates should show 1 secondary fulfillment"
    );

    // Verify all-time requestor aggregates
    let latest_requestor_all_time =
        fixture.test_db.db.get_latest_all_time_requestor_summary(requestor_address).await.unwrap();
    assert!(latest_requestor_all_time.is_some(), "Should have all-time requestor summary");
    let requestor_all_time = latest_requestor_all_time.unwrap();
    assert_eq!(
        requestor_all_time.total_secondary_fulfillments, 1,
        "All-time requestor aggregates should show 1 secondary fulfillment"
    );

    // Advance time and slash
    fixture
        .ctx
        .customer_provider
        .anvil_set_next_block_timestamp(req.expires_at() + 1)
        .await
        .unwrap();
    fixture.ctx.customer_provider.anvil_mine(Some(1), None).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;
    fixture.ctx.prover_market.slash(req.id).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

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

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_cumulative_carry_forward_with_no_activity_gaps(pool: sqlx::PgPool) {
    // This test verifies that:
    // 1. All-time cumulatives properly carry forward during hours with no activity
    // 2. There are no gaps in hour entries (every hour gets an entry)
    // 3. Cumulative values never decrease

    let fixture = new_market_test_fixture(pool).await.unwrap();

    tracing::info!(
        "Starting test, block number: {}",
        fixture.ctx.customer_provider.get_block_number().await.unwrap()
    );

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .retries("1")
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let one_eth = parse_ether("1").unwrap();

    // Create and fulfill first request with 0 collateral
    let request1 = ProofRequest::new(
        RequestId::new(fixture.ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: one_eth,
            maxPrice: one_eth,
            rampUpStart: now - 3,
            timeout: 24,
            rampUpPeriod: 1,
            lockTimeout: 24,
            lockCollateral: U256::from(0),
        },
    );
    let client_sig1 = request1
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();

    fixture.ctx.customer_market.deposit(one_eth * U256::from(2)).await.unwrap();
    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request1, Bytes::from(client_sig1.as_bytes()))
        .await
        .unwrap();

    lock_and_fulfill_request(
        &fixture.ctx,
        &fixture.prover,
        &request1,
        client_sig1.as_bytes().into(),
    )
    .await
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Advance time by 3 hours with NO activity
    tracing::info!("Advancing time by 3 hours with no activity");
    advance_time_and_mine(&fixture.ctx.customer_provider, 3 * 3600 + 100, 3).await.unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    let now2 = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;

    // Create and fulfill second request (different hour) with no collateral
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
    let client_sig2 = request2
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();

    fixture
        .ctx
        .customer_market
        .submit_request_with_signature(&request2, Bytes::from(client_sig2.as_bytes()))
        .await
        .unwrap();

    lock_and_fulfill_request(
        &fixture.ctx,
        &fixture.prover,
        &request2,
        client_sig2.as_bytes().into(),
    )
    .await
    .unwrap();

    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    tracing::info!(
        "Getting hourly summaries, block number: {}",
        fixture.ctx.customer_provider.get_block_number().await.unwrap()
    );

    // Get all hourly summaries and verify gaps are filled with zero-activity entries
    let hourly_summaries = get_hourly_summaries(&fixture.test_db.db).await;

    // We should have at least 5 hours of data (hour with request1, 3 hours of no activity, hour with request2)
    // Why at least? Because the indexer always reprocesses the past HOURLY_AGGREGATION_RECOMPUTE_HOURS hours of data on each run,
    // so it will go back in the past and process the hours before the test even started.
    assert!(
        hourly_summaries.len() >= 5,
        "Expected at least 5 hourly summaries, got {}",
        hourly_summaries.len()
    );

    // Verify hour boundaries and NO GAPS
    verify_hour_boundaries(&hourly_summaries);

    // Check for continuous hourly timestamps (no gaps)
    for i in 1..hourly_summaries.len() {
        let prev_hour = hourly_summaries[i - 1].period_timestamp;
        let curr_hour = hourly_summaries[i].period_timestamp;
        let hour_diff = curr_hour.saturating_sub(prev_hour);
        assert_eq!(
            hour_diff, 3600,
            "Expected exactly 1 hour (3600s) between consecutive summaries, but got {}s between hour {} and {}",
            hour_diff, prev_hour, curr_hour
        );
    }
    tracing::info!("No gaps in hourly summaries - all hours present");

    // Identify hours with activity and hours without activity
    let hours_with_activity: Vec<_> =
        hourly_summaries.iter().filter(|s| s.total_requests_submitted > 0).collect();
    let hours_without_activity: Vec<_> =
        hourly_summaries.iter().filter(|s| s.total_requests_submitted == 0).collect();

    assert_eq!(hours_with_activity.len(), 2, "Expected 2 hours with activity");

    // Verify zero-activity hours have all zero values
    for zero_hour in &hours_without_activity {
        assert_eq!(zero_hour.total_fulfilled, 0, "Zero-activity hour should have 0 fulfilled");
        assert_eq!(
            zero_hour.total_requests_submitted, 0,
            "Zero-activity hour should have 0 submitted"
        );
        assert_eq!(zero_hour.total_requests_locked, 0, "Zero-activity hour should have 0 locked");
        assert_eq!(
            zero_hour.total_fees_locked,
            U256::ZERO,
            "Zero-activity hour should have 0 fees"
        );
        assert_eq!(
            zero_hour.total_collateral_locked,
            U256::ZERO,
            "Zero-activity hour should have 0 collateral"
        );
    }

    // Get all-time summaries and verify cumulative behavior
    let all_time_summaries = get_all_time_summaries(&fixture.test_db.pool).await;

    assert!(
        all_time_summaries.len() >= 4,
        "Expected at least 4 all-time summaries, got {}",
        all_time_summaries.len()
    );

    // Verify all-time summaries have NO GAPS (one for each hour)
    for i in 1..all_time_summaries.len() {
        let prev_ts = all_time_summaries[i - 1].period_timestamp;
        let curr_ts = all_time_summaries[i].period_timestamp;
        let time_diff = curr_ts.saturating_sub(prev_ts);
        assert_eq!(
            time_diff, 3600,
            "Expected exactly 1 hour between consecutive all-time summaries, but got {}s between {} and {}",
            time_diff, prev_ts, curr_ts
        );
    }

    // Verify all-time cumulatives NEVER decrease (always monotonically increasing or staying the same)
    for i in 1..all_time_summaries.len() {
        let prev = &all_time_summaries[i - 1];
        let curr = &all_time_summaries[i];

        assert!(
            curr.total_fulfilled >= prev.total_fulfilled,
            "All-time total_fulfilled should never decrease: hour {} has {}, hour {} has {}",
            prev.period_timestamp,
            prev.total_fulfilled,
            curr.period_timestamp,
            curr.total_fulfilled
        );

        assert!(curr.total_requests_submitted >= prev.total_requests_submitted,
            "All-time total_requests_submitted should never decrease: hour {} has {}, hour {} has {}",
            prev.period_timestamp, prev.total_requests_submitted, curr.period_timestamp, curr.total_requests_submitted);

        assert!(
            curr.total_requests_locked >= prev.total_requests_locked,
            "All-time total_requests_locked should never decrease: hour {} has {}, hour {} has {}",
            prev.period_timestamp,
            prev.total_requests_locked,
            curr.period_timestamp,
            curr.total_requests_locked
        );

        assert!(
            curr.total_fees_locked >= prev.total_fees_locked,
            "All-time total_fees_locked should never decrease: hour {} has {}, hour {} has {}",
            prev.period_timestamp,
            prev.total_fees_locked,
            curr.period_timestamp,
            curr.total_fees_locked
        );

        assert!(curr.total_collateral_locked >= prev.total_collateral_locked,
            "All-time total_collateral_locked should never decrease: hour {} has {}, hour {} has {}",
            prev.period_timestamp, prev.total_collateral_locked, curr.period_timestamp, curr.total_collateral_locked);
    }

    // Verify that during no-activity hours, all-time values stay constant (carry forward)
    // and match the exact expected values
    let first_activity_hour = hourly_summaries
        .iter()
        .position(|s| s.total_requests_submitted > 0)
        .expect("Should find first activity hour");

    let second_activity_hour = hourly_summaries
        .iter()
        .skip(first_activity_hour + 1)
        .position(|s| s.total_requests_submitted > 0)
        .map(|pos| pos + first_activity_hour + 1)
        .expect("Should find second activity hour");

    // Check all-time summaries between the two activity periods
    if second_activity_hour > first_activity_hour + 1 {
        // There are hours in between - verify they all have the same cumulative values
        let first_activity_ts = hourly_summaries[first_activity_hour].period_timestamp;
        let second_activity_ts = hourly_summaries[second_activity_hour].period_timestamp;

        // Find corresponding all-time summaries
        let first_all_time_idx = all_time_summaries
            .iter()
            .position(|s| s.period_timestamp == first_activity_ts)
            .expect("Should find all-time for first activity");

        let second_all_time_idx = all_time_summaries
            .iter()
            .position(|s| s.period_timestamp == second_activity_ts)
            .expect("Should find all-time for second activity");

        // After first request, we should have: 1 submitted, 1 locked, 1 fulfilled, 0 collateral
        let expected_after_first = &all_time_summaries[first_all_time_idx];
        assert_eq!(
            expected_after_first.total_requests_submitted, 1,
            "After first request: should have exactly 1 submitted"
        );
        assert_eq!(
            expected_after_first.total_requests_locked, 1,
            "After first request: should have exactly 1 locked"
        );
        assert_eq!(
            expected_after_first.total_fulfilled, 1,
            "After first request: should have exactly 1 fulfilled"
        );
        assert_eq!(
            expected_after_first.total_collateral_locked,
            U256::ZERO,
            "After first request: should have exactly 0 collateral (none used)"
        );

        // Verify carry-forward during gap - values should stay at first request totals
        for gap_summary in
            all_time_summaries.iter().take(second_all_time_idx).skip(first_all_time_idx + 1)
        {
            // During no-activity periods, cumulative values should stay exactly at first request values
            assert_eq!(gap_summary.total_fulfilled, 1,
                "During no-activity hour {}: total_fulfilled should stay at 1 (not increase or decrease)", gap_summary.period_timestamp);
            assert_eq!(
                gap_summary.total_requests_submitted, 1,
                "During no-activity hour {}: total_requests_submitted should stay at 1",
                gap_summary.period_timestamp
            );
            assert_eq!(
                gap_summary.total_requests_locked, 1,
                "During no-activity hour {}: total_requests_locked should stay at 1",
                gap_summary.period_timestamp
            );
            assert_eq!(
                gap_summary.total_collateral_locked,
                U256::ZERO,
                "During no-activity hour {}: total_collateral_locked should stay at 0",
                gap_summary.period_timestamp
            );

            tracing::debug!(
                "  Gap hour {}: correctly carries forward values (1, 1, 1, 0)",
                gap_summary.period_timestamp
            );
        }

        // After second request, we should have: 2 submitted, 2 locked, 2 fulfilled, 0 collateral
        let after_second = &all_time_summaries[second_all_time_idx];
        assert_eq!(
            after_second.total_requests_submitted, 2,
            "After second request: should have exactly 2 submitted (1 + 1)"
        );
        assert_eq!(
            after_second.total_requests_locked, 2,
            "After second request: should have exactly 2 locked (1 + 1)"
        );
        assert_eq!(
            after_second.total_fulfilled, 2,
            "After second request: should have exactly 2 fulfilled (1 + 1)"
        );
        assert_eq!(
            after_second.total_collateral_locked,
            U256::ZERO,
            "After second request: should have exactly 0 collateral (0 + 0)"
        );
    }

    // Verify final cumulative totals match the exact sum of all activity (2 requests total)
    let latest_all_time = all_time_summaries.last().unwrap();
    assert_eq!(
        latest_all_time.total_fulfilled, 2,
        "Final cumulative fulfilled should be exactly 2 (request 1 + request 2)"
    );
    assert_eq!(
        latest_all_time.total_requests_submitted, 2,
        "Final cumulative submitted should be exactly 2 (request 1 + request 2)"
    );
    assert_eq!(
        latest_all_time.total_requests_locked, 2,
        "Final cumulative locked should be exactly 2 (request 1 + request 2)"
    );
    assert_eq!(
        latest_all_time.total_requests_submitted_onchain, 2,
        "Final cumulative onchain requests should be exactly 2 (both were onchain)"
    );
    assert_eq!(
        latest_all_time.total_requests_submitted_offchain, 0,
        "Final cumulative offchain requests should be exactly 0 (none were offchain)"
    );
    assert_eq!(
        latest_all_time.total_collateral_locked,
        U256::ZERO,
        "Final cumulative collateral should be exactly 0 (0 from request 1 + 0 from request 2)"
    );
    assert_eq!(
        latest_all_time.total_locked_and_fulfilled, 2,
        "Final cumulative locked_and_fulfilled should be exactly 2 (both requests)"
    );
    assert_eq!(
        latest_all_time.total_locked_and_expired, 0,
        "Final cumulative locked_and_expired should be exactly 0 (no expired requests)"
    );
    assert_eq!(
        latest_all_time.total_expired, 0,
        "Final cumulative expired should be exactly 0 (no expired requests)"
    );
    assert_eq!(
        latest_all_time.total_requests_slashed, 0,
        "Final cumulative slashed should be exactly 0 (no slashed requests)"
    );
    assert!(
        latest_all_time.total_fees_locked > U256::ZERO,
        "Final cumulative fees should be non-zero (2 fulfilled requests with fees)"
    );

    cli_process.kill().unwrap();
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_prover_aggregation(pool: sqlx::PgPool) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    let mut cli_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .start_block("0")
    .retries("1")
    .spawn()
    .unwrap();

    let prover_address = fixture.ctx.prover_signer.address();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let request = ProofRequest::new(
        RequestId::new(fixture.ctx.customer_signer.address(), 1),
        Requirements::new(Predicate::prefix_match(ECHO_ID, Bytes::default())),
        format!("file://{ECHO_PATH}"),
        RequestInput::builder().build_inline().unwrap(),
        Offer {
            minPrice: parse_ether("0.001").unwrap(),
            maxPrice: parse_ether("0.001").unwrap(),
            rampUpStart: now - 3,
            rampUpPeriod: 100,
            lockTimeout: 1000,
            timeout: 1000,
            lockCollateral: U256::ZERO,
        },
    );

    let client_sig = request
        .sign_request(
            &fixture.ctx.customer_signer,
            fixture.ctx.deployment.boundless_market_address,
            fixture.anvil.chain_id(),
        )
        .await
        .unwrap();

    submit_request_with_deposit(&fixture.ctx, &request, Bytes::from(client_sig.as_bytes()))
        .await
        .unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    lock_and_fulfill_request_with_collateral(
        &fixture.ctx,
        &fixture.prover,
        &request,
        Bytes::from(client_sig.as_bytes()),
    )
    .await
    .unwrap();
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    let request_id_str = format!("{:x}", request.id);
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_requests").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_submitted_events")
        .await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_locked_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proof_delivered_events").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "proofs").await;
    verify_request_in_table(&fixture.test_db.pool, &request_id_str, "request_fulfilled_events")
        .await;

    let prover_address_str = format!("{:x}", prover_address);
    let status_row = sqlx::query(
        "SELECT lock_prover_address, fulfill_prover_address, locked_at, fulfilled_at
         FROM request_status
         WHERE request_id = $1",
    )
    .bind(&request_id_str)
    .fetch_one(&fixture.test_db.pool)
    .await
    .unwrap();
    let lock_prover_address: Option<String> = status_row.try_get("lock_prover_address").ok();
    let fulfill_prover_address: Option<String> = status_row.try_get("fulfill_prover_address").ok();
    let locked_at: Option<i64> = status_row.try_get("locked_at").ok();
    let fulfilled_at: Option<i64> = status_row.try_get("fulfilled_at").ok();
    assert_eq!(
        lock_prover_address.as_deref(),
        Some(prover_address_str.as_str()),
        "request_status lock_prover_address should match prover signer"
    );
    assert_eq!(
        fulfill_prover_address.as_deref(),
        Some(prover_address_str.as_str()),
        "request_status fulfill_prover_address should match prover signer"
    );
    assert!(locked_at.is_some(), "request_status locked_at should be set");
    assert!(fulfilled_at.is_some(), "request_status fulfilled_at should be set");

    let current_block_timestamp = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    let prover_summaries = fixture
        .test_db
        .db
        .get_hourly_prover_summaries_by_range(
            prover_address,
            current_block_timestamp.saturating_sub(7200),
            current_block_timestamp + 36000,
        )
        .await
        .unwrap();

    assert!(!prover_summaries.is_empty(), "Should have prover summaries");
    let total_locked: u64 = prover_summaries.iter().map(|s| s.total_requests_locked).sum();
    let total_fulfilled: u64 = prover_summaries.iter().map(|s| s.total_requests_fulfilled).sum();
    let total_unique_requestors: u64 =
        prover_summaries.iter().map(|s| s.total_unique_requestors).sum();
    let total_fees_earned: U256 = prover_summaries.iter().map(|s| s.total_fees_earned).sum();
    let activity_summary = prover_summaries
        .iter()
        .find(|s| s.total_requests_locked > 0 || s.total_requests_fulfilled > 0);
    assert!(
        activity_summary.is_some(),
        "Expected at least one hourly prover summary with activity"
    );
    tracing::info!(
        "Totals: locked={}, fulfilled={}, unique_requestors={}, fees_earned={}",
        total_locked,
        total_fulfilled,
        total_unique_requestors,
        total_fees_earned
    );
    assert_eq!(total_locked, 1, "Should have 1 locked request");
    assert_eq!(total_fulfilled, 1, "Should have 1 fulfilled request");
    assert!(total_fees_earned > U256::ZERO, "Should have earned fees");
    assert_eq!(total_unique_requestors, 1, "Should have worked with 1 requestor");

    let latest_all_time =
        fixture.test_db.db.get_latest_all_time_prover_summary(prover_address).await.unwrap();

    assert!(latest_all_time.is_some(), "Should have all-time prover summary");
    let all_time = latest_all_time.unwrap();
    assert!(
        all_time.total_requests_locked >= 1,
        "All-time should include at least this test's lock"
    );
    assert!(
        all_time.total_requests_fulfilled >= 1,
        "All-time should include at least this test's fulfillment"
    );
    assert!(all_time.total_fees_earned > U256::ZERO, "All-time should show fees earned");

    cli_process.kill().unwrap();
}
