// Copyright 2025 RISC Zero, Inc.
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

use alloy::primitives::Address;
use anyhow::{bail, Result};
use boundless_indexer::rewards::{RewardsIndexerService, RewardsIndexerServiceConfig};
use clap::Parser;
use url::Url;

/// Arguments for the rewards indexer.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct RewardsIndexerArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,

    /// Address of the veZKC (staking) contract.
    #[clap(long, env)]
    vezkc_address: Address,

    /// Address of the ZKC token contract.
    #[clap(long, env)]
    zkc_address: Address,

    /// Address of the PoVW Accounting contract.
    #[clap(long, env)]
    povw_accounting_address: Address,

    /// DB connection string.
    #[clap(long, env = "DATABASE_URL")]
    db: String,

    /// Starting block number (if not set, uses chain-specific defaults).
    #[clap(long)]
    start_block: Option<u64>,

    /// Interval in seconds between checking for new events.
    #[clap(long, default_value = "600")]
    interval: u64,

    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "3")]
    retries: u32,

    /// Whether to log in JSON format.
    #[clap(long, env, default_value_t = false)]
    log_json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = RewardsIndexerArgs::parse();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        .from_env_lossy();

    if args.log_json {
        tracing_subscriber::fmt().with_ansi(false).json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_ansi(false).with_env_filter(filter).init();
    }

    let config = RewardsIndexerServiceConfig {
        interval: Duration::from_secs(args.interval),
        retries: args.retries,
        start_block: args.start_block,
    };

    let mut service = RewardsIndexerService::new(
        args.rpc_url,
        args.vezkc_address,
        args.zkc_address,
        args.povw_accounting_address,
        &args.db,
        config,
    )
    .await?;

    let mut failures = 0u32;
    loop {
        match service.run().await {
            Ok(_) => {
                failures = 0;
                tracing::info!("Sleeping for {} seconds", args.interval);
                tokio::time::sleep(Duration::from_secs(args.interval)).await;
            }
            Err(e) => {
                failures += 1;
                tracing::error!("Error running rewards indexer: {:?}", e);
                if failures >= args.retries {
                    bail!("Maximum retries reached");
                }
                tracing::info!("Retrying in {} seconds", args.interval);
                tokio::time::sleep(Duration::from_secs(args.interval)).await;
            }
        }
    }
}