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

use std::{path::PathBuf, time::Duration};

use alloy::{
    primitives::{utils::parse_ether, Address, U256},
    signers::local::PrivateKeySigner,
};
use anyhow::{bail, Result};
use boundless_slasher::{SlashService, SlashServiceConfig};
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
    #[clap(
        long,
        env = "DB_URL",
        default_value = "postgres://postgres:password@localhost:5433/postgres"
    )]
    db: String,
    /// Starting block number. Note this will only be used the first time the slasher is run.
    /// After that point, it will use the last processed block from the database.
    /// UNLESS the ignore_last_processed_block flag is set.
    #[clap(long)]
    start_block: Option<u64>,
    /// Ignore last processed block and start from the given starting block.
    #[clap(long, requires = "start_block")]
    ignore_last_processed_block: Option<bool>,
    /// Interval in seconds between checking for expired requests.
    #[clap(long, default_value = "5")]
    interval: u64,
    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "10")]
    retries: u32,
    /// Balance threshold at which to log a warning.
    #[clap(long, value_parser = parse_ether, default_value = "1")]
    warn_balance_below: Option<U256>,
    /// Balance threshold at which to log an error.
    #[clap(long, value_parser = parse_ether, default_value = "0.1")]
    error_balance_below: Option<U256>,
    /// Comma-separated list of addresses to skip when processing locked events.
    #[clap(long, value_delimiter = ',', value_parser = parse_address)]
    skip_addresses: Vec<Address>,
    /// Transaction timeout in seconds.
    #[clap(long, default_value = "120")]
    tx_timeout: u64,
    /// Maximum block range to query in a single request.
    #[clap(long, default_value = "500")]
    max_block_range: u64,
}

fn parse_address(s: &str) -> Result<Address, String> {
    s.trim().parse::<Address>().map_err(|e| format!("Failed to parse address {s}: {e}"))
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
        .json()
        .init();

    let args = MainArgs::parse();

    let slash_service = SlashService::new(
        args.rpc_url.clone(),
        &args.private_key,
        args.boundless_market_address,
        &args.db,
        SlashServiceConfig {
            interval: Duration::from_secs(args.interval),
            retries: args.retries,
            balance_warn_threshold: args.warn_balance_below,
            balance_error_threshold: args.error_balance_below,
            skip_addresses: args.skip_addresses,
            tx_timeout: Duration::from_secs(args.tx_timeout),
            max_block_range: args.max_block_range,
        },
    )
    .await?;

    if let Err(err) = slash_service.run(args.start_block, args.ignore_last_processed_block).await {
        bail!("FATAL: Error running the slasher: {err}");
    }

    Ok(())
}
