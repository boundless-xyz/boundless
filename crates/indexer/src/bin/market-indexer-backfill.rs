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

use alloy::{
    primitives::Address,
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_indexer::db::IndexerDb;
use boundless_indexer::market::backfill::{BackfillMode, BackfillService};
use boundless_indexer::market::epoch_calculator::DEFAULT_EPOCH0_START_TIME;
use boundless_indexer::market::service::TransactionFetchStrategy;
use boundless_indexer::market::{IndexerService, IndexerServiceConfig};
use clap::Parser;
use url::Url;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Backfill mode: "statuses_and_aggregates", "aggregates", or "chain_data"
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

    /// Start block number (backfill from this block). Either this or --lookback-blocks is required.
    #[clap(long)]
    start_block: Option<u64>,

    /// Number of blocks to look back from the current block. Alternative to --start-block.
    /// When specified, fetches current block from RPC and calculates start_block = current - lookback.
    #[clap(long)]
    lookback_blocks: Option<u64>,

    /// End block number. Defaults to the last block processed by the main indexer.
    /// The backfill will never process beyond what the main indexer has already indexed.
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

    /// Delay in milliseconds between batches during chain data backfill (default: 1000)
    #[clap(long, env, default_value_t = 1000)]
    chain_data_batch_delay_ms: u64,

    /// Number of blocks to process in each batch (default: 750)
    #[clap(long, env, default_value_t = 750)]
    batch_size: u64,

    /// Unix timestamp when epoch 0 started for epoch-based aggregations.
    /// Uses ZKC contract epoch logic: epoch = (timestamp - epoch0_start_time) / EPOCH_DURATION
    /// where EPOCH_DURATION = 2 days (172800 seconds).
    #[clap(long, env, default_value_t = DEFAULT_EPOCH0_START_TIME)]
    epoch0_start_time: u64,
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
        "chain_data" => BackfillMode::ChainData,
        _ => bail!(
            "Invalid mode: {}. Use 'statuses_and_aggregates', 'aggregates', or 'chain_data'",
            args.mode
        ),
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
        retries: 3,
        batch_size: args.batch_size,
        cache_uri: args.cache_uri.clone(),
        tx_fetch_strategy,
        execution_config: None,
        block_delay: 0,
        epoch0_start_time: args.epoch0_start_time,
    };

    tracing::info!("Epoch aggregations enabled with epoch0_start_time: {}", args.epoch0_start_time);

    let logs_rpc_url = args.logs_rpc_url.clone().unwrap_or_else(|| args.rpc_url.clone());

    // Determine start and end blocks
    let (start_block, end_block) = match (args.start_block, args.lookback_blocks) {
        (Some(start), None) => {
            // Explicit start block provided
            (start, args.end_block)
        }
        (None, Some(lookback)) => {
            // Lookback mode: fetch current block from RPC
            tracing::info!("Fetching current block for lookback calculation...");
            let provider = ProviderBuilder::new().connect_http(args.rpc_url.clone());
            let current_block = provider.get_block_number().await?;
            let start = current_block.saturating_sub(lookback);
            let end = args.end_block.unwrap_or(current_block);
            tracing::info!(
                "Lookback mode: current_block={}, lookback={}, start_block={}, end_block={}",
                current_block,
                lookback,
                start,
                end
            );
            (start, Some(end))
        }
        (Some(_), Some(_)) => {
            bail!("Cannot specify both --start-block and --lookback-blocks");
        }
        (None, None) => {
            bail!("Either --start-block or --lookback-blocks must be specified");
        }
    };

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

    // Determine final end block (defaults to last block processed by main indexer)
    let end_block = if let Some(end) = end_block {
        end
    } else {
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
        start_block,
        end_block
    );

    let mut backfill_service = BackfillService::new(
        indexer_service,
        mode,
        start_block,
        end_block,
        args.chain_data_batch_delay_ms,
    );

    if let Err(err) = backfill_service.run().await {
        bail!("FATAL: Error running backfill: {err}");
    }

    tracing::info!("Backfill completed successfully");
    Ok(())
}
