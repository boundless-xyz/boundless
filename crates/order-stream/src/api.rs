// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::Address;
use anyhow::Context;
use axum::extract::{Json, Path, Query, State};
use boundless_market::order_stream_client::{
    ErrMsg, Nonce, OrderData, SubmitOrderRes, HEALTH_CHECK, ORDER_LIST_PATH, ORDER_SUBMISSION_PATH,
};
use serde::Deserialize;
use std::sync::Arc;
use utoipa::IntoParams;

use crate::{
    order_db::{DbOrder, OrderDbErr},
    AppError, AppState, Order,
};

#[utoipa::path(
    post,
    path = ORDER_SUBMISSION_PATH,
    request_body = Order,
    responses(
        (status = 200, description = "Order submission response", body = SubmitOrderRes),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Submit a new order to the market order-stream
pub(crate) async fn submit_order(
    State(state): State<Arc<AppState>>,
    Json(order): Json<Order>,
) -> Result<Json<SubmitOrderRes>, AppError> {
    // Validate the order
    order.validate(state.config.market_address, state.chain_id)?;
    let order_req_id = order.request.id;
    let order_id = state.db.add_order(order).await.context("failed to add order to db")?;

    tracing::debug!("Order 0x{order_req_id:x} - [{order_id}] submitted",);
    Ok(Json(SubmitOrderRes { status: "success".into(), request_id: order_req_id }))
}

const MAX_ORDERS: u64 = 1000;

/// Paging query parameters
#[derive(Deserialize, IntoParams)]
pub struct Pagination {
    /// order id offset to start at
    offset: u64,
    /// Limit of orders returned, max 1000
    limit: u64,
}

#[utoipa::path(
    get,
    path = ORDER_LIST_PATH,
    params(
        Pagination,
    ),
    responses(
        (status = 200, description = "list of orders", body = Vec<OrderData>),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Returns a list of orders, with optional paging.
pub(crate) async fn list_orders(
    State(state): State<Arc<AppState>>,
    paging: Query<Pagination>,
) -> Result<Json<Vec<DbOrder>>, AppError> {
    let limit = if paging.limit > MAX_ORDERS { MAX_ORDERS } else { paging.limit };
    // i64::try_from converts to non-zero u64
    let limit = i64::try_from(limit).map_err(|_| AppError::QueryParamErr("limit"))?;
    let offset = i64::try_from(paging.offset).map_err(|_| AppError::QueryParamErr("index"))?;

    let results = state.db.list_orders(offset, limit).await.context("Failed to query DB")?;
    Ok(Json(results))
}

#[utoipa::path(
    get,
    path = "api/orders/<request_id>",
    params(
        ("id" = String, Path, description = "Request ID")
    ),
    responses(
        (status = 200, description = "list of orders", body = Vec<OrderData>),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Returns all the orders with the given request_id.
pub(crate) async fn find_orders_by_request_id(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> Result<Json<Vec<DbOrder>>, AppError> {
    let results =
        state.db.find_orders_by_request_id(request_id).await.context("Failed to query DB")?;
    Ok(Json(results))
}

#[utoipa::path(
    get,
    path = "/api/nonce/<addr>",
    params(
        Pagination,
    ),
    params(
        ("id" = String, Path, description = "Ethereum address")
    ),
    responses(
        (status = 200, description = "nonce", body = Nonce),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Returns the brokers current nonce by address
pub(crate) async fn get_nonce(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<Address>,
) -> Result<Json<Nonce>, AppError> {
    let res = state.db.get_nonce(addr).await;

    let nonce = match res {
        Ok(nonce) => nonce,
        Err(OrderDbErr::AddrNotFound(addr)) => {
            state.db.add_broker(addr).await.context("Failed to add new broker")?
        }
        Err(err) => {
            return Err(AppError::InternalErr(err.into()));
        }
    };

    Ok(Json(Nonce { nonce }))
}

#[utoipa::path(
    get,
    path = HEALTH_CHECK,
    responses(
        (status = 200, description = "Healthy"),
        (status = 500, description = "Unhealthy", body = ErrMsg)
    )
)]
/// Submit a new order to the market order-stream
pub(crate) async fn health(State(state): State<Arc<AppState>>) -> Result<(), AppError> {
    state.db.health_check().await.context("Failed health check")?;
    Ok(())
}
