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

    /// Ending block number (must be provided together with --end-epoch).
    #[clap(long, requires = "end_epoch")]
    end_block: Option<u64>,

    /// Ending epoch number (must be provided together with --end-block).
    #[clap(long, requires = "end_block")]
    end_epoch: Option<u64>,

    /// Interval in seconds between checking for new events.
    #[clap(long, default_value = "600")]
    interval: u64,

    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "3")]
    retries: u32,

    /// Number of blocks to process in each batch.
    #[clap(long, default_value = "500")]
    batch_size: u64,

    /// Whether to log in JSON format.
    #[clap(long, env, default_value_t = false)]
    log_json: bool,

    /// Number of epochs back from the current block to index each time. Only valid when
    /// end_block and end_epoch are not provided. Defaults to process all epochs.
    /// Typically only used for testing, as to get accurate aggregate counts we only support reindexing from 0.
    #[clap(long, conflicts_with_all = ["end_block", "end_epoch"])]
    epochs_to_process: Option<u64>,

    /// Number of blocks to query in each chunk when fetching event logs.
    #[clap(long, default_value = "50000")]
    block_chunk_size: u64,
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
        batch_size: args.batch_size,
        block_chunk_size: args.block_chunk_size,
        start_block: args.start_block,
        end_block: args.end_block,
        end_epoch: args.end_epoch,
        epochs_to_process: args.epochs_to_process,
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

    // If both end-epoch and end-block are specified, run once and exit
    if args.end_epoch.is_some() && args.end_block.is_some() {
        tracing::info!("Running indexer once (end-epoch and end-block specified)");
        service.run().await?;
        tracing::info!("Indexer completed successfully");
        return Ok(());
    }

    // Otherwise, run in a loop
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
