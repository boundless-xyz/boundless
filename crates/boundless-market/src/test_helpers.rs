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

use crate::indexer_client::IndexerClient;
use alloy::primitives::{utils::format_ether, U256};
use httpmock::MockServer;
use url::Url;

/// Mock price provider for testing the PriceProvider trait.
///
/// This implementation allows tests to control the price range returned
/// and simulate failures, making it useful for testing price provider
/// integration without requiring a real indexer.
///
/// # Example
///
/// ```rust,no_run
/// # use boundless_test_utils::market::MockPriceProvider;
/// # use alloy::primitives::U256;
/// let provider = MockPriceProvider::new(
///     U256::from(1000u64), // min price
///     U256::from(5000u64), // max price
/// );
/// ```
pub struct MockPriceProvider {
    min_price: U256,
    max_price: U256,
    should_fail: bool,
}

impl MockPriceProvider {
    /// Creates a new mock price provider with the given min and max prices.
    ///
    /// # Arguments
    ///
    /// * `min_price` - The minimum price (p10 percentile) to return
    /// * `max_price` - The maximum price (p99 percentile) to return
    pub fn new(min_price: U256, max_price: U256) -> Self {
        Self { min_price, max_price, should_fail: false }
    }

    /// Configures the mock to fail when `price_range()` is called.
    ///
    /// This is useful for testing error handling and fallback behavior.
    pub fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }
}

impl crate::request_builder::PriceProvider for MockPriceProvider {
    fn price_range(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<crate::indexer_client::PriceRange>>
                + '_,
        >,
    > {
        let min = self.min_price;
        let max = self.max_price;
        let should_fail = self.should_fail;
        Box::pin(async move {
            if should_fail {
                anyhow::bail!("Mock price provider failure")
            }
            Ok(crate::indexer_client::PriceRange { min, max })
        })
    }
}

/// Formats a U256 wei value as ETH string, removing trailing zeros.
fn format_eth_trimmed(wei: U256) -> String {
    let formatted = format_ether(wei);
    // Remove trailing zeros and decimal point if needed
    let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
    format!("{} ETH", trimmed)
}

/// Creates a test IndexerClient connected to a mock server.
///
/// This is a general-purpose helper for creating an IndexerClient that points to
/// a mock HTTP server. The mock server must remain alive for the client to work,
/// so it's returned along with the client.
pub fn create_test_indexer_client() -> (MockServer, IndexerClient) {
    let server = MockServer::start();
    let base_url = Url::parse(&server.base_url()).unwrap();
    let indexer_client =
        IndexerClient::new(base_url).expect("Failed to create IndexerClient from mock server URL");
    (server, indexer_client)
}

/// Creates a mock IndexerClient that returns price data for hourly aggregates (past day).
pub fn create_mock_indexer_client_hourly(
    p10_price: U256,
    p99_price: U256,
) -> (MockServer, IndexerClient) {
    let server = MockServer::start();

    let p10_str = format!("{}", p10_price);
    let p99_str = format!("{}", p99_price);
    // Format wei to ETH using alloy's format_ether utility, trimming trailing zeros
    let p10_formatted = format_eth_trimmed(p10_price);
    let p99_formatted = format_eth_trimmed(p99_price);

    let response_json = format!(
        r#"{{"chain_id":1,"aggregation":"hourly","data":[{{"chain_id":1,"timestamp":1234567890,"timestamp_iso":"2009-02-13T23:31:30Z","total_fulfilled":100,"unique_provers_locking_requests":5,"unique_requesters_submitting_requests":10,"total_fees_locked":"1000000000000000000","total_fees_locked_formatted":"1.0 ETH","total_collateral_locked":"2000000000000000000","total_collateral_locked_formatted":"2.0 ZKC","total_locked_and_expired_collateral":"0","total_locked_and_expired_collateral_formatted":"0.0 ZKC","p10_lock_price_per_cycle":"{}","p10_lock_price_per_cycle_formatted":"{}","p25_lock_price_per_cycle":"2000000000000000","p25_lock_price_per_cycle_formatted":"0.002 ETH","p50_lock_price_per_cycle":"3000000000000000","p50_lock_price_per_cycle_formatted":"0.003 ETH","p75_lock_price_per_cycle":"4000000000000000","p75_lock_price_per_cycle_formatted":"0.004 ETH","p90_lock_price_per_cycle":"5000000000000000","p90_lock_price_per_cycle_formatted":"0.005 ETH","p95_lock_price_per_cycle":"6000000000000000","p95_lock_price_per_cycle_formatted":"0.006 ETH","p99_lock_price_per_cycle":"{}","p99_lock_price_per_cycle_formatted":"{}","total_requests_submitted":150,"total_requests_submitted_onchain":100,"total_requests_submitted_offchain":50,"total_requests_locked":80,"total_requests_slashed":2,"total_expired":10,"total_locked_and_expired":5,"total_locked_and_fulfilled":75,"total_secondary_fulfillments":3,"locked_orders_fulfillment_rate":93.75,"total_program_cycles":"1000000","total_cycles":"1200000"}}],"next_cursor":null,"has_more":false}}"#,
        p10_str, p10_formatted, p99_str, p99_formatted
    );

    let _mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/v1/market/aggregates")
            .query_param("aggregation", "hourly")
            .query_param("limit", "24")
            .query_param("sort", "desc");
        then.status(200).body(response_json);
    });

    let base_url = Url::parse(&server.base_url()).unwrap();
    let indexer_client =
        IndexerClient::new(base_url).expect("Failed to create IndexerClient from mock server URL");

    (server, indexer_client)
}

