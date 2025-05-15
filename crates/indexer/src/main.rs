// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::time::Duration;

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Result};
use boundless_indexer::{IndexerService, IndexerServiceConfig};
use clap::Parser;
use url::Url;

/// Arguments of the indexer.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// DB connection string.
    #[clap(long, env = "DATABASE_URL")]
    db: String,
    /// Starting block number.
    #[clap(long)]
    start_block: Option<u64>,
    /// Interval in seconds between checking for new events.
    #[clap(long, default_value = "5")]
    interval: u64,
    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "10")]
    retries: u32,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = MainArgs::parse();

    let mut indexer_service = IndexerService::new(
        args.rpc_url.clone(),
        &PrivateKeySigner::random(),
        args.boundless_market_address,
        &args.db,
        IndexerServiceConfig {
            interval: Duration::from_secs(args.interval),
            retries: args.retries,
        },
    )
    .await?;

    if let Err(err) = indexer_service.run(args.start_block).await {
        bail!("Error running the indexer: {err}");
    }

    Ok(())
}
