// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::{bail, Result};
use clap::Parser;
use tracing_subscriber::fmt::format::FmtSpan;

use boundless_bench::{run, MainArgs};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(false)
        .with_span_events(FmtSpan::CLOSE)
        .json()
        .init();

    match dotenvy::dotenv() {
        Ok(path) => tracing::debug!("Loaded environment variables from {:?}", path),
        Err(e) if e.not_found() => tracing::debug!("No .env file found"),
        Err(e) => bail!("failed to load .env file: {}", e),
    }

    let args = MainArgs::parse();

    run(&args).await?;

    Ok(())
}
