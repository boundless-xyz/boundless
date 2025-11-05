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
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa;

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
};
use boundless_indexer::db::market::SortDirection;

/// Create market routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/aggregates", get(get_market_aggregates))
}

const MAX_AGGREGATES: u64 = 1000;
const DEFAULT_LIMIT: u64 = 100;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum AggregationGranularity {
    Hourly,
    Daily,
    Weekly,
    Monthly,
}

impl Default for AggregationGranularity {
    fn default() -> Self {
        Self::Monthly
    }
}

impl std::fmt::Display for AggregationGranularity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hourly => write!(f, "hourly"),
            Self::Daily => write!(f, "daily"),
            Self::Weekly => write!(f, "weekly"),
            Self::Monthly => write!(f, "monthly"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct MarketAggregatesParams {
    /// Aggregation granularity: hourly, daily, weekly, or monthly
    #[serde(default)]
    aggregation: AggregationGranularity,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of aggregates returned, max 1000 (default 100)
    #[serde(default)]
    limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc")
    #[serde(default)]
    sort: Option<String>,

    /// Unix timestamp to fetch aggregates before this time
    #[serde(default)]
    before: Option<i64>,

    /// Unix timestamp to fetch aggregates after this time
    #[serde(default)]
    after: Option<i64>,
}

fn encode_cursor(timestamp: i64) -> Result<String, anyhow::Error> {
    let json = serde_json::to_string(&timestamp)?;
    Ok(BASE64.encode(json))
}

fn decode_cursor(cursor_str: &str) -> Result<i64, anyhow::Error> {
    let json = BASE64.decode(cursor_str)?;
    let timestamp: i64 = serde_json::from_slice(&json)?;
    Ok(timestamp)
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

    /// 10th percentile fee locked (as string)
    pub p10_fees_locked: String,

    /// 10th percentile fee locked (formatted for display)
    pub p10_fees_locked_formatted: String,

    /// 25th percentile fee locked (as string)
    pub p25_fees_locked: String,

    /// 25th percentile fee locked (formatted for display)
    pub p25_fees_locked_formatted: String,

    /// Median (p50) fee locked (as string)
    pub p50_fees_locked: String,

    /// Median (p50) fee locked (formatted for display)
    pub p50_fees_locked_formatted: String,

    /// 75th percentile fee locked (as string)
    pub p75_fees_locked: String,

    /// 75th percentile fee locked (formatted for display)
    pub p75_fees_locked_formatted: String,

    /// 90th percentile fee locked (as string)
    pub p90_fees_locked: String,

    /// 90th percentile fee locked (formatted for display)
    pub p90_fees_locked_formatted: String,

    /// 95th percentile fee locked (as string)
    pub p95_fees_locked: String,

    /// 95th percentile fee locked (formatted for display)
    pub p95_fees_locked_formatted: String,

    /// 99th percentile fee locked (as string)
    pub p99_fees_locked: String,

    /// 99th percentile fee locked (formatted for display)
    pub p99_fees_locked_formatted: String,

    /// Total number of requests submitted in this period
    pub total_requests_submitted: i64,

    /// Total number of requests submitted onchain in this period
    pub total_requests_submitted_onchain: i64,

    /// Total number of requests submitted offchain in this period
    pub total_requests_submitted_offchain: i64,

    /// Total number of requests locked in this period
    pub total_requests_locked: i64,

    /// Total number of requests slashed in this period
    pub total_requests_slashed: i64,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct MarketAggregatesResponse {
    /// The aggregation granularity used: hourly, daily, weekly, or monthly
    pub aggregation: AggregationGranularity,
    pub data: Vec<MarketAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
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
            // If querying recent data (no before filter or before is within last 24h), use short cache
            let is_recent = params_clone.before.is_none_or(|before| before > now - 86400);
            let cache_duration = if is_recent {
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
        "Fetching market aggregates: aggregation={}, cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        params.aggregation,
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    // Parse cursor if provided
    let cursor = if let Some(cursor_str) = &params.cursor {
        Some(decode_cursor(cursor_str)?)
    } else {
        None
    };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT);
    let limit = if limit > MAX_AGGREGATES { MAX_AGGREGATES } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    // Parse sort direction
    let sort = match params.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => anyhow::bail!("Invalid sort direction. Must be 'asc' or 'desc'"),
    };

    // Request one extra item to efficiently determine if more pages exist
    // without needing a separate COUNT query. If we get limit+1 items back,
    // we know there are more results, and we discard the extra item.
    let limit_plus_one = limit_i64 + 1;
    
    // Route to appropriate database method based on aggregation type
    let mut summaries = match params.aggregation {
        AggregationGranularity::Hourly => state
            .market_db
            .get_hourly_market_summaries(cursor, limit_plus_one, sort, params.before, params.after)
            .await?,
        AggregationGranularity::Daily => state
            .market_db
            .get_daily_market_summaries(cursor, limit_plus_one, sort, params.before, params.after)
            .await?,
        AggregationGranularity::Weekly => state
            .market_db
            .get_weekly_market_summaries(cursor, limit_plus_one, sort, params.before, params.after)
            .await?,
        AggregationGranularity::Monthly => state
            .market_db
            .get_monthly_market_summaries(cursor, limit_plus_one, sort, params.before, params.after)
            .await?,
    };

    let has_more = summaries.len() > limit as usize;
    if has_more {
        summaries.pop();
    }

    // Generate next cursor if there are more results
    let next_cursor = if has_more && !summaries.is_empty() {
        let last_summary = summaries.last().unwrap();
        Some(encode_cursor(last_summary.period_timestamp as i64)?)
    } else {
        None
    };

    // Convert to response format
    let data = summaries
        .into_iter()
        .map(|summary| {
            // Format timestamp as ISO 8601 manually
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            // Format values for display (convert from wei to ETH/USDC)
            let format_value = |s: &str| {
                // Parse as U256 and format
                // For now, just return the raw string - proper formatting can be added later
                s.to_string()
            };

            MarketAggregateEntry {
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                unique_requesters_submitting_requests: summary.unique_requesters_submitting_requests as i64,
                total_fees_locked: summary.total_fees_locked.clone(),
                total_fees_locked_formatted: format_value(&summary.total_fees_locked),
                total_collateral_locked: summary.total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_value(&summary.total_collateral_locked),
                p10_fees_locked: summary.p10_fees_locked.clone(),
                p10_fees_locked_formatted: format_value(&summary.p10_fees_locked),
                p25_fees_locked: summary.p25_fees_locked.clone(),
                p25_fees_locked_formatted: format_value(&summary.p25_fees_locked),
                p50_fees_locked: summary.p50_fees_locked.clone(),
                p50_fees_locked_formatted: format_value(&summary.p50_fees_locked),
                p75_fees_locked: summary.p75_fees_locked.clone(),
                p75_fees_locked_formatted: format_value(&summary.p75_fees_locked),
                p90_fees_locked: summary.p90_fees_locked.clone(),
                p90_fees_locked_formatted: format_value(&summary.p90_fees_locked),
                p95_fees_locked: summary.p95_fees_locked.clone(),
                p95_fees_locked_formatted: format_value(&summary.p95_fees_locked),
                p99_fees_locked: summary.p99_fees_locked.clone(),
                p99_fees_locked_formatted: format_value(&summary.p99_fees_locked),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
            }
        })
        .collect();

    Ok(MarketAggregatesResponse { 
        aggregation: params.aggregation,
        data, 
        next_cursor, 
        has_more 
    })
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
        year, month.min(12), day.clamp(1, 31), hours, minutes, seconds
    )
}
