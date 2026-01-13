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
use axum::{
    http::{header, HeaderValue, StatusCode, Uri},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use lambda_http::Error;
use serde_json::json;
use std::{env, sync::Arc};
use tower_http::{
    cors::{Any, CorsLayer},
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

use crate::db::AppState;
use crate::openapi::ApiDoc;
use crate::routes::{delegations, market, povw, staking};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

/// Creates the Lambda handler with axum router
pub async fn create_handler() -> Result<Router, Error> {
    // Load configuration from environment
    let db_url = env::var("DB_URL").context("DB_URL environment variable is required")?;
    let chain_id_str = env::var("CHAIN_ID").context("CHAIN_ID environment variable is required")?;
    let chain_id = chain_id_str.parse::<u64>().context("CHAIN_ID must be a valid u64")?;

    // Create application state with database connection
    let state = AppState::new(&db_url, chain_id).await?;
    let shared_state = Arc::new(state);

    // Create the axum application with routes
    Ok(create_app(shared_state))
}

/// Creates the axum application with all routes
pub fn create_app(state: Arc<AppState>) -> Router {
    // Configure CORS
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    // Configure request/response tracing
    let trace_layer = TraceLayer::new_for_http()
        .make_span_with(DefaultMakeSpan::new().level(Level::DEBUG))
        .on_response(DefaultOnResponse::new().level(Level::DEBUG));

    // Build the router
    Router::new()
        // Health check endpoint
        .route("/health", get(health_check))
        // OpenAPI spec endpoint (YAML format)
        .route("/openapi.yaml", get(openapi_yaml))
        // Swagger UI documentation with generated spec (includes /openapi.json automatically)
        .merge(SwaggerUi::new("/docs").url("/openapi.json", ApiDoc::openapi()))
        // API v1 routes
        .nest("/v1", api_v1_routes(state))
        // Add CORS layer
        .layer(cors)
        // Add request/response tracing layer
        .layer(trace_layer)
        // Add fallback for unmatched routes
        .fallback(not_found)
}

/// API v1 routes
fn api_v1_routes(state: Arc<AppState>) -> Router {
    Router::new()
        // RESTful structure
        .nest("/staking", staking::routes())
        .nest("/povw", povw::routes())
        .nest("/delegations", delegations::routes())
        .nest("/market", market::routes())
        .with_state(state)
}

/// Health check endpoint
#[utoipa::path(
    get,
    path = "/health",
    tag = "Health",
    responses(
        (status = 200, description = "Service is healthy", body = serde_json::Value)
    )
)]
async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "service": "indexer-api"
    }))
}

/// OpenAPI specification endpoint (YAML)
async fn openapi_yaml() -> impl IntoResponse {
    // Convert the generated JSON spec to YAML
    let openapi_json = ApiDoc::openapi();
    match serde_yaml::to_string(&openapi_json) {
        Ok(yaml) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/x-yaml")
            .body(yaml)
            .unwrap(),
        Err(err) => Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(format!("Failed to convert to YAML: {}", err))
            .unwrap(),
    }
}

/// 404 handler
async fn not_found(uri: Uri) -> impl IntoResponse {
    tracing::warn!(path = %uri.path(), "Endpoint not found");
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
    // Log the full error with backtrace for debugging
    tracing::error!(error = ?err, "Request failed");

    // Convert error message to lowercase for case-insensitive matching
    let error_message = err.to_string().to_lowercase();

    // Determine appropriate status code based on error type
    let (status, message) = if error_message.contains("database")
        || error_message.contains("connection")
        || error_message.contains("pool")
    {
        (StatusCode::SERVICE_UNAVAILABLE, "Database connection error. Please try again later.")
    } else if error_message.starts_with("no data available")
        || error_message.starts_with("no povw")
        || error_message.contains("does not exist")
    {
        (StatusCode::NOT_FOUND, "The requested data was not found.")
    } else {
        // For production, return a generic message. In dev, you might want to return the actual error
        (StatusCode::INTERNAL_SERVER_ERROR, "An internal error occurred. Please try again later.")
    };

    (
        status,
        Json(json!({
            "error": status.canonical_reason().unwrap_or("Error"),
            "message": message
        })),
    )
}

/// Create a cache control header value safely
pub fn cache_control(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or_else(|_| HeaderValue::from_static("public, max-age=60"))
}
