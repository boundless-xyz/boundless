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

use std::{str::FromStr, time::Duration};

use alloy::{primitives::utils::parse_ether, signers::local::PrivateKeySigner};
use anyhow::{Context, Result};
use blake3_groth16::Blake3Groth16ReceiptClaim;
use boundless_market::{
    request_builder::OfferParamsBuilder, Client, Deployment, StorageProviderConfig,
};
use clap::Parser;
use guest_util::{ECHO_ELF, ECHO_ID};
use tracing_subscriber::{filter::LevelFilter, prelude::*, EnvFilter};
use url::Url;

/// Arguments for the blake3-groth16 example app CLI.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to interact with the Boundless Market.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Configuration for the StorageProvider to use for uploading programs and inputs.
    #[clap(flatten, next_help_heading = "Storage Provider")]
    storage_config: StorageProviderConfig,
    #[clap(flatten, next_help_heading = "Boundless Market Deployment")]
    deployment: Option<Deployment>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            EnvFilter::builder()
                .with_default_directive(LevelFilter::from_str("info")?.into())
                .from_env_lossy(),
        )
        .init();

    let args = Args::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(args).await
}

/// Main logic which creates the Boundless client and request the proof.
async fn run(args: Args) -> Result<()> {
    // Create a Boundless client from the provided parameters.
    let client = Client::builder()
        .with_rpc_url(args.rpc_url)
        .with_deployment(args.deployment)
        .with_storage_provider_config(&args.storage_config)?
        .with_private_key(args.private_key)
        .build()
        .await
        .context("failed to build boundless client")?;

    // Use the default ECHO program with 32 bytes as input
    // We use exactly 32 bytes as Blake3Groth16 proofs require a 32-byte journal,
    // and the ECHO program will produce a journal of that size.
    let echo_message = [255u8; 32].to_vec();

    let request = client
        .new_request()
        .with_program(ECHO_ELF)
        .with_blake3_groth16_proof() // Specify Blake3Groth16 proof
        .with_stdin(echo_message.clone())
        .with_offer(
            OfferParamsBuilder::default()
                .min_price(parse_ether("0.001")?)
                .max_price(parse_ether("0.002")?),
        );

    let (request_id, expires_at) = client.submit(request).await?;

    // Wait for the request to be fulfilled.
    tracing::info!("Waiting for request 0x{:x} to be fulfilled", request_id);
    let fulfillment = client
        .wait_for_request_fulfillment(
            request_id,
            Duration::from_secs(5), // check every 5 seconds
            expires_at,
        )
        .await?;

    tracing::info!("Fulfillment seal: {:x}", fulfillment.seal);
    tracing::info!("Request 0x{:x} fulfilled", request_id);

    if is_dev_mode() {
        tracing::warn!("Running in dev mode: skipping seal verification");
        return Ok(());
    }
    // Verify the Blake3Groth16 seal.
    let blake3_claim_digest = Blake3Groth16ReceiptClaim::ok(ECHO_ID, echo_message).claim_digest();
    // The first 4 bytes of the seal are the selector, so we skip them.
    blake3_groth16::verify::verify_seal(&fulfillment.seal[4..], blake3_claim_digest)
        .context("failed to verify Blake3Groth16 seal")?;

    Ok(())
}

fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::node_bindings::Anvil;
    use boundless_market::contracts::hit_points::default_allowance;
    use boundless_market::storage::StorageProviderType;
    use boundless_test_utils::market::create_test_ctx;
    use broker::test_utils::BrokerBuilder;
    use test_log::test;
    use tokio::task::JoinSet;

    #[test(tokio::test)]
    async fn test_main() -> Result<()> {
        // Setup anvil and deploy contracts.
        let anvil = Anvil::new().spawn();
        let ctx = create_test_ctx(&anvil).await.unwrap();
        ctx.prover_market
            .deposit_collateral_with_permit(default_allowance(), &ctx.prover_signer)
            .await
            .unwrap();

        // A JoinSet automatically aborts all its tasks when dropped
        let mut tasks = JoinSet::new();

        // Start a broker.
        let (broker, _config) =
            BrokerBuilder::new_test(&ctx, anvil.endpoint_url()).await.build().await?;
        tasks.spawn(async move { broker.start_service().await });

        const TIMEOUT_SECS: u64 = 600; // 10 minutes

        let run_task = run(Args {
            rpc_url: anvil.endpoint_url(),
            private_key: ctx.customer_signer,
            storage_config: StorageProviderConfig::builder()
                .storage_provider(StorageProviderType::Mock)
                .build()
                .unwrap(),
            deployment: Some(ctx.deployment),
        });

        // Run with properly handled cancellation.
        tokio::select! {
            run_result = run_task => run_result?,

            broker_task_result = tasks.join_next() => {
                panic!("Broker exited unexpectedly: {:?}", broker_task_result.unwrap());
            },

            _ = tokio::time::sleep(Duration::from_secs(TIMEOUT_SECS)) => {
                panic!("The run function did not complete within {TIMEOUT_SECS} seconds")
            }
        }

        Ok(())
    }
}
