// Integration tests for Market API endpoints

use indexer_api::routes::market::{
    MarketAggregatesResponse, RequestListResponse,
};

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_list() {
    let env = TestEnv::market().await;

    let response: RequestListResponse = env.get("/v1/market/requests?limit=20").await.unwrap();

    tracing::info!(target: "market-requests-list", "response: {:?}", response);
    assert!(response.data.len() <= 20);

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
        assert!(
            !first.request_id.is_empty(),
            "request_id should not be empty"
        );

        // Status and source
        assert!(
            !first.request_status.is_empty(),
            "request_status should not be empty"
        );
        assert!(
            matches!(
                first.request_status.as_str(),
                "submitted" | "locked" | "fulfilled" | "slashed" | "expired"
            ),
            "request_status should be valid: {}",
            first.request_status
        );
        assert!(
            !first.source.is_empty(),
            "source should not be empty"
        );
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
            assert!(
                addr.starts_with("0x"),
                "lock_prover_address should start with 0x: {}",
                addr
            );
            assert_eq!(
                addr.len(),
                42,
                "lock_prover_address should be 42 chars: {}",
                addr
            );
        }

        if let Some(ref addr) = first.fulfill_prover_address {
            assert!(
                addr.starts_with("0x"),
                "fulfill_prover_address should start with 0x: {}",
                addr
            );
            assert_eq!(
                addr.len(),
                42,
                "fulfill_prover_address should be 42 chars: {}",
                addr
            );
        }

        // Timestamp fields (Unix)
        assert!(
            first.created_at >= 0,
            "created_at should be non-negative: {}",
            first.created_at
        );
        assert!(
            first.updated_at >= 0,
            "updated_at should be non-negative: {}",
            first.updated_at
        );
        assert!(
            first.ramp_up_start >= 0,
            "ramp_up_start should be non-negative: {}",
            first.ramp_up_start
        );
        assert!(
            first.expires_at >= 0,
            "expires_at should be non-negative: {}",
            first.expires_at
        );
        assert!(
            first.lock_end >= 0,
            "lock_end should be non-negative: {}",
            first.lock_end
        );

        if let Some(locked_at) = first.locked_at {
            assert!(
                locked_at >= 0,
                "locked_at should be non-negative: {}",
                locked_at
            );
        }

        if let Some(fulfilled_at) = first.fulfilled_at {
            assert!(
                fulfilled_at >= 0,
                "fulfilled_at should be non-negative: {}",
                fulfilled_at
            );
        }

        if let Some(slashed_at) = first.slashed_at {
            assert!(
                slashed_at >= 0,
                "slashed_at should be non-negative: {}",
                slashed_at
            );
        }

        // Timestamp ISO fields
        assert!(
            !first.created_at_iso.is_empty(),
            "created_at_iso should not be empty"
        );
        assert!(
            first.created_at_iso.contains('T'),
            "created_at_iso should be ISO 8601 format: {}",
            first.created_at_iso
        );

        assert!(
            !first.updated_at_iso.is_empty(),
            "updated_at_iso should not be empty"
        );
        assert!(
            first.updated_at_iso.contains('T'),
            "updated_at_iso should be ISO 8601 format: {}",
            first.updated_at_iso
        );

        assert!(
            !first.ramp_up_start_iso.is_empty(),
            "ramp_up_start_iso should not be empty"
        );
        assert!(
            first.ramp_up_start_iso.contains('T'),
            "ramp_up_start_iso should be ISO 8601 format: {}",
            first.ramp_up_start_iso
        );

        assert!(
            !first.expires_at_iso.is_empty(),
            "expires_at_iso should not be empty"
        );
        assert!(
            first.expires_at_iso.contains('T'),
            "expires_at_iso should be ISO 8601 format: {}",
            first.expires_at_iso
        );

        assert!(
            !first.lock_end_iso.is_empty(),
            "lock_end_iso should not be empty"
        );
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
            assert!(
                submit_block >= 0,
                "submit_block should be non-negative: {}",
                submit_block
            );
        }

        if let Some(lock_block) = first.lock_block {
            assert!(
                lock_block >= 0,
                "lock_block should be non-negative: {}",
                lock_block
            );
        }

        if let Some(fulfill_block) = first.fulfill_block {
            assert!(
                fulfill_block >= 0,
                "fulfill_block should be non-negative: {}",
                fulfill_block
            );
        }

        if let Some(slashed_block) = first.slashed_block {
            assert!(
                slashed_block >= 0,
                "slashed_block should be non-negative: {}",
                slashed_block
            );
        }

        // Price and collateral fields (raw)
        assert!(
            !first.min_price.is_empty(),
            "min_price should not be empty"
        );
        assert!(
            first.min_price.chars().all(|c| c.is_ascii_digit()),
            "min_price should be numeric: {}",
            first.min_price
        );

        assert!(
            !first.max_price.is_empty(),
            "max_price should not be empty"
        );
        assert!(
            first.max_price.chars().all(|c| c.is_ascii_digit()),
            "max_price should be numeric: {}",
            first.max_price
        );

        assert!(
            !first.lock_collateral.is_empty(),
            "lock_collateral should not be empty"
        );
        assert!(
            first.lock_collateral.chars().all(|c| c.is_ascii_digit()),
            "lock_collateral should be numeric: {}",
            first.lock_collateral
        );

        // Price and collateral fields (formatted)
        assert!(
            !first.min_price_formatted.is_empty(),
            "min_price_formatted should not be empty"
        );
        assert!(
            !first.max_price_formatted.is_empty(),
            "max_price_formatted should not be empty"
        );
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
            assert!(
                !amount.is_empty(),
                "slash_transferred_amount should not be empty if Some"
            );
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
            assert!(
                !amount.is_empty(),
                "slash_burned_amount should not be empty if Some"
            );
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
        if let Some(cycles) = first.cycles {
            assert!(
                cycles >= 0,
                "cycles should be non-negative: {}",
                cycles
            );
        }

        if let Some(peak_prove_mhz) = first.peak_prove_mhz {
            assert!(
                peak_prove_mhz >= 0,
                "peak_prove_mhz should be non-negative: {}",
                peak_prove_mhz
            );
        }

        if let Some(effective_prove_mhz) = first.effective_prove_mhz {
            assert!(
                effective_prove_mhz >= 0,
                "effective_prove_mhz should be non-negative: {}",
                effective_prove_mhz
            );
        }

        // Transaction hashes
        if let Some(ref tx_hash) = first.submit_tx_hash {
            assert!(
                tx_hash.starts_with("0x"),
                "submit_tx_hash should start with 0x: {}",
                tx_hash
            );
            assert_eq!(
                tx_hash.len(),
                66,
                "submit_tx_hash should be 66 chars: {}",
                tx_hash
            );
        }

        if let Some(ref tx_hash) = first.lock_tx_hash {
            assert!(
                tx_hash.starts_with("0x"),
                "lock_tx_hash should start with 0x: {}",
                tx_hash
            );
            assert_eq!(
                tx_hash.len(),
                66,
                "lock_tx_hash should be 66 chars: {}",
                tx_hash
            );
        }

        if let Some(ref tx_hash) = first.fulfill_tx_hash {
            assert!(
                tx_hash.starts_with("0x"),
                "fulfill_tx_hash should start with 0x: {}",
                tx_hash
            );
            assert_eq!(
                tx_hash.len(),
                66,
                "fulfill_tx_hash should be 66 chars: {}",
                tx_hash
            );
        }

        if let Some(ref tx_hash) = first.slash_tx_hash {
            assert!(
                tx_hash.starts_with("0x"),
                "slash_tx_hash should start with 0x: {}",
                tx_hash
            );
            assert_eq!(
                tx_hash.len(),
                66,
                "slash_tx_hash should be 66 chars: {}",
                tx_hash
            );
        }

        // Image and predicate fields
        assert!(
            !first.image_id.is_empty(),
            "image_id should not be empty"
        );
        // image_id is typically 64 hex chars (32 bytes)
        assert!(
            first.image_id.len() == 64 || first.image_id.starts_with("0x"),
            "image_id should be 64 hex chars or start with 0x: {}",
            first.image_id
        );

        if let Some(ref image_url) = first.image_url {
            assert!(
                !image_url.is_empty(),
                "image_url should not be empty if Some"
            );
            assert!(
                image_url.starts_with("http://") || image_url.starts_with("https://") || image_url.starts_with("ipfs://") || image_url.starts_with("dweb://"),
                "image_url should be a valid URL: {}",
                image_url
            );
        }

        assert!(
            !first.selector.is_empty(),
            "selector should not be empty"
        );
        // selector is typically 8 hex chars (4 bytes)
        assert_eq!(
            first.selector.len(),
            8,
            "selector should be 8 hex chars: {}",
            first.selector
        );

        assert!(
            !first.predicate_type.is_empty(),
            "predicate_type should not be empty"
        );
        assert!(
            matches!(
                first.predicate_type.as_str(),
                "DigestMatch" | "PrefixMatch" | "ClaimDigestMatch"
            ),
            "predicate_type should be valid: {}",
            first.predicate_type
        );

        assert!(
            !first.predicate_data.is_empty(),
            "predicate_data should not be empty"
        );
        assert!(
            first.predicate_data.starts_with("0x"),
            "predicate_data should start with 0x: {}",
            first.predicate_data
        );

        // Input fields
        assert!(
            !first.input_type.is_empty(),
            "input_type should not be empty"
        );
        assert!(
            matches!(first.input_type.as_str(), "Inline" | "Url" | "Storage"),
            "input_type should be valid: {}",
            first.input_type
        );

        assert!(
            !first.input_data.is_empty(),
            "input_data should not be empty"
        );
        assert!(
            first.input_data.starts_with("0x"),
            "input_data should start with 0x: {}",
            first.input_data
        );

        // Fulfillment fields
        if let Some(ref fulfill_journal) = first.fulfill_journal {
            assert!(
                !fulfill_journal.is_empty(),
                "fulfill_journal should not be empty if Some"
            );
            assert!(
                fulfill_journal.starts_with("0x"),
                "fulfill_journal should start with 0x: {}",
                fulfill_journal
            );
        }

        if let Some(ref fulfill_seal) = first.fulfill_seal {
            assert!(
                !fulfill_seal.is_empty(),
                "fulfill_seal should not be empty if Some"
            );
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

    if let Some(first) = list_response.data.first() {
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
    if !response.data.is_empty() {
        let first = &response.data[0];

        // Verify timestamp_iso exists
        assert!(
            !first.timestamp_iso.is_empty(),
            "timestamp_iso should not be empty"
        );
        assert!(
            first.timestamp_iso.contains('T'),
            "timestamp_iso should be ISO 8601 format: {}",
            first.timestamp_iso
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
}

#[tokio::test]
#[ignore = "Requires BASE_MAINNET_RPC_URL"]
async fn test_market_requests_pagination() {
    let env = TestEnv::market().await;

    // Get first page
    let page1: RequestListResponse = env.get("/v1/market/requests?limit=2").await.unwrap();

    assert!(page1.data.len() <= 5);

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
