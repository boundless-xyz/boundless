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

use std::time::Duration;

use alloy::signers::local::PrivateKeySigner;
use anyhow::Result;
use boundless_market::{Client, Deployment, StorageUploaderConfig};
use clap::Parser;
use guest_util::ECHO_ELF;
use tracing_subscriber::EnvFilter;
use url::Url;

#[derive(Parser, Debug)]
#[clap(about = "Submit a proof request using the ECHO program")]
struct Args {
    #[clap(short, long, env)]
    rpc_url: Url,

    #[clap(long, env)]
    requestor_key: PrivateKeySigner,

    #[clap(flatten)]
    storage_config: StorageUploaderConfig,

    #[clap(flatten)]
    deployment: Option<Deployment>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();

    // Create Boundless client
    let client = Client::builder()
        .with_rpc_url(args.rpc_url)
        .with_deployment(args.deployment)
        .with_uploader_config(&args.storage_config)
        .await?
        .with_private_key(args.requestor_key)
        .build()
        .await?;

    // Build and submit request using the builder pattern
    let request = client.new_request().with_program(ECHO_ELF).with_stdin(b"Hello, Boundless!");

    let (request_id, expires_at) = client.submit(request).await?;
    println!("Submitted request: {:x}", request_id);

    // Wait for fulfillment
    let fulfillment =
        client.wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at).await?;

    println!(
        "Fulfilled! Journal: {}",
        String::from_utf8_lossy(fulfillment.data()?.journal().unwrap())
    );
    Ok(())
}
