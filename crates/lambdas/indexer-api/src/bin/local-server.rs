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

use anyhow::{Context, Result};
use std::{env, net::SocketAddr, sync::Arc};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

use indexer_api::db::AppState;
use indexer_api::handler::create_app;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // Load configuration from environment or use defaults
    let db_url = env::var("DB_URL")
        .or_else(|_| env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://postgres:password@localhost:5433/postgres".to_string());

    let chain_id = env::var("CHAIN_ID").ok().and_then(|c| c.parse::<u64>().ok()).unwrap_or(1); // Default to mainnet chain ID

    let port = env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(3000);

    tracing::info!("Starting local indexer-api server");
    tracing::info!("Database URL: {}", db_url);
    tracing::info!("Chain ID: {}", chain_id);
    tracing::info!("Port: {}", port);

    // Create application state with database connection
    let state =
        AppState::new(&db_url, chain_id).await.context("Failed to create application state")?;
    let shared_state = Arc::new(state);

    // Create the axum application with routes
    let app = create_app(shared_state);

    // Create the server address
    let addr = SocketAddr::from(([127, 0, 0, 1], port));

    tracing::info!("Server listening on http://{}", addr);

    // Create the listener
    let listener =
        tokio::net::TcpListener::bind(addr).await.context("Failed to bind to address")?;

    // Run the server
    axum::serve(listener, app).await.context("Server failed")?;

    Ok(())
}
