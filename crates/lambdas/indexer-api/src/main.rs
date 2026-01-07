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

use anyhow::Result;
use lambda_http::{run, tracing, Error};

mod db;
mod handler;
mod models;
mod openapi;
mod routes;
mod utils;

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Use Lambda's built-in subscriber (respects RUST_LOG env var)
    tracing::init_default_subscriber();

    tracing::info!("Starting indexer-api Lambda function");

    // Get the axum app
    let app = handler::create_handler().await?;

    // Run the Lambda runtime with the axum app
    run(app).await
}
