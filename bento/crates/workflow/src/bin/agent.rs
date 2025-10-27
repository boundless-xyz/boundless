// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
use clap::Parser;
use tracing_subscriber::filter::EnvFilter;
use workflow::metrics_server;
use workflow::{Agent, Args};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().with_env_filter(EnvFilter::from_default_env()).init();

    // Initialize and register all metrics with the default Prometheus registry
    workflow_common::metrics::helpers::init();

    let args = Args::parse();
    let task_stream = args.task_stream.clone();
    let agent = Agent::new(args).await.context("Failed to initialize Agent")?;

    sqlx::migrate!("../taskdb/migrations")
        .run(&agent.db_pool)
        .await
        .context("Failed to run migrations")?;

    tracing::info!("Successful agent startup! Worker type: {task_stream}");

    // Start metrics server in background
    let metrics_port = std::env::var("METRICS_PORT")
        .unwrap_or_else(|_| "9090".to_string())
        .parse::<u16>()
        .unwrap_or(9090);

    tokio::spawn(async move {
        if let Err(e) = metrics_server::start_metrics_server(metrics_port).await {
            tracing::error!("Metrics server error: {}", e);
        }
    });

    // Poll until agent is signaled to exit:
    agent.poll_work().await.context("Exiting agent polling")
}
