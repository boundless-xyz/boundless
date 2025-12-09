// Copyright 2025 Boundless Foundation, Inc.
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
    let num_agents = args.agents;

    // Start metrics server in background (only once, not per agent)
    tokio::spawn(async move {
        if let Err(e) = start_metrics_exporter() {
            tracing::error!("Failed to start metrics server: {}", e);
        }
    });

    // Run migrations once before spawning agents
    // We'll create a temporary agent just for migrations, then create the actual agents
    let temp_agent = Agent::new(args.clone())
        .await
        .context("[BENTO-AGENT-001] Failed to initialize Agent for migrations")?;

    sqlx::migrate!("../taskdb/migrations")
        .run(&temp_agent.db_pool)
        .await
        .context("[BENTO-AGENT-002] Failed to run migrations")?;

    // Drop the temporary agent to free resources
    drop(temp_agent);

    tracing::info!("Spawning {} agent instance(s)", num_agents);

    // Use LocalSet to spawn agents that contain non-Send types (Rc)
    let local = tokio::task::LocalSet::new();

    local.run_until(async move {
        // Spawn N agents, each running independently
        let mut handles = Vec::new();
        for agent_id in 1..=num_agents {
            let args_clone = args.clone();
            let handle = tokio::task::spawn_local(async move {
                let agent = match Agent::new(args_clone).await {
                    Ok(agent) => agent,
                    Err(e) => {
                        tracing::error!(
                            "[BENTO-AGENT-001] Agent {} failed to initialize: {:#}",
                            agent_id,
                            e
                        );
                        return Err(e);
                    }
                };

                tracing::info!("Agent {} started", agent_id);
                agent
                    .poll_work()
                    .await
                    .context(format!("[BENTO-AGENT-003] Agent {} exiting", agent_id))
            });
            handles.push(handle);
        }

        // Wait for all agents to complete (or fail)
        let mut has_error = false;
        for (idx, handle) in handles.into_iter().enumerate() {
            match handle.await {
                Ok(Ok(())) => {
                    tracing::info!("Agent {} completed successfully", idx + 1);
                }
                Ok(Err(e)) => {
                    tracing::error!("Agent {} exited with error: {:#}", idx + 1, e);
                    has_error = true;
                }
                Err(e) => {
                    tracing::error!("Agent {} task panicked: {:#}", idx + 1, e);
                    has_error = true;
                }
            }
        }

        // If any agent failed, return an error
        if has_error {
            return Err(anyhow::anyhow!(
                "One or more agents failed. Check logs for details."
            ));
        }

        Ok(())
    })
    .await
}
