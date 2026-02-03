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

use crate::{indexer_client::IndexerClient, price_provider::PricePercentiles};
use alloy::primitives::{utils::format_ether, U256};
use httpmock::MockServer;
use url::Url;

/// Mock price provider for testing the PriceProvider trait.
///
/// This implementation allows tests to control the price range returned
/// and simulate failures, making it useful for testing price provider
/// integration without requiring a real indexer.
pub struct MockPriceProvider {
    price_percentiles: PricePercentiles,
    should_fail: bool,
}

impl MockPriceProvider {
    /// Creates a new mock price provider with the given price percentiles.
    ///
    /// # Arguments
    ///
    /// * `price_percentiles` - The price percentiles to return
    pub fn new(price_percentiles: PricePercentiles) -> Self {
        Self { price_percentiles, should_fail: false }
    }

    /// Configures the mock to fail when `price_percentiles()` is called.
    ///
    /// This is useful for testing error handling and fallback behavior.
    pub fn with_failure(mut self) -> Self {
        self.should_fail = true;
        self
    }
}

impl crate::price_provider::PriceProvider for MockPriceProvider {
    fn price_percentiles(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = anyhow::Result<crate::price_provider::PricePercentiles>,
                > + Send
                + '_,
        >,
    > {
        let should_fail = self.should_fail;
        Box::pin(async move {
            if should_fail {
                anyhow::bail!("Mock price provider failure")
            }
            Ok(self.price_percentiles.clone())
        })
    }
}

#[cfg(feature = "s3")]
pub use s3::S3Mock;

#[cfg(feature = "s3")]
mod s3 {
    use aws_config::{BehaviorVersion, Region, SdkConfig};
    use aws_sdk_s3::{
        config::{Credentials, SharedCredentialsProvider},
        Client as S3Client,
    };
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use s3s::{
        auth::SimpleAuth,
        host::SingleDomain,
        service::{S3ServiceBuilder, SharedS3Service},
    };
    use s3s_fs::FileSystem;
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    /// An ephemeral, self-contained S3 mock server for integration testing.
    pub struct S3Mock {
        endpoint: String,
        client: S3Client,
        server_handle: tokio::task::JoinHandle<()>,
        _temp_dir: TempDir,
    }

    impl Drop for S3Mock {
        fn drop(&mut self) {
            self.server_handle.abort();
        }
    }

    impl S3Mock {
        /// The default access key ID used by the mock server ("testkey").
        pub const ACCESS_KEY: &'static str = "testkey";
        /// The default secret access key used by the mock server ("testsecret").
        pub const SECRET_KEY: &'static str = "testsecret";
        /// The default AWS region configured for the client ("us-east-1").
        pub const REGION: &'static str = "us-east-1";
        /// The name of the default bucket created on startup ("test-bucket").
        pub const BUCKET: &'static str = "test-bucket";

        /// Starts a new S3 mock server on a random local IPv4 port.
        ///
        /// This initializes a temporary filesystem, spawns the server background task,
        /// and pre-creates a default bucket named "test-bucket".
        pub async fn start() -> anyhow::Result<Self> {
            let temp_dir = TempDir::new()?;

            let service: SharedS3Service = {
                let fs = FileSystem::new(temp_dir.path()).expect("create s3s-fs");
                let mut builder = S3ServiceBuilder::new(fs);
                builder.set_host(SingleDomain::new("localhost")?);
                builder.set_auth(SimpleAuth::from_single(Self::ACCESS_KEY, Self::SECRET_KEY));
                builder.build().into_shared()
            };

            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("http://{}", listener.local_addr()?);

            let server_handle = Self::spawn_server(listener, service);

            let cred = Credentials::new(Self::ACCESS_KEY, Self::SECRET_KEY, None, None, "test");
            let sdk_config = SdkConfig::builder()
                .behavior_version(BehaviorVersion::latest())
                .credentials_provider(SharedCredentialsProvider::new(cred))
                .region(Region::new(Self::REGION))
                .endpoint_url(&endpoint)
                .build();

            let s3_config_builder = aws_sdk_s3::config::Builder::from(&sdk_config);
            let client = S3Client::from_conf(s3_config_builder.build());

            client.create_bucket().bucket(Self::BUCKET).send().await?;

            Ok(Self { endpoint, client, server_handle, _temp_dir: temp_dir })
        }

        /// Returns the HTTP URL of the running server (e.g., "http://127.0.0.1:54321").
        pub fn endpoint(&self) -> &str {
            &self.endpoint
        }

