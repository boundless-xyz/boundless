// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::filter::EnvFilter;
use workflow::{Agent, Args};
use workflow_common::metrics::helpers::start_metrics_exporter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    let args = Args::parse();
    let task_stream = args.task_stream.clone();
    let agent = Agent::new(args).await.context("Failed to initialize Agent")?;

    sqlx::migrate!("../taskdb/migrations")
        .run(&agent.db_pool)
        .await
        .context("Failed to run migrations")?;

    // Start metrics server in background
    tokio::spawn(async move {
        if let Err(e) = start_metrics_exporter() {
            tracing::error!("Failed to start metrics server: {}", e);
        }
    });

    tracing::info!("Successful agent startup! Worker type: {task_stream}");
    agent.poll_work().await.context("Exiting agent polling")
}
