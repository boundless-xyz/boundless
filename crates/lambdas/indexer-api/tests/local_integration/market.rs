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

use indexer_api::routes::market::{
    MarketAggregatesResponse, MarketCumulativesResponse, ProverAggregatesResponse,
    ProverCumulativesResponse, RequestListResponse, RequestStatusResponse,
    RequestorAggregatesResponse, RequestorCumulativesResponse,
};

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_list() {
    let env = TestEnv::market().await;

    let response: RequestListResponse = env.get("/v1/market/requests?limit=20").await.unwrap();

    tracing::info!(target: "market-requests-list", "response: {:?}", response);
    assert!(!response.data.is_empty() && response.data.len() <= 20);

    // Verify request structure and all fields
    if !response.data.is_empty() {
        let first = &response.data[0];

        // Basic identifiers
        assert!(
            first.request_digest.starts_with("0x"),
            "request_digest should start with 0x: {}",
            first.request_digest
        );
        assert_eq!(
            first.request_digest.len(),
            66,
            "request_digest should be 66 chars (0x + 64 hex): {}",
            first.request_digest
        );
        assert!(!first.request_id.is_empty(), "request_id should not be empty");

        // Status and source
        assert!(!first.request_status.is_empty(), "request_status should not be empty");
        assert!(
            matches!(
                first.request_status.as_str(),
                "submitted" | "locked" | "fulfilled" | "slashed" | "expired"
            ),
            "request_status should be valid: {}",
            first.request_status
        );
        assert!(!first.source.is_empty(), "source should not be empty");
        assert!(
            matches!(first.source.as_str(), "onchain" | "offchain" | "unknown"),
            "source should be valid: {}",
            first.source
        );

        // Address fields
        assert!(
            first.client_address.starts_with("0x"),
            "client_address should start with 0x: {}",
            first.client_address
        );
        assert_eq!(
            first.client_address.len(),
            42,
            "client_address should be 42 chars (0x + 40 hex): {}",
            first.client_address
        );

        if let Some(ref addr) = first.lock_prover_address {
            assert!(addr.starts_with("0x"), "lock_prover_address should start with 0x: {}", addr);
            assert_eq!(addr.len(), 42, "lock_prover_address should be 42 chars: {}", addr);
        }

        if let Some(ref addr) = first.fulfill_prover_address {
            assert!(
                addr.starts_with("0x"),
                "fulfill_prover_address should start with 0x: {}",
                addr
            );
            assert_eq!(addr.len(), 42, "fulfill_prover_address should be 42 chars: {}", addr);
        }

        // Timestamp fields (Unix)
        assert!(first.created_at >= 0, "created_at should be non-negative: {}", first.created_at);
        assert!(first.updated_at >= 0, "updated_at should be non-negative: {}", first.updated_at);
        assert!(
            first.ramp_up_start >= 0,
            "ramp_up_start should be non-negative: {}",
            first.ramp_up_start
        );
        assert!(first.expires_at >= 0, "expires_at should be non-negative: {}", first.expires_at);
        assert!(first.lock_end >= 0, "lock_end should be non-negative: {}", first.lock_end);

        if let Some(locked_at) = first.locked_at {
            assert!(locked_at >= 0, "locked_at should be non-negative: {}", locked_at);
        }

        if let Some(fulfilled_at) = first.fulfilled_at {
            assert!(fulfilled_at >= 0, "fulfilled_at should be non-negative: {}", fulfilled_at);
        }

        if let Some(slashed_at) = first.slashed_at {
            assert!(slashed_at >= 0, "slashed_at should be non-negative: {}", slashed_at);
        }

        // Timestamp ISO fields
        assert!(!first.created_at_iso.is_empty(), "created_at_iso should not be empty");
        assert!(
            first.created_at_iso.contains('T'),
            "created_at_iso should be ISO 8601 format: {}",
            first.created_at_iso
        );

        assert!(!first.updated_at_iso.is_empty(), "updated_at_iso should not be empty");
        assert!(
            first.updated_at_iso.contains('T'),
            "updated_at_iso should be ISO 8601 format: {}",
            first.updated_at_iso
        );

        assert!(!first.ramp_up_start_iso.is_empty(), "ramp_up_start_iso should not be empty");
        assert!(
            first.ramp_up_start_iso.contains('T'),
            "ramp_up_start_iso should be ISO 8601 format: {}",
            first.ramp_up_start_iso
        );

        assert!(!first.expires_at_iso.is_empty(), "expires_at_iso should not be empty");
        assert!(
            first.expires_at_iso.contains('T'),
            "expires_at_iso should be ISO 8601 format: {}",
            first.expires_at_iso
        );

        assert!(!first.lock_end_iso.is_empty(), "lock_end_iso should not be empty");
        assert!(
            first.lock_end_iso.contains('T'),
            "lock_end_iso should be ISO 8601 format: {}",
            first.lock_end_iso
        );

        if let Some(ref locked_at_iso) = first.locked_at_iso {
            assert!(
                !locked_at_iso.is_empty(),
                "locked_at_iso should not be empty if locked_at is Some"
            );
            assert!(
                locked_at_iso.contains('T'),
                "locked_at_iso should be ISO 8601 format: {}",
                locked_at_iso
            );
        }

        if let Some(ref fulfilled_at_iso) = first.fulfilled_at_iso {
            assert!(
                !fulfilled_at_iso.is_empty(),
                "fulfilled_at_iso should not be empty if fulfilled_at is Some"
            );
            assert!(
                fulfilled_at_iso.contains('T'),
                "fulfilled_at_iso should be ISO 8601 format: {}",
                fulfilled_at_iso
            );
        }

        if let Some(ref slashed_at_iso) = first.slashed_at_iso {
            assert!(
                !slashed_at_iso.is_empty(),
                "slashed_at_iso should not be empty if slashed_at is Some"
            );
            assert!(
                slashed_at_iso.contains('T'),
                "slashed_at_iso should be ISO 8601 format: {}",
                slashed_at_iso
            );
        }

        // Block numbers
        if let Some(submit_block) = first.submit_block {
            assert!(submit_block >= 0, "submit_block should be non-negative: {}", submit_block);
        }

        if let Some(lock_block) = first.lock_block {
            assert!(lock_block >= 0, "lock_block should be non-negative: {}", lock_block);
        }

        if let Some(fulfill_block) = first.fulfill_block {
            assert!(fulfill_block >= 0, "fulfill_block should be non-negative: {}", fulfill_block);
        }

        if let Some(slashed_block) = first.slashed_block {
            assert!(slashed_block >= 0, "slashed_block should be non-negative: {}", slashed_block);
        }

        // Price and collateral fields (raw)
        assert!(!first.min_price.is_empty(), "min_price should not be empty");
        assert!(
            first.min_price.chars().all(|c| c.is_ascii_digit()),
            "min_price should be numeric: {}",
            first.min_price
        );

        assert!(!first.max_price.is_empty(), "max_price should not be empty");
        assert!(
            first.max_price.chars().all(|c| c.is_ascii_digit()),
            "max_price should be numeric: {}",
            first.max_price
        );

        assert!(!first.lock_collateral.is_empty(), "lock_collateral should not be empty");
        assert!(
            first.lock_collateral.chars().all(|c| c.is_ascii_digit()),
            "lock_collateral should be numeric: {}",
            first.lock_collateral
        );

        // Price and collateral fields (formatted)
        assert!(!first.min_price_formatted.is_empty(), "min_price_formatted should not be empty");
        assert!(!first.max_price_formatted.is_empty(), "max_price_formatted should not be empty");
        assert!(
            !first.lock_collateral_formatted.is_empty(),
            "lock_collateral_formatted should not be empty"
        );

        // Ramp up period
        assert!(
            first.ramp_up_period >= 0,
            "ramp_up_period should be non-negative: {}",
            first.ramp_up_period
        );

        // Slash-related fields
        if let Some(ref slash_recipient) = first.slash_recipient {
            assert!(
                slash_recipient.starts_with("0x"),
                "slash_recipient should start with 0x: {}",
                slash_recipient
            );
            assert_eq!(
                slash_recipient.len(),
                42,
                "slash_recipient should be 42 chars: {}",
                slash_recipient
            );
        }

        if let Some(ref amount) = first.slash_transferred_amount {
            assert!(!amount.is_empty(), "slash_transferred_amount should not be empty if Some");
            assert!(
                amount.chars().all(|c| c.is_ascii_digit()),
                "slash_transferred_amount should be numeric: {}",
                amount
            );
        }

        if let Some(ref formatted) = first.slash_transferred_amount_formatted {
            assert!(
                !formatted.is_empty(),
                "slash_transferred_amount_formatted should not be empty if Some"
            );
        }

        if let Some(ref amount) = first.slash_burned_amount {
            assert!(!amount.is_empty(), "slash_burned_amount should not be empty if Some");
            assert!(
                amount.chars().all(|c| c.is_ascii_digit()),
                "slash_burned_amount should be numeric: {}",
                amount
            );
        }

        if let Some(ref formatted) = first.slash_burned_amount_formatted {
            assert!(
                !formatted.is_empty(),
                "slash_burned_amount_formatted should not be empty if Some"
            );
        }

        // Performance metrics
        if let Some(cycles) = &first.total_cycles {
            assert!(
                cycles.parse::<u64>().unwrap_or(0) > 0,
                "total cycles should be greater than 0: {}",
                cycles
            );
        }

        // TODO: Add back in once we have peak and effective prove mhz
        // if let Some(peak_prove_mhz) = first.peak_prove_mhz {
        //     assert!(
        //         peak_prove_mhz >= 0,
        //         "peak_prove_mhz should be non-negative: {}",
        //         peak_prove_mhz
        //     );
        // }

        // if let Some(effective_prove_mhz) = first.effective_prove_mhz {
        //     assert!(
        //         effective_prove_mhz >= 0,
        //         "effective_prove_mhz should be non-negative: {}",
        //         effective_prove_mhz
        //     );
        // }

        // Transaction hashes
        if let Some(ref tx_hash) = first.submit_tx_hash {
            assert!(tx_hash.starts_with("0x"), "submit_tx_hash should start with 0x: {}", tx_hash);
            assert_eq!(tx_hash.len(), 66, "submit_tx_hash should be 66 chars: {}", tx_hash);
        }

        if let Some(ref tx_hash) = first.lock_tx_hash {
            assert!(tx_hash.starts_with("0x"), "lock_tx_hash should start with 0x: {}", tx_hash);
            assert_eq!(tx_hash.len(), 66, "lock_tx_hash should be 66 chars: {}", tx_hash);
        }

        if let Some(ref tx_hash) = first.fulfill_tx_hash {
            assert!(tx_hash.starts_with("0x"), "fulfill_tx_hash should start with 0x: {}", tx_hash);
            assert_eq!(tx_hash.len(), 66, "fulfill_tx_hash should be 66 chars: {}", tx_hash);
        }

        if let Some(ref tx_hash) = first.slash_tx_hash {
            assert!(tx_hash.starts_with("0x"), "slash_tx_hash should start with 0x: {}", tx_hash);
            assert_eq!(tx_hash.len(), 66, "slash_tx_hash should be 66 chars: {}", tx_hash);
        }

        // Image and predicate fields
        assert!(!first.image_id.is_empty(), "image_id should not be empty");
        // image_id is typically 64 hex chars (32 bytes)
        assert!(
            first.image_id.len() == 64 || first.image_id.starts_with("0x"),
            "image_id should be 64 hex chars or start with 0x: {}",
            first.image_id
        );

        if let Some(ref image_url) = first.image_url {
            assert!(!image_url.is_empty(), "image_url should not be empty if Some");
            assert!(
                image_url.starts_with("http://")
                    || image_url.starts_with("https://")
                    || image_url.starts_with("ipfs://")
                    || image_url.starts_with("dweb://"),
                "image_url should be a valid URL: {}",
                image_url
            );
        }

        assert!(!first.selector.is_empty(), "selector should not be empty");
        // selector is typically 8 hex chars (4 bytes)
        assert_eq!(first.selector.len(), 8, "selector should be 8 hex chars: {}", first.selector);

        assert!(!first.predicate_type.is_empty(), "predicate_type should not be empty");
        assert!(
            matches!(
                first.predicate_type.as_str(),
                "DigestMatch" | "PrefixMatch" | "ClaimDigestMatch"
            ),
            "predicate_type should be valid: {}",
            first.predicate_type
        );

        assert!(!first.predicate_data.is_empty(), "predicate_data should not be empty");
        assert!(
            first.predicate_data.starts_with("0x"),
            "predicate_data should start with 0x: {}",
            first.predicate_data
        );

        // Input fields
        assert!(!first.input_type.is_empty(), "input_type should not be empty");
        assert!(
            matches!(first.input_type.as_str(), "Inline" | "Url" | "Storage"),
            "input_type should be valid: {}",
            first.input_type
        );

        assert!(!first.input_data.is_empty(), "input_data should not be empty");
        assert!(
            first.input_data.starts_with("0x"),
            "input_data should start with 0x: {}",
            first.input_data
        );

        // Fulfillment fields
        if let Some(ref fulfill_journal) = first.fulfill_journal {
            assert!(!fulfill_journal.is_empty(), "fulfill_journal should not be empty if Some");
            assert!(
                fulfill_journal.starts_with("0x"),
                "fulfill_journal should start with 0x: {}",
                fulfill_journal
            );
        }

        if let Some(ref fulfill_seal) = first.fulfill_seal {
            assert!(!fulfill_seal.is_empty(), "fulfill_seal should not be empty if Some");
            assert!(
                fulfill_seal.starts_with("0x"),
                "fulfill_seal should start with 0x: {}",
                fulfill_seal
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_by_requestor() {
    let env = TestEnv::market().await;

    // First get a request to obtain a valid client address
    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=1").await.unwrap();

    let first = list_response.data.first().unwrap();
    let client_address = &first.client_address;

    // Test with 0x prefix (as returned from API)
    let path = format!("/v1/market/requestors/{}/requests", client_address);
    let response: RequestListResponse = env.get(&path).await.unwrap();

    // Should return at least the request we found
    assert!(!response.data.is_empty(), "Should return at least one request");

    // Verify all returned requests are from this requestor
    for req in &response.data {
        assert_eq!(
            req.client_address, *client_address,
            "All requests should be from the same requestor"
        );
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_aggregates_hourly() {
    let env = TestEnv::market().await;

    let response: MarketAggregatesResponse =
        env.get("/v1/market/aggregates?aggregation=hourly&limit=10").await.unwrap();

    assert_eq!(response.aggregation.to_string(), "hourly");
    assert!(response.data.len() <= 10);

    // Verify aggregate data structure
    let first = response.data.first().unwrap();

    // Verify timestamp_iso exists
    assert!(!first.timestamp_iso.is_empty(), "timestamp_iso should not be empty");
    assert!(
        first.timestamp_iso.contains('T'),
        "timestamp_iso should be ISO 8601 format: {}",
        first.timestamp_iso
    );

    // Verify timestamp is on hour boundary (minutes and seconds = 0)
    use chrono::{DateTime, Timelike, Utc};
    let dt =
        DateTime::<Utc>::from_timestamp(first.timestamp, 0).expect("timestamp should be valid");
    assert_eq!(
        dt.minute(),
        0,
        "Hourly aggregate timestamp should have minutes=0: {} (timestamp: {})",
        first.timestamp_iso,
        first.timestamp
    );
    assert_eq!(
        dt.second(),
        0,
        "Hourly aggregate timestamp should have seconds=0: {} (timestamp: {})",
        first.timestamp_iso,
        first.timestamp
    );

    // Verify _formatted fields exist for currency amounts
    assert!(
        !first.total_fees_locked_formatted.is_empty(),
        "total_fees_locked_formatted should not be empty"
    );
    assert!(
        !first.total_collateral_locked_formatted.is_empty(),
        "total_collateral_locked_formatted should not be empty"
    );

    // Verify no hour gaps (consecutive entries should be exactly 1 hour apart)
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs(); // Use abs since order depends on sort direction
            assert_eq!(
                gap, 3600,
                "Hourly aggregates should have exactly 1 hour (3600s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_aggregates_daily() {
    let env = TestEnv::market().await;

    let response: MarketAggregatesResponse =
        env.get("/v1/market/aggregates?aggregation=daily&limit=5").await.unwrap();

    assert_eq!(response.aggregation.to_string(), "daily");
    assert!(response.data.len() <= 5);
    assert!(!response.data.is_empty());

    // Verify timestamps are at day boundaries (midnight UTC)
    use chrono::{DateTime, Timelike, Utc};
    for entry in &response.data {
        let dt =
            DateTime::<Utc>::from_timestamp(entry.timestamp, 0).expect("timestamp should be valid");
        assert_eq!(
            dt.hour(),
            0,
            "Daily aggregate timestamp should have hour=0 (midnight): {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.minute(),
            0,
            "Daily aggregate timestamp should have minutes=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.second(),
            0,
            "Daily aggregate timestamp should have seconds=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
    }

    // Verify no day gaps (consecutive entries should be exactly 1 day apart)
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 86400,
                "Daily aggregates should have exactly 1 day (86400s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_aggregates_weekly() {
    let env = TestEnv::market().await;

    let response: MarketAggregatesResponse =
        env.get("/v1/market/aggregates?aggregation=weekly&limit=5").await.unwrap();

    assert_eq!(response.aggregation.to_string(), "weekly");
    assert!(response.data.len() <= 5);
    assert!(!response.data.is_empty());

    // Verify timestamps are at week boundaries (Monday 00:00:00 UTC)
    use chrono::{DateTime, Datelike, Timelike, Utc, Weekday};
    for entry in &response.data {
        let dt =
            DateTime::<Utc>::from_timestamp(entry.timestamp, 0).expect("timestamp should be valid");
        assert_eq!(
            dt.weekday(),
            Weekday::Mon,
            "Weekly aggregate timestamp should be Monday: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.hour(),
            0,
            "Weekly aggregate timestamp should have hour=0 (midnight): {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.minute(),
            0,
            "Weekly aggregate timestamp should have minutes=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.second(),
            0,
            "Weekly aggregate timestamp should have seconds=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
    }

    // Verify no week gaps (consecutive entries should be exactly 1 week apart)
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 604800,
                "Weekly aggregates should have exactly 1 week (604800s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_aggregates_monthly() {
    let env = TestEnv::market().await;

    let response: MarketAggregatesResponse =
        env.get("/v1/market/aggregates?aggregation=monthly&limit=5").await.unwrap();

    assert_eq!(response.aggregation.to_string(), "monthly");
    assert!(response.data.len() <= 5);
    assert!(!response.data.is_empty());

    // Verify timestamps are at month boundaries (1st day 00:00:00 UTC)
    use chrono::{DateTime, Datelike, Timelike, Utc};
    for entry in &response.data {
        let dt =
            DateTime::<Utc>::from_timestamp(entry.timestamp, 0).expect("timestamp should be valid");
        assert_eq!(
            dt.day(),
            1,
            "Monthly aggregate timestamp should be 1st of month: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.hour(),
            0,
            "Monthly aggregate timestamp should have hour=0 (midnight): {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.minute(),
            0,
            "Monthly aggregate timestamp should have minutes=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
        assert_eq!(
            dt.second(),
            0,
            "Monthly aggregate timestamp should have seconds=0: {} (timestamp: {})",
            entry.timestamp_iso,
            entry.timestamp
        );
    }

    // Verify no month gaps (consecutive entries should be approximately 1 month apart)
    // Note: Months vary in length, so we check for reasonable range (28-31 days)
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            let days = gap / 86400;
            assert!(
                (28..=31).contains(&days),
                "Monthly aggregates should have approximately 1 month (28-31 days) gap between consecutive entries. Found gap: {} days ({}s) between {} and {}",
                days, gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_pagination() {
    let env = TestEnv::market().await;

    // Get first page
    let page1: RequestListResponse = env.get("/v1/market/requests?limit=2").await.unwrap();

    assert!(!page1.data.is_empty() && page1.data.len() <= 5);

    // If there's a cursor, test pagination
    if let Some(cursor) = &page1.next_cursor {
        let path = format!("/v1/market/requests?limit=5&cursor={}", cursor);
        let page2: RequestListResponse = env.get(&path).await.unwrap();

        // Verify we got different data
        if !page1.data.is_empty() && !page2.data.is_empty() {
            assert_ne!(
                page1.data[0].request_digest, page2.data[0].request_digest,
                "Pages should contain different requests"
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_sorting() {
    let env = TestEnv::market().await;

    // Test sorting by created_at
    let response_created: RequestListResponse =
        env.get("/v1/market/requests?limit=10&sort_by=created_at").await.unwrap();

    if response_created.data.len() >= 2 {
        // Verify descending order (default)
        assert!(
            response_created.data[0].created_at >= response_created.data[1].created_at,
            "Should be sorted by created_at descending"
        );
    }

    // Test sorting by updated_at
    let response_updated: RequestListResponse =
        env.get("/v1/market/requests?limit=10&sort_by=updated_at").await.unwrap();

    if response_updated.data.len() >= 2 {
        // Verify descending order (default)
        assert!(
            response_updated.data[0].updated_at >= response_updated.data[1].updated_at,
            "Should be sorted by updated_at descending"
        );
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_verify_all_formatted_currency_fields() {
    let env = TestEnv::market().await;

    let response: RequestListResponse = env.get("/v1/market/requests?limit=1").await.unwrap();

    if let Some(first) = response.data.first() {
        // Verify all _formatted fields exist and are non-empty
        assert!(!first.min_price_formatted.is_empty());
        assert!(!first.max_price_formatted.is_empty());
        assert!(!first.lock_collateral_formatted.is_empty());

        // Verify formatted fields contain expected suffix
        assert!(
            first.min_price_formatted.contains("ETH"),
            "Formatted price should contain ETH: {}",
            first.min_price_formatted
        );
        assert!(
            first.max_price_formatted.contains("ETH"),
            "Formatted price should contain ETH: {}",
            first.max_price_formatted
        );
        assert!(
            first.lock_collateral_formatted.contains("ZKC"),
            "Formatted collateral should contain ZKC: {}",
            first.lock_collateral_formatted
        );

        // Optional slash amount fields
        if first.slash_transferred_amount.is_some() {
            assert!(first.slash_transferred_amount_formatted.is_some());
            if let Some(ref formatted) = first.slash_transferred_amount_formatted {
                assert!(
                    formatted.contains("ZKC"),
                    "Formatted slash amount should contain ZKC: {}",
                    formatted
                );
            }
        }
        if first.slash_burned_amount.is_some() {
            assert!(first.slash_burned_amount_formatted.is_some());
            if let Some(ref formatted) = first.slash_burned_amount_formatted {
                assert!(
                    formatted.contains("ZKC"),
                    "Formatted slash amount should contain ZKC: {}",
                    formatted
                );
            }
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_by_request_id_hex_parsing() {
    let env = TestEnv::market().await;

    // Get a valid request ID from the database
    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=1").await.unwrap();
    assert!(!list_response.data.is_empty(), "Test requires at least one request in the database");
    let first = list_response.data.first().unwrap();
    // Strip 0x prefix if present to normalize to hex without prefix
    let request_id_no_prefix = first.request_id.strip_prefix("0x").unwrap_or(&first.request_id);

    // Test without 0x prefix
    let path_no_prefix = format!("/v1/market/requests/{}", request_id_no_prefix);
    let response_no_prefix: Vec<RequestStatusResponse> = env.get(&path_no_prefix).await.unwrap();

    // Test with 0x prefix
    let path_with_prefix = format!("/v1/market/requests/0x{}", request_id_no_prefix);
    let response_with_prefix: Vec<RequestStatusResponse> =
        env.get(&path_with_prefix).await.unwrap();
    // Verify both return the same results
    assert_eq!(
        response_no_prefix.len(),
        response_with_prefix.len(),
        "Both formats should return the same number of requests"
    );

    // Verify the results are identical
    for (req1, req2) in response_no_prefix.iter().zip(response_with_prefix.iter()) {
        assert_eq!(req1.request_digest, req2.request_digest, "Request digests should match");
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_cumulatives() {
    let env = TestEnv::market().await;

    // Test basic endpoint
    let response: MarketCumulativesResponse =
        env.get("/v1/market/cumulatives?limit=10").await.unwrap();

    assert!(response.data.len() <= 10);
    assert!(!response.data.is_empty());

    // Verify response structure and data types
    let first = response.data.first().unwrap();

    // Verify timestamp_iso format (ISO 8601)
    assert!(!first.timestamp_iso.is_empty(), "timestamp_iso should not be empty");
    assert!(
        first.timestamp_iso.contains('T'),
        "timestamp_iso should be ISO 8601 format: {}",
        first.timestamp_iso
    );

    // Verify formatted currency fields exist
    assert!(
        !first.total_fees_locked_formatted.is_empty(),
        "total_fees_locked_formatted should not be empty"
    );
    assert!(
        !first.total_collateral_locked_formatted.is_empty(),
        "total_collateral_locked_formatted should not be empty"
    );
    assert!(
        !first.total_locked_and_expired_collateral_formatted.is_empty(),
        "total_locked_and_expired_collateral_formatted should not be empty"
    );

    // Verify cumulative nature: later timestamps should have >= values than earlier ones
    if response.data.len() >= 2 {
        let sorted_data: Vec<_> = response.data.iter().collect();
        for i in 1..sorted_data.len() {
            let prev = &sorted_data[i - 1];
            let curr = &sorted_data[i];

            // If sorted descending, prev should have >= values
            // If sorted ascending, curr should have >= values
            // For simplicity, check that values are non-decreasing when sorted by timestamp
            if prev.timestamp > curr.timestamp {
                // Descending order - previous should have >= values
                assert!(
                    prev.total_fulfilled >= curr.total_fulfilled,
                    "Cumulative values should be non-decreasing: prev.total_fulfilled={}, curr.total_fulfilled={}",
                    prev.total_fulfilled,
                    curr.total_fulfilled
                );
                assert!(
                    prev.total_requests_submitted >= curr.total_requests_submitted,
                    "Cumulative values should be non-decreasing: prev.total_requests_submitted={}, curr.total_requests_submitted={}",
                    prev.total_requests_submitted,
                    curr.total_requests_submitted
                );
            } else {
                // Ascending order - current should have >= values
                assert!(
                    curr.total_fulfilled >= prev.total_fulfilled,
                    "Cumulative values should be non-decreasing: prev.total_fulfilled={}, curr.total_fulfilled={}",
                    prev.total_fulfilled,
                    curr.total_fulfilled
                );
                assert!(
                    curr.total_requests_submitted >= prev.total_requests_submitted,
                    "Cumulative values should be non-decreasing: prev.total_requests_submitted={}, curr.total_requests_submitted={}",
                    prev.total_requests_submitted,
                    curr.total_requests_submitted
                );
            }
        }
    }

    // Test pagination with cursor
    if let Some(cursor) = &response.next_cursor {
        let page2: MarketCumulativesResponse =
            env.get(&format!("/v1/market/cumulatives?limit=10&cursor={}", cursor)).await.unwrap();

        // Verify we got different data
        if !response.data.is_empty() && !page2.data.is_empty() {
            assert_ne!(
                response.data[0].timestamp, page2.data[0].timestamp,
                "Pages should contain different timestamps"
            );
        }
    }

    // Test time filtering with before param
    if !response.data.is_empty() {
        let before_ts = response.data[0].timestamp;
        let filtered: MarketCumulativesResponse = env
            .get(&format!("/v1/market/cumulatives?limit=10&before={}", before_ts))
            .await
            .unwrap();

        // All results should be before the specified timestamp
        for entry in &filtered.data {
            assert!(
                entry.timestamp < before_ts,
                "All entries should be before {}: found {}",
                before_ts,
                entry.timestamp
            );
        }
    }

    // Test time filtering with after param
    if !response.data.is_empty() {
        let after_ts = response.data.last().unwrap().timestamp;
        let filtered: MarketCumulativesResponse =
            env.get(&format!("/v1/market/cumulatives?limit=10&after={}", after_ts)).await.unwrap();

        // All results should be after the specified timestamp
        for entry in &filtered.data {
            assert!(
                entry.timestamp > after_ts,
                "All entries should be after {}: found {}",
                after_ts,
                entry.timestamp
            );
        }
    }

    // Test sorting (asc)
    let response_asc: MarketCumulativesResponse =
        env.get("/v1/market/cumulatives?limit=10&sort=asc").await.unwrap();

    if response_asc.data.len() >= 2 {
        // Verify ascending order
        for i in 1..response_asc.data.len() {
            assert!(
                response_asc.data[i - 1].timestamp <= response_asc.data[i].timestamp,
                "Should be sorted ascending by timestamp"
            );
        }
    }

    // Test sorting (desc - default)
    let response_desc: MarketCumulativesResponse =
        env.get("/v1/market/cumulatives?limit=10&sort=desc").await.unwrap();

    if response_desc.data.len() >= 2 {
        // Verify descending order
        for i in 1..response_desc.data.len() {
            assert!(
                response_desc.data[i - 1].timestamp >= response_desc.data[i].timestamp,
                "Should be sorted descending by timestamp"
            );
        }

        // Verify no hour gaps (cumulatives are created hourly, so consecutive entries should be 1 hour apart)
        for i in 1..response_desc.data.len() {
            let prev_ts = response_desc.data[i - 1].timestamp;
            let curr_ts = response_desc.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 3600,
                "Market cumulatives should have exactly 1 hour (3600s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_requestor_aggregates() {
    let env = TestEnv::market().await;

    // First get a request to obtain a valid requestor address
    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=1").await.unwrap();

    let first = list_response.data.first().unwrap();
    let requestor_address = &first.client_address;

    // Test hourly aggregation
    let path = format!(
        "/v1/market/requestors/{}/aggregates?aggregation=hourly&limit=10",
        requestor_address
    );
    let response: RequestorAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "hourly");
    assert_eq!(response.requestor_address, *requestor_address);
    assert!(response.data.len() <= 10);

    // Verify response structure
    if !response.data.is_empty() {
        let first_entry = &response.data[0];
        assert_eq!(first_entry.requestor_address, *requestor_address);
        assert!(!first_entry.timestamp_iso.is_empty());
        assert!(!first_entry.total_fees_locked_formatted.is_empty());
        // Verify percentile fields exist
        assert!(!first_entry.p50_lock_price_per_cycle_formatted.is_empty());
    }

    // Verify no hour gaps for hourly aggregates
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 3600,
                "Hourly requestor aggregates should have exactly 1 hour (3600s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }

    // Test daily aggregation
    let path =
        format!("/v1/market/requestors/{}/aggregates?aggregation=daily&limit=5", requestor_address);
    let response: RequestorAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "daily");
    assert_eq!(response.requestor_address, *requestor_address);

    // Verify no day gaps for daily aggregates
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 86400,
                "Daily requestor aggregates should have exactly 1 day (86400s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }

    // Test weekly aggregation
    let path = format!(
        "/v1/market/requestors/{}/aggregates?aggregation=weekly&limit=5",
        requestor_address
    );
    let response: RequestorAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "weekly");
    assert_eq!(response.requestor_address, *requestor_address);

    // Verify no week gaps for weekly aggregates
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 604800,
                "Weekly requestor aggregates should have exactly 1 week (604800s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }

    // Test that monthly is rejected
    let path = format!(
        "/v1/market/requestors/{}/aggregates?aggregation=monthly&limit=5",
        requestor_address
    );
    let result: Result<RequestorAggregatesResponse, _> = env.get(&path).await;
    assert!(result.is_err(), "Monthly aggregation should be rejected");

    // Test pagination
    if let Some(cursor) = &response.next_cursor {
        let path = format!(
            "/v1/market/requestors/{}/aggregates?aggregation=weekly&limit=10&cursor={}",
            requestor_address, cursor
        );
        let page2: RequestorAggregatesResponse = env.get(&path).await.unwrap();

        if !response.data.is_empty() && !page2.data.is_empty() {
            assert_ne!(
                response.data[0].timestamp, page2.data[0].timestamp,
                "Pages should contain different timestamps"
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_requestor_cumulatives() {
    let env = TestEnv::market().await;

    // First get a request to obtain a valid requestor address
    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=1").await.unwrap();

    let first = list_response.data.first().unwrap();
    let requestor_address = &first.client_address;

    // Test basic endpoint
    let path = format!("/v1/market/requestors/{}/cumulatives?limit=10", requestor_address);
    let response: RequestorCumulativesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.requestor_address, *requestor_address);
    assert!(response.data.len() <= 10);
    assert!(!response.data.is_empty());

    // Verify response structure
    let first_entry = response.data.first().unwrap();
    assert_eq!(first_entry.requestor_address, *requestor_address);
    assert!(!first_entry.timestamp_iso.is_empty());
    assert!(!first_entry.total_fees_locked_formatted.is_empty());

    // Verify cumulative nature
    if response.data.len() >= 2 {
        let sorted_data: Vec<_> = response.data.iter().collect();
        for i in 1..sorted_data.len() {
            let prev = &sorted_data[i - 1];
            let curr = &sorted_data[i];

            if prev.timestamp > curr.timestamp {
                // Descending order - previous should have >= values
                assert!(
                    prev.total_fulfilled >= curr.total_fulfilled,
                    "Cumulative values should be non-decreasing"
                );
            } else {
                // Ascending order - current should have >= values
                assert!(
                    curr.total_fulfilled >= prev.total_fulfilled,
                    "Cumulative values should be non-decreasing"
                );
            }
        }
    }

    // Test pagination
    if let Some(cursor) = &response.next_cursor {
        let path = format!(
            "/v1/market/requestors/{}/cumulatives?limit=10&cursor={}",
            requestor_address, cursor
        );
        let page2: RequestorCumulativesResponse = env.get(&path).await.unwrap();

        if !response.data.is_empty() && !page2.data.is_empty() {
            assert_ne!(
                response.data[0].timestamp, page2.data[0].timestamp,
                "Pages should contain different timestamps"
            );
        }
    }

    // Test time filtering
    if !response.data.is_empty() {
        let before_ts = response.data[0].timestamp;
        let path = format!(
            "/v1/market/requestors/{}/cumulatives?limit=10&before={}",
            requestor_address, before_ts
        );
        let filtered: RequestorCumulativesResponse = env.get(&path).await.unwrap();

        for entry in &filtered.data {
            assert!(
                entry.timestamp < before_ts,
                "All entries should be before {}: found {}",
                before_ts,
                entry.timestamp
            );
        }
    }

    // Verify no hour gaps (requestor cumulatives are created hourly, so consecutive entries should be 1 hour apart)
    if response.data.len() >= 2 {
        for i in 1..response.data.len() {
            let prev_ts = response.data[i - 1].timestamp;
            let curr_ts = response.data[i].timestamp;
            let gap = (prev_ts - curr_ts).abs();
            assert_eq!(
                gap, 3600,
                "Requestor cumulatives should have exactly 1 hour (3600s) gap between consecutive entries. Found gap: {}s between {} and {}",
                gap, prev_ts, curr_ts
            );
        }
    }
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_prover_aggregates() {
    let env = TestEnv::market().await;

    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=10").await.unwrap();

    let prover_address = list_response
        .data
        .iter()
        .find_map(|r| r.lock_prover_address.as_ref().or(r.fulfill_prover_address.as_ref()))
        .expect("Should find a prover address");

    let path =
        format!("/v1/market/provers/{}/aggregates?aggregation=hourly&limit=10", prover_address);
    let response: ProverAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "hourly");
    assert_eq!(response.prover_address, *prover_address);
    assert!(response.data.len() <= 10);

    if !response.data.is_empty() {
        let first_entry = &response.data[0];
        assert_eq!(first_entry.prover_address, *prover_address);
        assert!(!first_entry.timestamp_iso.is_empty());
        assert!(!first_entry.total_fees_earned_formatted.is_empty());
    }

    let path =
        format!("/v1/market/provers/{}/aggregates?aggregation=daily&limit=5", prover_address);
    let response: ProverAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "daily");
    assert_eq!(response.prover_address, *prover_address);

    let path =
        format!("/v1/market/provers/{}/aggregates?aggregation=weekly&limit=5", prover_address);
    let response: ProverAggregatesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.aggregation.to_string(), "weekly");
    assert_eq!(response.prover_address, *prover_address);

    let path =
        format!("/v1/market/provers/{}/aggregates?aggregation=monthly&limit=5", prover_address);
    let result: Result<ProverAggregatesResponse, _> = env.get(&path).await;
    assert!(result.is_err(), "Monthly aggregation should be rejected");
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_prover_cumulatives() {
    let env = TestEnv::market().await;

    let list_response: RequestListResponse = env.get("/v1/market/requests?limit=10").await.unwrap();

    let prover_address = list_response
        .data
        .iter()
        .find_map(|r| r.lock_prover_address.as_ref().or(r.fulfill_prover_address.as_ref()))
        .expect("Should find a prover address");

    let path = format!("/v1/market/provers/{}/cumulatives?limit=10", prover_address);
    let response: ProverCumulativesResponse = env.get(&path).await.unwrap();

    assert_eq!(response.prover_address, *prover_address);
    assert!(response.data.len() <= 10);

    if !response.data.is_empty() {
        let first_entry = response.data.first().unwrap();
        assert_eq!(first_entry.prover_address, *prover_address);
        assert!(!first_entry.timestamp_iso.is_empty());
        assert!(!first_entry.total_fees_earned_formatted.is_empty());
    }

    if response.data.len() >= 2 {
        let sorted_data: Vec<_> = response.data.iter().collect();
        for i in 1..sorted_data.len() {
            let prev = &sorted_data[i - 1];
            let curr = &sorted_data[i];

            if prev.timestamp > curr.timestamp {
                assert!(
                    prev.total_requests_locked >= curr.total_requests_locked,
                    "Cumulative values should be non-decreasing"
                );
            } else {
                assert!(
                    curr.total_requests_locked >= prev.total_requests_locked,
                    "Cumulative values should be non-decreasing"
                );
            }
        }
    }
}
