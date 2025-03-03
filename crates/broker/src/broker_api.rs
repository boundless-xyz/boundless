// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::{Context, Error as AnyhowErr, Result};
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::Router,
    routing::{get, post},
    Json,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;
use url::Url;

use crate::{
    db::{DbError, DbObj},
    task::{RetryRes, RetryTask, SupervisorErr},
    Batch, Order,
};

struct AppState {
    db: DbObj,
}

impl AppState {
    async fn new(db: DbObj) -> Result<Self> {
        Ok(Self { db })
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ErrMsg {
    #[serde(rename = "type")]
    pub(crate) ty: String,
    pub(crate) msg: String,
}

impl std::fmt::Display for ErrMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error_type: {} msg: {}", self.ty, self.msg)
    }
}

#[derive(Error, Debug)]
pub enum ApiError {
    #[error("internal error")]
    InternalErr(AnyhowErr),

    #[error("Database error")]
    DatabaseErr(#[from] DbError),
}

impl ApiError {
    fn type_str(&self) -> String {
        match self {
            Self::InternalErr(_) | Self::DatabaseErr(_) => "InternalErr",
        }
        .into()
    }
}

impl From<AnyhowErr> for ApiError {
    fn from(err: AnyhowErr) -> Self {
        Self::InternalErr(err)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = match self {
            Self::InternalErr(_) | Self::DatabaseErr(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        tracing::error!("api error, code {code}: {self:?}");
        (code, Json(ErrMsg { ty: self.type_str(), msg: self.to_string() })).into_response()
    }
}

const NEW_ORDER: &str = "/v1/orders/new";
async fn new_order(
    State(state): State<Arc<AppState>>,
    Json(new_order): Json<Order>,
) -> Result<(), ApiError> {
    let order_id = new_order.request.id;
    let order = new_order;

    state.db.add_order(order_id, order).await?;

    Ok(())
}

const GET_BATCH: &str = "/v1/batches/latest";
async fn get_latest_batch(State(state): State<Arc<AppState>>) -> Result<Json<Batch>, ApiError> {
    let latest_batch = state.db.get_current_batch().await?;
    let batch = state.db.get_batch(latest_batch).await?;

    Ok(Json(batch))
}

fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route(NEW_ORDER, post(new_order))
        .route(GET_BATCH, get(get_latest_batch))
        .with_state(state)
}

// TODO: Dedup from `api` crate
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn run(db: DbObj, bind_addr: &Url) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind_addr.to_string())
        .await
        .context("Failed to bind a TCP listener")?;

    tracing::info!("REST API listening on: {}", bind_addr);
    let app_state = Arc::new(AppState::new(db).await.context("Failed to initialize AppState")?);
    axum::serve(listener, self::app(app_state))
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("REST API service failed")?;

    Ok(())
}

pub struct BrokerApi {
    db: DbObj,
    bind_addr: Url,
}

impl BrokerApi {
    pub fn new(db: DbObj, bind_addr: Url) -> Self {
        Self { db, bind_addr }
    }
}

impl RetryTask for BrokerApi {
    fn spawn(&self) -> RetryRes {
        let db = self.db.clone();
        let bind_addr = self.bind_addr.clone();
        Box::pin(async move {
            tracing::info!("Starting Broker RPC API");
            run(db, &bind_addr).await.map_err(SupervisorErr::Fault)?;
            Ok(())
        })
    }
}
