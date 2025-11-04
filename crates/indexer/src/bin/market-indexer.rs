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

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Result};
use boundless_indexer::market::{IndexerService, IndexerServiceConfig};
use boundless_indexer::market::service::TransactionFetchStrategy;
use clap::Parser;
use url::Url;

/// Arguments of the indexer.
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct MainArgs {
    /// URL of the Ethereum RPC endpoint.
    #[clap(short, long, env)]
    rpc_url: Url,
    /// Optional URL of the Ethereum RPC endpoint for get_logs calls.
    /// If not provided, uses the main rpc_url for all operations.
    #[clap(long, env)]
    logs_rpc_url: Option<Url>,
    /// Address of the BoundlessMarket contract.
    #[clap(short, long, env)]
    boundless_market_address: Address,
    /// DB connection string.
    #[clap(long, env = "DATABASE_URL")]
    db: String,
    /// Starting block number.
    #[clap(long)]
    start_block: Option<u64>,
    /// Ending block number (if set, indexer will process up to this block and exit).
    #[clap(long)]
    end_block: Option<u64>,
    /// Interval in seconds between checking for new events.
    #[clap(long, default_value = "3")]
    interval: u64,
    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "10")]
    retries: u32,
    /// Number of blocks to process in each batch.
    #[clap(long, default_value = "500")]
    batch_size: u64,
    /// Whether to log in JSON format.
    #[clap(long, env, default_value_t = false)]
    log_json: bool,
    /// Optional URL of the order stream API for off-chain order indexing.
    #[clap(long, env)]
    order_stream_url: Option<Url>,
    /// Optional cache storage URI (e.g., file:///path/to/cache or s3://bucket-name).
    #[clap(long, env)]
    cache_uri: Option<String>,
    /// Transaction fetching strategy: "block-receipts" (default, more efficient) or "tx-by-hash" (fallback).
    #[clap(long, env, default_value = "block-receipts")]
    tx_fetch_strategy: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = MainArgs::parse();

    if args.log_json {
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .json()
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    let tx_fetch_strategy = match args.tx_fetch_strategy.as_str() {
        "block-receipts" => TransactionFetchStrategy::BlockReceipts,
        "tx-by-hash" => TransactionFetchStrategy::TransactionByHash,
        _ => bail!(
            "Invalid tx_fetch_strategy: {}. Use 'block-receipts' or 'tx-by-hash'",
            args.tx_fetch_strategy
        ),
    };

    let config = IndexerServiceConfig {
        interval: Duration::from_secs(args.interval),
        retries: args.retries,
        batch_size: args.batch_size,
        cache_uri: args.cache_uri.clone(),
        tx_fetch_strategy,
    };

    let logs_rpc_url = args.logs_rpc_url.clone().unwrap_or_else(|| args.rpc_url.clone());

    let mut indexer_service = if let Some(order_stream_url) = args.order_stream_url {
        tracing::info!("Initializing market indexer with order stream at: {}", order_stream_url);
        IndexerService::new_with_order_stream(
            args.rpc_url.clone(),
            logs_rpc_url.clone(),
            &PrivateKeySigner::random(),
            args.boundless_market_address,
            &args.db,
            config,
            order_stream_url,
        )
        .await?
    } else {
        tracing::info!("Initializing market indexer without order stream");
        IndexerService::new(
            args.rpc_url.clone(),
            logs_rpc_url,
            &PrivateKeySigner::random(),
            args.boundless_market_address,
            &args.db,
            config,
        )
        .await?
    };

    // If end_block is specified, run once and exit
    if args.end_block.is_some() {
        tracing::info!("Running indexer once (end-block specified)");
        if let Err(err) = indexer_service.run(args.start_block, args.end_block).await {
            bail!("FATAL: Error running the indexer: {err}");
        }
        tracing::info!("Indexer completed successfully");
        return Ok(());
    }

    // Otherwise, run in the normal loop (existing behavior)
    if let Err(err) = indexer_service.run(args.start_block, None).await {
        bail!("FATAL: Error running the indexer: {err}");
    }

    Ok(())
}
