use alloy::primitives::Address;
use anyhow::Context;
use axum::extract::{Json, Path, Query, State};
use boundless_market::order_stream_client::Nonce;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

use crate::{
    order_db::{DbOrder, OrderDbErr},
    AppError, AppState, Order,
};

// Submit order handler
pub(crate) async fn submit_order(
    State(state): State<Arc<AppState>>,
    Json(order): Json<Order>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Validate the order
    order.validate(state.config.market_address, state.chain_id)?;
    let order_req_id = order.request.id;
    let order_id = state.db.add_order(order).await.context("failed to add order to db")?;

    tracing::debug!("Order 0x{order_req_id:x} - [{order_id}] submitted",);
    Ok(Json(json!({ "status": "success", "request_id": order_req_id })))
}

#[derive(Deserialize)]
pub struct Pagination {
    offset: u64,
    limit: u64,
}

const MAX_ORDERS: u64 = 1000;

pub(crate) async fn list_orders(
    State(state): State<Arc<AppState>>,
    paging: Query<Pagination>,
) -> Result<Json<Vec<DbOrder>>, AppError> {
    let limit = if paging.limit > MAX_ORDERS { MAX_ORDERS } else { paging.limit };
    // i64::try_from converts to non-zero u64
    let limit = i64::try_from(limit).map_err(|_| AppError::QueryParamErr("limit"))?;
    let offset = i64::try_from(paging.offset).map_err(|_| AppError::QueryParamErr("index"))?;

    let results = state.db.list_orders(limit, offset).await.context("Failed to query DB")?;
    Ok(Json(results))
}

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
