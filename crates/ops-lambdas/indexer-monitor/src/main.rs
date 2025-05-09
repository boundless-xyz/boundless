// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use indexer_monitor::handler::function_handler;
use lambda_runtime::{run, service_fn, tracing, Error};

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing::init_default_subscriber();

    run(service_fn(function_handler)).await
}
