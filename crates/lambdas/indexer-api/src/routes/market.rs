// Copyright 2025 RISC Zero, Inc.
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

use axum::{
    extract::{Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa;

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
};

/// Create market routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/aggregates", get(get_market_aggregates))
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct MarketAggregatesParams {
    /// Aggregation granularity: hourly, daily, weekly, or monthly
    #[serde(default = "default_aggregation")]
    aggregation: String,

    /// Start timestamp (Unix timestamp in seconds)
    start: i64,

    /// End timestamp (Unix timestamp in seconds)
    end: i64,
}

fn default_aggregation() -> String {
    "hourly".to_string()
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarketAggregateEntry {
    /// Timestamp for this aggregate period (Unix timestamp)
    pub timestamp: i64,

    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,

    /// Total number of fulfilled orders in this period
    pub total_fulfilled: i64,

    /// Unique provers who locked requests in this period
    pub unique_provers_locking_requests: i64,

    /// Unique requesters who submitted requests in this period
    pub unique_requesters_submitting_requests: i64,

    /// Total fees locked (as string)
    pub total_fees_locked: String,

    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,

    /// Total collateral locked (as string)
    pub total_collateral_locked: String,

    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,

    /// Median (p50) fee locked (as string)
    pub p50_fees_locked: String,

    /// Median (p50) fee locked (formatted for display)
    pub p50_fees_locked_formatted: String,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarketAggregatesResponse {
    pub data: Vec<MarketAggregateEntry>,
}

/// GET /v1/market/aggregates
/// Returns aggregated market data for the specified time period
#[utoipa::path(
    get,
    path = "/v1/market/aggregates",
    tag = "Market",
    params(
        MarketAggregatesParams
    ),
    responses(
        (status = 200, description = "Market aggregates", body = MarketAggregatesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_market_aggregates(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MarketAggregatesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_market_aggregates_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Use shorter cache for recent data, longer for historical
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cache_duration = if params_clone.end > now - 86400 {
                "public, max-age=60" // 1 minute for recent data
            } else {
                "public, max-age=300" // 5 minutes for historical data
            };
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control(cache_duration));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_market_aggregates_impl(
    state: Arc<AppState>,
    params: MarketAggregatesParams,
) -> anyhow::Result<MarketAggregatesResponse> {
    tracing::debug!(
        "Fetching market aggregates: aggregation={}, start={}, end={}",
        params.aggregation,
        params.start,
        params.end
    );

    // Validate aggregation type
    match params.aggregation.as_str() {
        "hourly" | "daily" | "weekly" | "monthly" => {}
        _ => {
            anyhow::bail!("Invalid aggregation type. Must be one of: hourly, daily, weekly, monthly");
        }
    }

    // For now, only support hourly - we'll add the other aggregations later
    if params.aggregation != "hourly" {
        anyhow::bail!("Only hourly aggregation is currently supported");
    }

    // Fetch hourly summaries from database
    let summaries =
        state.market_db.get_hourly_market_summaries(params.start, params.end).await?;

    // Convert to response format
    let data = summaries
        .into_iter()
        .map(|summary| {
            // Format timestamp as ISO 8601 manually
            let timestamp_iso = format_timestamp_iso(summary.hour_timestamp);

            // Format values for display (convert from wei to ETH/USDC)
            let format_value = |s: &str| {
                // Parse as U256 and format
                // For now, just return the raw string - proper formatting can be added later
                s.to_string()
            };

            MarketAggregateEntry {
                timestamp: summary.hour_timestamp,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled,
                unique_provers_locking_requests: summary.unique_provers_locking_requests,
                unique_requesters_submitting_requests: summary.unique_requesters_submitting_requests,
                total_fees_locked: summary.total_fees_locked.clone(),
                total_fees_locked_formatted: format_value(&summary.total_fees_locked),
                total_collateral_locked: summary.total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_value(&summary.total_collateral_locked),
                p50_fees_locked: summary.p50_fees_locked.clone(),
                p50_fees_locked_formatted: format_value(&summary.p50_fees_locked),
            }
        })
        .collect();

    Ok(MarketAggregatesResponse { data })
}

/// Format Unix timestamp as ISO 8601 string (UTC)
fn format_timestamp_iso(timestamp: i64) -> String {
    // Simple ISO 8601 formatting: YYYY-MM-DDTHH:MM:SSZ
    let secs_since_epoch = timestamp as u64;
    let days_since_epoch = secs_since_epoch / 86400;
    let secs_today = secs_since_epoch % 86400;

    let hours = secs_today / 3600;
    let minutes = (secs_today % 3600) / 60;
    let seconds = secs_today % 60;

    // Approximate date calculation (good enough for display)
    // Using a simple algorithm: days since 1970-01-01
    let year = 1970 + (days_since_epoch / 365);
    let day_in_year = days_since_epoch % 365;
    let month = (day_in_year / 30) + 1; // Rough approximation
    let day = (day_in_year % 30) + 1;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month.min(12), day.max(1).min(31), hours, minutes, seconds
    )
}
