// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{path::PathBuf, time::Duration};

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Result};
use boundless_slasher::SlashService;
use clap::{Args, Parser};
use url::Url;

/// Arguments of the order generator.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Private key used to sign and submit requests.
    #[clap(long, env)]
    private_key: PrivateKeySigner,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// Interval in seconds between expired request checks.
    #[clap(short, long, default_value = "60")]
    interval: u64,
}

#[derive(Args, Clone, Debug)]
#[group(required = false, multiple = false)]
struct OrderInput {
    /// Input for the guest, given as a hex-encoded string.
    #[clap(long, value_parser = |s: &str| hex::decode(s))]
    input: Option<Vec<u8>>,
    /// Input for the guest, given as a path to a file.
    #[clap(long)]
    input_file: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = MainArgs::parse();

    // NOTE: Using a separate `run` function to facilitate testing below.
    run(&args).await?;

    Ok(())
}

async fn run(args: &MainArgs) -> Result<()> {
    let slash_service = SlashService::new(
        args.rpc_url.clone(),
        args.private_key.clone(),
        args.boundless_market_address,
        Duration::from_secs(args.interval),
    )
    .await?;

    if let Err(err) = slash_service.run().await {
        bail!("Error running the slasher: {err}");
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use alloy::{
//         node_bindings::Anvil, providers::Provider, rpc::types::Filter, sol_types::SolEvent,
//     };
//     use boundless_market::contracts::{test_utils::TestCtx, IBoundlessMarket};
//     use guest_assessor::ASSESSOR_GUEST_ID;
//     use guest_set_builder::SET_BUILDER_ID;
//     use risc0_zkvm::sha::Digest;
//     use tracing_test::traced_test;

//     use super::*;

//     #[tokio::test]
//     #[traced_test]
//     async fn test_main() {
//         let anvil = Anvil::new().spawn();
//         let ctx =
//             TestCtx::new(&anvil, Digest::from(SET_BUILDER_ID), Digest::from(ASSESSOR_GUEST_ID))
//                 .await
//                 .unwrap();

//         let args = MainArgs {
//             rpc_url: anvil.endpoint_url(),
//             order_stream_url: None,
//             storage_config: Some(StorageProviderConfig::dev_mode()),
//             private_key: ctx.customer_signer,
//             set_verifier_address: ctx.set_verifier_addr,
//             boundless_market_address: ctx.boundless_market_addr,
//             interval: 1,
//             count: Some(2),
//             min_price_per_mcycle: parse_ether("0.001").unwrap(),
//             max_price_per_mcycle: parse_ether("0.002").unwrap(),
//             lockin_stake: parse_ether("0.0").unwrap(),
//             bidding_start_offset: 5,
//             ramp_up: 0,
//             timeout: 1000,
//             elf: None,
//             input: OrderInput { input: None, input_file: None },
//             encode_input: false,
//         };

//         run(&args).await.unwrap();

//         // Check that the requests were submitted
//         let filter = Filter::new()
//             .event_signature(IBoundlessMarket::RequestSubmitted::SIGNATURE_HASH)
//             .from_block(0)
//             .address(ctx.boundless_market_addr);
//         let logs = ctx.customer_provider.get_logs(&filter).await.unwrap();
//         let decoded_logs = logs.iter().filter_map(|log| {
//             match log.log_decode::<IBoundlessMarket::RequestSubmitted>() {
//                 Ok(res) => Some(res),
//                 Err(err) => {
//                     tracing::error!("Failed to decode RequestSubmitted log: {err:?}");
//                     None
//                 }
//             }
//         });
//         assert!(decoded_logs.count() == 2);
//     }
// }
