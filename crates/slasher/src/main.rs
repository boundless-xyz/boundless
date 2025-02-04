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
    /// Private key used to sign and submit slash requests.
    #[clap(short, long, env)]
    private_key: PrivateKeySigner,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// DB connection string.
    #[clap(long, default_value = "sqlite::memory:")]
    db: String,
    /// Starting block number.
    #[clap(long)]
    start_block: Option<u64>,
    /// Interval in seconds between checking for expired requests.
    #[clap(long, default_value = "5")]
    interval: u64,
    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "10")]
    retries: u32,
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
        &args.private_key,
        args.boundless_market_address,
        &args.db,
        Duration::from_secs(args.interval),
        args.retries,
    )
    .await?;

    if let Err(err) = slash_service.run(args.start_block).await {
        bail!("Error running the slasher: {err}");
    }

    Ok(())
}
