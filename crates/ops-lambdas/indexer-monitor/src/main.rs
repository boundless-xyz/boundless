// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use indexer_monitor::handler::function_handler;
use lambda_runtime::{run, service_fn, tracing, Error};
use tracing_subscriber::fmt::format::FmtSpan;

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_ansi(false)
        .with_file(true)
        .with_line_number(true)
        .with_span_events(FmtSpan::CLOSE)
        .json()
        .init();

    run(service_fn(function_handler)).await
}