        /// Returns a pre-configured AWS S3 client authenticated against this mock server.
        pub fn client(&self) -> &S3Client {
            &self.client
        }

        /// Returns environment variables configured to target this mock server.
        pub fn env_vars(&self) -> [(String, Option<String>); 4] {
            [
                ("AWS_ACCESS_KEY_ID", Some(Self::ACCESS_KEY)),
                ("AWS_SECRET_ACCESS_KEY", Some(Self::SECRET_KEY)),
                ("AWS_REGION", Some(Self::REGION)),
                ("S3_URL", Some(&self.endpoint)),
            ]
            .map(|(k, v)| (k.to_string(), v.map(|s| s.to_string())))
        }

        fn spawn_server(
            listener: TcpListener,
            service: SharedS3Service,
        ) -> tokio::task::JoinHandle<()> {
            tokio::spawn(async move {
                let http = hyper_util::server::conn::auto::Builder::new(TokioExecutor::new());

                while let Ok((stream, _)) = listener.accept().await {
                    let svc = service.clone();
                    let http = http.clone();

                    tokio::spawn(async move {
                        let _ = http.serve_connection(TokioIo::new(stream), svc).await;
                    });
                }
            })
        }
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
pub fn create_mock_indexer_client(
    price_percentiles: &PricePercentiles,
) -> (MockServer, IndexerClient) {
    let server = MockServer::start();

    let p10_str = format!("{}", price_percentiles.p10);
    let p25_str = format!("{}", price_percentiles.p25);
    let p50_str = format!("{}", price_percentiles.p50);
    let p75_str = format!("{}", price_percentiles.p75);
    let p90_str = format!("{}", price_percentiles.p90);
    let p95_str = format!("{}", price_percentiles.p95);
    let p99_str = format!("{}", price_percentiles.p99);
    let p10_formatted = format_eth_trimmed(price_percentiles.p10);
    let p25_formatted = format_eth_trimmed(price_percentiles.p25);
    let p50_formatted = format_eth_trimmed(price_percentiles.p50);
    let p75_formatted = format_eth_trimmed(price_percentiles.p75);
    let p90_formatted = format_eth_trimmed(price_percentiles.p90);
    let p95_formatted = format_eth_trimmed(price_percentiles.p95);
    let p99_formatted = format_eth_trimmed(price_percentiles.p99);

    let response_json = format!(
        r#"{{"chain_id":1,"aggregation":"weekly","data":[{{"chain_id":1,"timestamp":1234567890,"timestamp_iso":"2009-02-13T23:31:30Z","total_fulfilled":500,"unique_provers_locking_requests":10,"unique_requesters_submitting_requests":20,"total_fees_locked":"5000000000000000000","total_fees_locked_formatted":"5.0 ETH","total_collateral_locked":"10000000000000000000","total_collateral_locked_formatted":"10.0 ZKC","total_locked_and_expired_collateral":"0","total_locked_and_expired_collateral_formatted":"0.0 ZKC","p10_lock_price_per_cycle":"{}","p10_lock_price_per_cycle_formatted":"{}","p25_lock_price_per_cycle":"{}","p25_lock_price_per_cycle_formatted":"{}","p50_lock_price_per_cycle":"{}","p50_lock_price_per_cycle_formatted":"{}","p75_lock_price_per_cycle":"{}","p75_lock_price_per_cycle_formatted":"{}","p90_lock_price_per_cycle":"{}","p90_lock_price_per_cycle_formatted":"{}","p95_lock_price_per_cycle":"{}","p95_lock_price_per_cycle_formatted":"{}","p99_lock_price_per_cycle":"{}","p99_lock_price_per_cycle_formatted":"{}","total_requests_submitted":750,"total_requests_submitted_onchain":500,"total_requests_submitted_offchain":250,"total_requests_locked":400,"total_requests_slashed":5,"total_expired":50,"total_locked_and_expired":25,"total_locked_and_fulfilled":375,"total_secondary_fulfillments":10,"locked_orders_fulfillment_rate":93.75,"total_program_cycles":"1000000","total_cycles":"1200000"}}],"next_cursor":null,"has_more":false}}"#,
        p10_str,
        p10_formatted,
        p25_str,
        p25_formatted,
        p50_str,
        p50_formatted,
        p75_str,
        p75_formatted,
        p90_str,
        p90_formatted,
        p95_str,
        p95_formatted,
        p99_str,
        p99_formatted
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
