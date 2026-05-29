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

use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use multi_zkvm_prover::MultiZkvmServer;
use risc0_backend::provers::{Bonsai, BonsaiConfig, ProverObj};
use url::Url;

/// MultiZKVM proving server.
///
/// Listens for RPC connections from MultiZKVM clients and forwards proof
/// requests to a Bento backend.
#[derive(Parser, Debug)]
#[command(author, version)]
struct Args {
    /// Address to bind the server to.
    #[clap(short, long, env = "BIND_ADDR", default_value = "0.0.0.0:7777")]
    bind_addr: String,

    /// Bento API URL.
    #[clap(long, env = "BENTO_API_URL", default_value = "http://localhost:8081")]
    bento_api_url: Url,

    /// Log in JSON format.
    #[clap(long, env = "LOG_JSON", default_value_t = false)]
    log_json: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    if args.log_json {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .json()
            .with_ansi(false)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    let prover: ProverObj = Arc::new(
        Bonsai::new(BonsaiConfig::default(), args.bento_api_url.as_str(), "v1:reserved:1000")
            .context("Failed to initialize Bento client")?,
    );

    let server = MultiZkvmServer::new(&args.bind_addr, prover)
        .await
        .context("Failed to bind server")?;

    tracing::info!("listening on {}", args.bind_addr);
    server.run().await;

    Ok(())
}
