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
use boundless_indexer::market::service::{IndexerServiceExecutionConfig, TransactionFetchStrategy};
use boundless_indexer::market::{IndexerService, IndexerServiceConfig};
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
    /// Interval in seconds between running aggregation tasks.
    #[clap(long, default_value = "10")]
    aggregation_interval: u64,
    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "10")]
    retries: u32,
    /// Number of blocks to process in each batch.
    #[clap(long, default_value = "9999")]
    batch_size: u64,
    /// Whether to log in JSON format.
    #[clap(long, env, default_value_t = false)]
    log_json: bool,
    /// Optional URL of the order stream API for off-chain order indexing.
    #[clap(long, env)]
    order_stream_url: Option<Url>,
    /// Optional API key for authenticating with the order stream API.
    #[clap(long, env)]
    order_stream_api_key: Option<String>,
    /// Optional cache storage URI (e.g., file:///path/to/cache or s3://bucket-name).
    #[clap(long, env)]
    cache_uri: Option<String>,
    /// Transaction fetching strategy: "block-receipts" or "tx-by-hash".
    /// I.e. should we use eth_getBlockReceipts or eth_getTransactionByHash to fetch transaction metadata
    /// Depending on RPC provider, one may be more efficient than the other.
    #[clap(long, env, default_value = "tx-by-hash")]
    tx_fetch_strategy: String,
    /// Interval in seconds between checking for requests with pending cycle counts, which will need
    /// to be scheduled for execution.
    #[clap(long, default_value = "3")]
    execution_interval: u64,
    /// Bento API execution configuration. All fields in this group require bento_api_url to be set.
    #[clap(flatten)]
    bento_config: BentoExecutionConfig,
}

/// Bento API execution configuration.
#[derive(Parser, Debug, Clone)]
struct BentoExecutionConfig {
    /// URL to the Bento API (required if any other bento option is provided)
    #[clap(long, env, requires = "bento_api_key")]
    bento_api_url: Option<String>,
    /// An API key to use for Bento API operations (required if bento_api_url is provided)
    #[clap(long, env, requires = "bento_api_url")]
    bento_api_key: Option<String>,
    /// Number of times bento API operations will be retried in case of errors
    #[clap(long, default_value = "3", requires = "bento_api_url")]
    bento_retry_count: u64,
    /// Wait interval between retries of bento API operations in case of errors, in milliseconds
    #[clap(long, default_value = "1000", requires = "bento_api_url")]
    bento_retry_sleep_ms: u64,
    /// Max number of requests submitted for execution to populate cycle counts
    #[clap(long, default_value = "20", requires = "bento_api_url")]
    max_concurrent_executing: u32,
    /// Max number of executing requests queried for status on each iteration of the execution task
    #[clap(long, default_value = "30", requires = "bento_api_url")]
    max_status_queries: u32,
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

    // Bento execution config is enabled if bento_api_url and bento_api_key are provided
    let execution_config = match (args.bento_config.bento_api_url, args.bento_config.bento_api_key)
    {
        (Some(bento_api_url), Some(bento_api_key)) => Some(IndexerServiceExecutionConfig {
            execution_interval: Duration::from_secs(args.execution_interval),
            bento_api_url,
            bento_api_key,
            bento_retry_count: args.bento_config.bento_retry_count,
            bento_retry_sleep_ms: args.bento_config.bento_retry_sleep_ms,
            max_concurrent_executing: args.bento_config.max_concurrent_executing,
            max_status_queries: args.bento_config.max_status_queries,
            max_iterations: 0,
        }),
        _ => None,
    };

    let config = IndexerServiceConfig {
        interval: Duration::from_secs(args.interval),
        aggregation_interval: Duration::from_secs(args.aggregation_interval),
        retries: args.retries,
        batch_size: args.batch_size,
        cache_uri: args.cache_uri.clone(),
        tx_fetch_strategy,
        execution_config,
    };

    let logs_rpc_url = args.logs_rpc_url.clone().unwrap_or_else(|| args.rpc_url.clone());

    let mut indexer_service = if let Some(order_stream_url) = args.order_stream_url {
        if args.order_stream_api_key.is_some() {
            tracing::info!(
                "Initializing market indexer with order stream at: {} (with API key)",
                order_stream_url
            );
        } else {
            tracing::info!(
                "Initializing market indexer with order stream at: {}",
                order_stream_url
            );
        }
        IndexerService::new_with_order_stream(
            args.rpc_url.clone(),
            logs_rpc_url.clone(),
            &PrivateKeySigner::random(),
            args.boundless_market_address,
            &args.db,
            config,
            order_stream_url,
            args.order_stream_api_key,
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
