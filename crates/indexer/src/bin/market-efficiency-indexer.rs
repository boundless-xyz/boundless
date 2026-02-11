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

use anyhow::{bail, Result};
use boundless_indexer::efficiency::{MarketEfficiencyService, MarketEfficiencyServiceConfig};
use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// DB connection string.
    #[clap(long, env = "DATABASE_URL")]
    db: String,

    /// Interval in seconds between runs.
    #[clap(long, default_value = "3600")]
    interval: u64,

    /// Number of days to look back for requests.
    #[clap(long, default_value = "3")]
    lookback_days: u64,

    /// Starting timestamp (if set with end-time, runs once and exits).
    #[clap(long, requires = "end_time")]
    start_time: Option<u64>,

    /// Ending timestamp (if set with start-time, runs once and exits).
    #[clap(long, requires = "start_time")]
    end_time: Option<u64>,

    /// Number of retries before quitting after an error.
    #[clap(long, default_value = "3")]
    retries: u32,

    /// Whether to log in JSON format.
    #[clap(long, env, default_value_t = false)]
    log_json: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
        .from_env_lossy();

    if args.log_json {
        tracing_subscriber::fmt().with_ansi(false).json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_ansi(false).with_env_filter(filter).init();
    }

    let config = MarketEfficiencyServiceConfig {
        interval: Duration::from_secs(args.interval),
        lookback_days: args.lookback_days,
        start_time: args.start_time,
        end_time: args.end_time,
    };

    let mut service = MarketEfficiencyService::new(&args.db, config).await?;

    // If start-time and end-time are specified, run once and exit
    if args.start_time.is_some() && args.end_time.is_some() {
        tracing::info!(
            "Running market efficiency indexer once (start-time and end-time specified)"
        );
        service.run().await?;
        tracing::info!("Market efficiency indexer completed successfully");
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
                tracing::error!("Error running market efficiency indexer: {:?}", e);
                if failures >= args.retries {
                    bail!("Maximum retries reached");
                }
                tracing::info!("Retrying in {} seconds", args.interval);
                tokio::time::sleep(Duration::from_secs(args.interval)).await;
            }
        }
    }
}