/// Creates a mock IndexerClient that returns the specified price percentiles.
pub fn create_mock_indexer_client(p10_price: U256, p99_price: U256) -> (MockServer, IndexerClient) {
    let server = MockServer::start();

    let p10_str = format!("{}", p10_price);
    let p99_str = format!("{}", p99_price);
    // Format wei to ETH using alloy's format_ether utility, trimming trailing zeros
    let p10_formatted = format_eth_trimmed(p10_price);
    let p99_formatted = format_eth_trimmed(p99_price);

    let response_json = format!(
        r#"{{"chain_id":1,"aggregation":"weekly","data":[{{"chain_id":1,"timestamp":1234567890,"timestamp_iso":"2009-02-13T23:31:30Z","total_fulfilled":500,"unique_provers_locking_requests":10,"unique_requesters_submitting_requests":20,"total_fees_locked":"5000000000000000000","total_fees_locked_formatted":"5.0 ETH","total_collateral_locked":"10000000000000000000","total_collateral_locked_formatted":"10.0 ZKC","total_locked_and_expired_collateral":"0","total_locked_and_expired_collateral_formatted":"0.0 ZKC","p10_lock_price_per_cycle":"{}","p10_lock_price_per_cycle_formatted":"{}","p25_lock_price_per_cycle":"3000000000000000","p25_lock_price_per_cycle_formatted":"0.003 ETH","p50_lock_price_per_cycle":"4000000000000000","p50_lock_price_per_cycle_formatted":"0.004 ETH","p75_lock_price_per_cycle":"5000000000000000","p75_lock_price_per_cycle_formatted":"0.005 ETH","p90_lock_price_per_cycle":"6000000000000000","p90_lock_price_per_cycle_formatted":"0.006 ETH","p95_lock_price_per_cycle":"6500000000000000","p95_lock_price_per_cycle_formatted":"0.0065 ETH","p99_lock_price_per_cycle":"{}","p99_lock_price_per_cycle_formatted":"{}","total_requests_submitted":750,"total_requests_submitted_onchain":500,"total_requests_submitted_offchain":250,"total_requests_locked":400,"total_requests_slashed":5,"total_expired":50,"total_locked_and_expired":25,"total_locked_and_fulfilled":375,"total_secondary_fulfillments":10,"locked_orders_fulfillment_rate":93.75,"total_program_cycles":"1000000","total_cycles":"1200000"}}],"next_cursor":null,"has_more":false}}"#,
        p10_str, p10_formatted, p99_str, p99_formatted
    );

    let _mock = server.mock(|when, then| {
        when.method(httpmock::Method::GET)
            .path("/v1/market/aggregates")
            .query_param("aggregation", "weekly")
            .query_param("limit", "1")
            .query_param("sort", "desc");
        then.status(200).body(response_json);
    });

    let base_url = Url::parse(&server.base_url()).unwrap();
    let indexer_client =
        IndexerClient::new(base_url).expect("Failed to create IndexerClient from mock server URL");

    (server, indexer_client)
}
