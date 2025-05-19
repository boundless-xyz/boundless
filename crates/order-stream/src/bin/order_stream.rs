// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::Result;
use clap::Parser;
use order_stream::Args;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let result = order_stream::run(&args).await;
    if let Err(e) = result {
        tracing::error!("FATAL: {:?}", e);
    }

    Ok(())
}
