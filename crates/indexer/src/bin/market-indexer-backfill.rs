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

use alloy::{primitives::Address, signers::local::PrivateKeySigner};
use anyhow::{bail, Result};
use boundless_indexer::db::IndexerDb;
use boundless_indexer::market::backfill::{BackfillMode, BackfillService};
use boundless_indexer::market::service::TransactionFetchStrategy;
use boundless_indexer::market::{IndexerService, IndexerServiceConfig};
use clap::Parser;
use url::Url;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Backfill mode: "statuses_and_aggregates" or "aggregates"
    #[clap(long, env)]
    mode: String,

    /// URL of the Ethereum RPC endpoint
    #[clap(short, long, env)]
    rpc_url: Url,

    /// Optional URL of the Ethereum RPC endpoint for get_logs calls
    #[clap(long, env)]
    logs_rpc_url: Option<Url>,

    /// Address of the BoundlessMarket contract
    #[clap(short, long, env)]
    boundless_market_address: Address,

    /// DB connection string
    #[clap(long, env = "DATABASE_URL")]
    db: String,

    /// Start block number (backfill from this block)
    #[clap(long)]
    start_block: u64,

    /// End block number (backfill up to this block, default: latest indexed block)
    #[clap(long)]
    end_block: Option<u64>,

    /// Whether to log in JSON format
    #[clap(long, env, default_value_t = false)]
    log_json: bool,

    /// Optional cache storage URI (e.g., file:///path/to/cache or s3://bucket-name)
    #[clap(long, env)]
    cache_uri: Option<String>,

    /// Transaction fetching strategy: "block-receipts" (default) or "tx-by-hash"
    #[clap(long, env, default_value = "block-receipts")]
    tx_fetch_strategy: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

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

    // Parse mode
    let mode = match args.mode.as_str() {
        "statuses_and_aggregates" => BackfillMode::StatusesAndAggregates,
        "aggregates" => BackfillMode::Aggregates,
        _ => bail!("Invalid mode: {}. Use 'statuses_and_aggregates' or 'aggregates'", args.mode),
    };

    let tx_fetch_strategy = match args.tx_fetch_strategy.as_str() {
        "block-receipts" => TransactionFetchStrategy::BlockReceipts,
        "tx-by-hash" => TransactionFetchStrategy::TransactionByHash,
        _ => bail!(
            "Invalid tx_fetch_strategy: {}. Use 'block-receipts' or 'tx-by-hash'",
            args.tx_fetch_strategy
        ),
    };

    let config = IndexerServiceConfig {
        interval: Duration::from_secs(3),
        aggregation_interval: Duration::from_secs(2),
        retries: 10,
        batch_size: 500,
        cache_uri: args.cache_uri.clone(),
        tx_fetch_strategy,
        execution_config: None,
    };

    let logs_rpc_url = args.logs_rpc_url.clone().unwrap_or_else(|| args.rpc_url.clone());

    tracing::info!("Initializing market indexer for backfill");
    let indexer_service = IndexerService::new(
        args.rpc_url.clone(),
        logs_rpc_url.clone(),
        &PrivateKeySigner::random(),
        args.boundless_market_address,
        &args.db,
        config,
    )
    .await?;

    // Determine end block
    let end_block = if let Some(end) = args.end_block {
        end
    } else {
        // Get latest indexed block from database
        match indexer_service.db.get_last_block().await? {
            Some(block) => {
                tracing::info!("Using latest indexed block: {}", block);
                block
            }
            None => {
                bail!("No blocks have been indexed yet. Please specify --end-block explicitly.");
            }
        }
    };

    tracing::info!(
        "Starting backfill with mode: {:?}, start_block: {}, end_block: {}",
        mode,
        args.start_block,
        end_block
    );

    let mut backfill_service =
        BackfillService::new(indexer_service, mode, args.start_block, end_block);

    if let Err(err) = backfill_service.run().await {
        bail!("FATAL: Error running backfill: {err}");
    }

    tracing::info!("Backfill completed successfully");
    Ok(())
}
