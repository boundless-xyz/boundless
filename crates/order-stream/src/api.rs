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

use alloy::primitives::Address;
use anyhow::Context;
use axum::extract::{Json, Path, Query, State};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use boundless_market::order_stream_client::{
    ErrMsg, Nonce, OrderData, SubmitOrderRes, AUTH_GET_NONCE, HEALTH_CHECK, ORDER_LIST_PATH,
    ORDER_LIST_PATH_V2, ORDER_SUBMISSION_PATH,
};
use serde::{Deserialize, Serialize};
use sqlx::types::chrono::{self, DateTime, Utc};
use std::sync::Arc;
use utoipa::IntoParams;

use crate::{
    order_db::{DbOrder, OrderDbErr, SortDirection},
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
    /// order id offset to start at (used when sort is not specified or sort=asc)
    #[serde(default)]
    offset: Option<u64>,
    /// Limit of orders returned, max 1000
    limit: u64,
    /// Sort order: "desc" for descending by creation time, otherwise ascending by id (default)
    #[serde(default)]
    sort: Option<String>,
    /// ISO 8601 timestamp to fetch orders created after this time (only used with sort=desc)
    #[serde(default)]
    after: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CursorData {
    timestamp: DateTime<Utc>,
    id: i64,
}

fn encode_cursor(timestamp: DateTime<Utc>, id: i64) -> Result<String, AppError> {
    let cursor_data = CursorData { timestamp, id };
    let json = serde_json::to_string(&cursor_data)
        .map_err(|e| AppError::InternalErr(anyhow::anyhow!("Failed to serialize cursor: {}", e)))?;
    Ok(BASE64.encode(json))
}

fn decode_cursor(cursor_str: &str) -> Result<(DateTime<Utc>, i64), AppError> {
    let json =
        BASE64.decode(cursor_str).map_err(|_| AppError::QueryParamErr("cursor: invalid base64"))?;
    let cursor_data: CursorData = serde_json::from_slice(&json)
        .map_err(|_| AppError::QueryParamErr("cursor: invalid format"))?;
    Ok((cursor_data.timestamp, cursor_data.id))
}

/// Paging query parameters for v2 API
#[derive(Deserialize, IntoParams)]
pub struct PaginationV2 {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,
    /// Limit of orders returned, max 1000 (default 100)
    #[serde(default)]
    limit: Option<u64>,
    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    sort: Option<String>,
    /// ISO 8601 timestamp to fetch orders created before this time
    #[serde(default)]
    #[param(value_type = Option<String>)]
    before: Option<DateTime<Utc>>,
    /// ISO 8601 timestamp to fetch orders created after this time
    #[serde(default)]
    #[param(value_type = Option<String>)]
    after: Option<DateTime<Utc>>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ListOrdersV2Response {
    pub orders: Vec<DbOrder>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
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
/// Returns a list of orders with optional paging and sorting.
///
/// By default, orders are sorted by id ascending. Use sort=desc to sort by creation time descending.
pub(crate) async fn list_orders(
    State(state): State<Arc<AppState>>,
    paging: Query<Pagination>,
) -> Result<Json<Vec<DbOrder>>, AppError> {
    let limit = if paging.limit > MAX_ORDERS { MAX_ORDERS } else { paging.limit };
    let limit = i64::try_from(limit).map_err(|_| AppError::QueryParamErr("limit"))?;

    let results = if paging.sort.as_deref() == Some("desc") {
        let after_timestamp = if let Some(after_str) = &paging.after {
            let ts = after_str
                .parse::<chrono::DateTime<chrono::Utc>>()
                .map_err(|_| AppError::QueryParamErr("after"))?;
            Some(ts)
        } else {
            None
        };

        state
            .db
            .list_orders_by_creation_desc(after_timestamp, limit)
            .await
            .context("Failed to query DB")?
    } else {
        let offset = i64::try_from(paging.offset.unwrap_or(0))
            .map_err(|_| AppError::QueryParamErr("offset"))?;
        state.db.list_orders(offset, limit).await.context("Failed to query DB")?
    };

    Ok(Json(results))
}

const DEFAULT_LIMIT_V2: u64 = 100;

#[utoipa::path(
    get,
    path = ORDER_LIST_PATH_V2,
    params(
        PaginationV2,
    ),
    responses(
        (status = 200, description = "list of orders with pagination info", body = ListOrdersV2Response),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Returns a list of orders with cursor-based pagination and flexible filtering (v2).
///
/// Supports:
/// - Cursor-based pagination for stable results during concurrent inserts
/// - Bidirectional sorting (asc/desc by creation time)
/// - Timestamp range filtering (before/after)
/// - Default sort is descending (newest first)
pub(crate) async fn list_orders_v2(
    State(state): State<Arc<AppState>>,
    paging: Query<PaginationV2>,
) -> Result<Json<ListOrdersV2Response>, AppError> {
    let limit = paging.limit.unwrap_or(DEFAULT_LIMIT_V2);
    let limit = if limit > MAX_ORDERS { MAX_ORDERS } else { limit };
    let limit_i64 = i64::try_from(limit).map_err(|_| AppError::QueryParamErr("limit"))?;

    let sort = match paging.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => return Err(AppError::QueryParamErr("sort: must be 'asc' or 'desc'")),
    };

    let cursor =
        if let Some(cursor_str) = &paging.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    // Request one extra item to efficiently determine if more pages exist
    // without needing a separate COUNT query. If we get limit+1 items back,
    // we know there are more results, and we discard the extra item.
    let limit_plus_one = limit_i64 + 1;
    let mut orders = state
        .db
        .list_orders_v2(cursor, limit_plus_one, sort, paging.before, paging.after)
        .await
        .context("Failed to query DB")?;

    let has_more = orders.len() > limit as usize;
    if has_more {
        orders.pop();
    }

    let next_cursor = if has_more && !orders.is_empty() {
        let last_order = orders.last().unwrap();
        let timestamp = last_order.created_at.ok_or_else(|| {
            AppError::InternalErr(anyhow::anyhow!("Order missing created_at timestamp"))
        })?;
        Some(encode_cursor(timestamp, last_order.id)?)
    } else {
        None
    };

    Ok(Json(ListOrdersV2Response { orders, next_cursor, has_more }))
}

#[utoipa::path(
    get,
    path = format!("{}/<request_id>", ORDER_LIST_PATH),
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
    path = format!("{}/<addr>", AUTH_GET_NONCE),
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
