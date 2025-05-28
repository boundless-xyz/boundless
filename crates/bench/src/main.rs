// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::Result;
use boundless_bench::{run, MainArgs};
use clap::Parser;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = MainArgs::parse();

    run(&args).await?;

    Ok(())
}
