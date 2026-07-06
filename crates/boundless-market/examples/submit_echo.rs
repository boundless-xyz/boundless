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

use alloy::{primitives::FixedBytes, signers::local::PrivateKeySigner};
use anyhow::Result;
use boundless_market::{
    request_builder::{OfferParams, RequirementParams},
    risc0::Risc0ZkvmOps,
    selector::is_blake3_groth16_selector,
    Client, Deployment, StorageUploaderConfig,
};
use clap::Parser;
use guest_util::ECHO_ELF;
use tracing_subscriber::EnvFilter;
use url::Url;

/// Echoed by the guest into the journal.
const ECHO_INPUT: &[u8] = b"Hello, Boundless!";
/// Blake3-groth16 seals commit to a 32-byte journal, so blake3 requests echo this instead.
const ECHO_INPUT_32: &[u8] = b"Hello, Boundless! (32-byte ver.)";

#[derive(Parser, Debug)]
#[clap(about = "Submit a proof request using the ECHO program")]
struct Args {
    #[clap(short, long, env)]
    rpc_url: Url,

    #[clap(long, env)]
    requestor_key: PrivateKeySigner,

    /// Requirement selector(s), one request per value: a router class id (e.g. 0xaa000003),
    /// an entry selector (e.g. 0x73c457ba), or 0x00000000 for the chain default.
    /// Accepts repeats and comma lists; defaults to a single chain-default request.
    /// Blake3 selectors require building with --features blake3-groth16.
    #[clap(long, value_delimiter = ',')]
    selectors: Vec<FixedBytes<4>>,

    #[clap(flatten)]
    storage_config: StorageUploaderConfig,

    #[clap(flatten)]
    deployment: Option<Deployment>,

    #[clap(flatten, next_help_heading = "Offer")]
    offer_params: OfferParams,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();

    // Create Boundless client
    let client = Client::builder()
        .with_zkvm(Risc0ZkvmOps::new().await)
        .with_rpc_url(args.rpc_url)
        .with_deployment(args.deployment)
        .with_uploader_config(&args.storage_config)
        .await?
        .with_private_key(args.requestor_key)
        .build()
        .await?;

    let selectors =
        if args.selectors.is_empty() { vec![FixedBytes::ZERO] } else { args.selectors.clone() };

    // Submit one request per selector, then wait for all fulfillments.
    let mut pending = Vec::with_capacity(selectors.len());
    for selector in selectors {
        let blake3 = is_blake3_groth16_selector(selector);
        let stdin = if blake3 { ECHO_INPUT_32 } else { ECHO_INPUT };

        #[cfg(not(feature = "blake3-groth16"))]
        if blake3 {
            anyhow::bail!("selector {selector} requires building with --features blake3-groth16");
        }

        let mut requirements = RequirementParams::builder();
        requirements.selector(selector);

        // Build and submit request using the builder pattern
        let request = client
            .new_request()
            .with_program(ECHO_ELF)
            .with_stdin(stdin)
            .with_requirements(requirements)
            .with_offer(args.offer_params.clone());

        let (request_id, expires_at) = client.submit(request).await?;
        println!("Submitted request {request_id:x} (selector {selector})");
        pending.push((selector, request_id, expires_at));
    }

    // Wait for fulfillment
    for (selector, request_id, expires_at) in pending {
        let fulfillment = client
            .wait_for_request_fulfillment(request_id, Duration::from_secs(5), expires_at)
            .await?;

        // The journal is absent for claim-digest-match fulfillments (e.g. blake3-groth16), where
        // verification binds the claim digest rather than the journal bytes.
        match fulfillment.data()?.journal() {
            Some(journal) => println!(
                "Fulfilled {selector} ({request_id:x})! Journal: {}",
                String::from_utf8_lossy(journal)
            ),
            None => println!("Fulfilled {selector} ({request_id:x})! (no journal in fulfillment)"),
        }
    }
    Ok(())
}
