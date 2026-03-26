// Copyright 2026 Boundless Foundation, Inc.
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
    let agent = Agent::new(args).await.context("[BENTO-AGENT-001] Failed to initialize Agent")?;

    sqlx::migrate!("../taskdb/migrations")
        .run(&agent.db_pool)
        .await
        .context("[BENTO-AGENT-002] Failed to run migrations")?;

    // Start metrics server in background
    tokio::spawn(async move {
        if let Err(e) = start_metrics_exporter() {
            tracing::error!("Failed to start metrics server: {}", e);
        }
    });

    // Poll until agent is signaled to exit:
    agent.poll_work().await.context("[BENTO-AGENT-003] Exiting agent polling")
}
