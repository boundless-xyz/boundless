// Copyright 2025 RISC Zero, Inc.
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
use axum::{
    http::{StatusCode},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use lambda_http::Error;
use serde_json::json;
use std::{env, sync::Arc};
use tower_http::cors::{Any, CorsLayer};

use crate::db::AppState;
use crate::routes::povw;

/// Creates the Lambda handler with axum router
pub async fn create_handler() -> Result<Router, Error> {
    // Load configuration from environment
    let db_url = env::var("DB_URL").context("DB_URL environment variable is required")?;

    // Create application state with database connection
    let state = AppState::new(&db_url).await?;
    let shared_state = Arc::new(state);

    // Create the axum application with routes
    Ok(create_app(shared_state))
}

/// Creates the axum application with all routes
pub fn create_app(state: Arc<AppState>) -> Router {
    // Configure CORS
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    // Build the router
    Router::new()
        // Health check endpoint
        .route("/health", get(health_check))
        // API v1 routes
        .nest("/v1", api_v1_routes(state))
        // Add CORS layer
        .layer(cors)
        // Add fallback for unmatched routes
        .fallback(not_found)
}

/// API v1 routes
fn api_v1_routes(state: Arc<AppState>) -> Router {
    Router::new()
        .nest("/rewards/povw", povw::routes())
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "service": "indexer-api"
    }))
}

/// 404 handler
async fn not_found() -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        Json(json!({
            "error": "Not Found",
            "message": "The requested endpoint does not exist"
        })),
    )
}

/// Global error handler that converts anyhow errors to HTTP responses
pub fn handle_error(err: anyhow::Error) -> impl IntoResponse {
    tracing::error!("Request failed: {:?}", err);

    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({
            "error": "Internal Server Error",
            "message": err.to_string()
        })),
    )
}