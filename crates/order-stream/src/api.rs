use anyhow::Context;
use axum::extract::{Json, State};
use serde_json::json;
use std::sync::Arc;

use crate::{AppError, AppState, Order};

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
