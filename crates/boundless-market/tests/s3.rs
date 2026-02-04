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

//! Integration tests for ClientBuilder with S3 storage using s3s-fs.
//!
//! Uses s3s-fs (pure Rust, no Docker required) for the S3 backend via S3Mock.

use alloy::node_bindings::Anvil;
use boundless_market::{
    client::ClientBuilder,
    contracts::RequestInputType,
    storage::{S3StorageDownloader, StorageDownloader, StorageUploaderType},
    test_helpers::S3Mock,
    StorageUploaderConfig,
};
use boundless_test_utils::{guests::ECHO_ELF, market::create_test_ctx};
use tracing_test::traced_test;
use url::Url;

#[tokio::test]
#[traced_test]
async fn test_s3_storage() {
    let s3 = S3Mock::start().await.expect("start S3Mock");
    temp_env::async_with_vars(s3.env_vars(), async move {
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();

        let config = StorageUploaderConfig::builder()
            .storage_uploader(StorageUploaderType::S3)
            .s3_bucket(S3Mock::BUCKET)
            .s3_url(s3.endpoint())
            .aws_access_key_id(S3Mock::ACCESS_KEY)
            .aws_secret_access_key(S3Mock::SECRET_KEY)
            .aws_region(S3Mock::REGION)
            .s3_presigned(false)
            .build()
            .expect("build storage uploader config");

        // Build client with s3:// storage
        let client = ClientBuilder::new()
            .with_signer(ctx.customer_signer.clone())
            .with_deployment(ctx.deployment.clone())
            .with_rpc_url(Url::parse(&anvil.endpoint()).unwrap())
            .with_uploader_config(&config)
            .await
            .expect("set storage uploader config")
            .with_skip_preflight(false)
            .config_storage_layer(|config| config.inline_input_max_bytes(0)) // Force URL uploads
            .build()
            .await
            .expect("build client");

        // Build request - should upload program and input to S3
        let request_params = client
            .new_request()
            .with_program(ECHO_ELF)
            .with_stdin(b"test input for s3 integration");
        let request = client.build_request(request_params).await.expect("build request");

        // Verify URLs are s3://
        let input_url = get_input_url(&request).expect("expected URL input type");
        assert_eq!(input_url.scheme(), "s3", "input URL should be s3://");
        assert!(request.imageUrl.starts_with("s3://"), "image URL should be s3://");

        let (request_id, _block) = client.submit_request(&request).await.expect("submit request");
        assert!(request_id > alloy::primitives::U256::ZERO);

        // Verify we can download the uploaded content
        // Note: S3StorageDownloader reads creds/region from the env vars set by temp_env
        let downloader = S3StorageDownloader::new(None).await.expect("create S3 downloader");

        let downloaded_input = downloader.download_url(input_url).await.expect("download input");
        assert!(!downloaded_input.is_empty());

        let downloaded_image =
            downloader.download(&request.imageUrl).await.expect("download image");
        assert_eq!(downloaded_image, ECHO_ELF);
    })
    .await;
}

fn get_input_url(request: &boundless_market::contracts::ProofRequest) -> Option<Url> {
    if request.input.inputType == RequestInputType::Url {
        let url_str = std::str::from_utf8(&request.input.data).ok()?;
        Url::parse(url_str).ok()
    } else {
        None
    }
}
