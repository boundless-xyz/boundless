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

use std::{net::SocketAddr, sync::Arc};

use anyhow::Context;
use boundless_executor::{api, backends, Storage};
use clap::Parser;
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};
use tracing_subscriber::{fmt, EnvFilter};

#[derive(Parser, Debug)]
#[command(name = "boundless-executor", version, about)]
struct Args {
    /// Address to bind the HTTP server to.
    #[arg(long, env = "EXECUTOR_BIND", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,

    /// Public URL the service is reachable at. Used to construct presigned
    /// upload URLs handed back to clients. Defaults to `http://{bind}`.
    #[arg(long, env = "EXECUTOR_PUBLIC_URL")]
    public_url: Option<String>,

    /// Maximum request body size, in bytes (applies to ELF/input/receipt
    /// uploads). Defaults to 256 MiB.
    #[arg(long, env = "EXECUTOR_BODY_LIMIT", default_value_t = 256 * 1024 * 1024)]
    body_limit: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();

    let registry = Arc::new(backends::default_registry());
    let enabled = registry.kinds();
    tracing::info!(?enabled, bind = %args.bind, "starting boundless-executor");
    if enabled.is_empty() {
        tracing::warn!("no zkvm backends enabled; rebuild with --features risc0");
    }

    let public_url = args.public_url.clone().unwrap_or_else(|| format!("http://{}", args.bind));
    tracing::info!(public_url = %public_url, "advertising presigned base URL");

    let state = api::AppState { registry, storage: Storage::new(), public_url };
    let app = api::router(state)
        .layer(RequestBodyLimitLayer::new(args.body_limit))
        .layer(TraceLayer::new_for_http());

    let listener = tokio::net::TcpListener::bind(args.bind)
        .await
        .with_context(|| format!("binding {}", args.bind))?;

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server error")?;

    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };

    #[cfg(unix)]
    let terminate = async {
        use tokio::signal::unix::{signal, SignalKind};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
