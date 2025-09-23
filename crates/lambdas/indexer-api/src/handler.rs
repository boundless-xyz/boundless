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
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use lambda_http::Error;
use serde_json::json;
use std::{env, sync::Arc};
use tower_http::cors::{Any, CorsLayer};

use crate::db::AppState;
use crate::routes::{address, delegations, povw, staking};

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
    let cors = CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);

    // Build the router
    Router::new()
        // Health check endpoint
        .route("/health", get(health_check))
        // OpenAPI spec endpoints
        .route("/openapi.yaml", get(openapi_spec))
        .route("/openapi.json", get(openapi_json))
        // Swagger UI documentation
        .route("/docs", get(swagger_docs))
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
        // New RESTful structure
        .nest("/staking", staking::routes())
        .nest("/povw", povw::routes())
        .nest("/delegations", delegations::routes())
        // Legacy address endpoint (to be removed later)
        .nest("/rewards/address", address::routes())
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> impl IntoResponse {
    Json(json!({
        "status": "healthy",
        "service": "indexer-api"
    }))
}

/// OpenAPI specification endpoint (YAML)
async fn openapi_spec() -> impl IntoResponse {
    const OPENAPI_YAML: &str = include_str!("../openapi.yaml");
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/x-yaml")
        .body(OPENAPI_YAML.to_string())
        .unwrap()
}

/// OpenAPI specification endpoint (JSON)
async fn openapi_json() -> impl IntoResponse {
    const OPENAPI_YAML: &str = include_str!("../openapi.yaml");

    // Parse YAML and convert to JSON
    match serde_yaml::from_str::<serde_json::Value>(OPENAPI_YAML) {
        Ok(spec) => Json(spec).into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": "Failed to parse OpenAPI spec",
                "message": err.to_string()
            })),
        )
            .into_response(),
    }
}

/// Swagger UI documentation endpoint
async fn swagger_docs() -> impl IntoResponse {
    const HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Boundless Indexer API Documentation</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.10.3/swagger-ui.css">
    <style>
        body {
            margin: 0;
            padding: 0;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        }
        #swagger-ui {
            max-width: 1460px;
            margin: 0 auto;
            padding: 20px;
        }
        .topbar {
            display: none !important;
        }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.10.3/swagger-ui-bundle.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5.10.3/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {
            // Get current origin to construct the OpenAPI spec URL
            const baseUrl = window.location.origin;
            const specUrl = baseUrl + '/openapi.json';

            const ui = SwaggerUIBundle({
                url: specUrl,
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [
                    SwaggerUIBundle.presets.apis,
                    SwaggerUIStandalonePreset
                ],
                plugins: [
                    SwaggerUIBundle.plugins.DownloadUrl
                ],
                layout: "StandaloneLayout",
                validatorUrl: null,
                tryItOutEnabled: true,
                supportedSubmitMethods: ['get']
            });
            window.ui = ui;
        }
    </script>
</body>
</html>"#;

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(HTML.to_string())
        .unwrap()
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
