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

//! Integration tests for documentation and OpenAPI endpoints

use indexer_api::models::HealthResponse;
use serde_json::Value;

use super::TestEnv;

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_health_endpoint() {
    let env = TestEnv::shared().await;

    let response: HealthResponse = env.get("/health").await.unwrap();

    assert_eq!(response.status, "healthy");
    assert_eq!(response.service, "indexer-api");
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_openapi_yaml_endpoint() {
    let env = TestEnv::shared().await;

    // Get the raw YAML response
    let client = reqwest::Client::new();
    let url = format!("{}/openapi.yaml", env.api_url());
    let response = client.get(&url).send().await.unwrap();

    assert!(response.status().is_success());

    let content_type =
        response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");

    assert!(content_type.contains("yaml") || content_type.contains("x-yaml"));

    let body = response.text().await.unwrap();
    assert!(body.contains("openapi:"));
    assert!(body.contains("Boundless Indexer API"));
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_openapi_json_endpoint() {
    let env = TestEnv::shared().await;

    let response: Value = env.get("/openapi.json").await.unwrap();

    // Verify it's valid OpenAPI JSON
    assert!(response.get("openapi").is_some());
    assert!(response.get("info").is_some());
    assert!(response.get("paths").is_some());
    assert!(response.get("components").is_some());

    // Verify basic info
    let info = response.get("info").unwrap();
    assert!(info.get("title").unwrap().as_str().unwrap().contains("Boundless"));
    assert!(info.get("version").is_some());

    // Verify we have paths defined
    let paths = response.get("paths").unwrap().as_object().unwrap();
    assert!(paths.contains_key("/health"));
    assert!(paths.contains_key("/v1/povw"));
    assert!(paths.contains_key("/v1/staking"));
    assert!(paths.contains_key("/v1/delegations/votes/addresses"));
    assert!(paths.contains_key("/v1/delegations/rewards/addresses"));

    // Verify components/schemas are defined
    let components = response.get("components").unwrap();
    let schemas = components.get("schemas").unwrap().as_object().unwrap();

    // Check for important schema definitions
    // Just verify we have some schemas defined
    assert!(!schemas.is_empty(), "Should have schema definitions");
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_swagger_ui_endpoint() {
    let env = TestEnv::shared().await;

    // Get the raw HTML response
    let client = reqwest::Client::new();
    let url = format!("{}/docs", env.api_url());
    let response = client.get(&url).send().await.unwrap();

    assert!(response.status().is_success());

    let content_type =
        response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("");

    assert!(content_type.contains("text/html"));

    let body = response.text().await.unwrap();

    // Verify it's the Swagger UI HTML
    // The utoipa-swagger-ui generates HTML with these characteristic elements
    assert!(body.contains("swagger-ui"), "Response should contain swagger-ui");
    assert!(body.contains("</html>"), "Response should be valid HTML");
    // Check for either the API title or the OpenAPI endpoint reference
    assert!(
        body.contains("openapi.json") || body.contains("Swagger UI"),
        "Response should reference OpenAPI spec or contain Swagger UI"
    );
}

#[tokio::test]
#[ignore = "Requires ETH_MAINNET_RPC_URL"]
async fn test_404_handler() {
    let env = TestEnv::shared().await;

    // Try to access a non-existent endpoint
    let client = reqwest::Client::new();
    let url = format!("{}/v1/nonexistent", env.api_url());
    let response = client.get(&url).send().await.unwrap();

    assert_eq!(response.status().as_u16(), 404);

    let body: Value = response.json().await.unwrap();
    assert!(body.get("error").is_some());
    assert!(body.get("message").is_some());
}
