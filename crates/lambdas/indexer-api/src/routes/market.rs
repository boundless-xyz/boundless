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

use alloy::primitives::{Address, U256};
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::{str::FromStr, sync::Arc};
use utoipa;

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
    utils::{format_eth, format_zkc},
};
use boundless_indexer::db::market::{RequestCursor, RequestSortField, RequestStatus, SortDirection};

/// Create market routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/aggregates", get(get_market_aggregates))
        .route("/requests", get(list_requests))
        .route("/requests/:request_id", get(get_requests_by_request_id))
        .route("/requestors/:address/requests", get(list_requests_by_requestor))
        .route("/provers/:address/requests", get(list_requests_by_prover))
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

    /// Total number of requests that expired in this period
    pub total_expired: i64,

    /// Total number of locked requests that expired in this period
    pub total_locked_and_expired: i64,

    /// Total number of locked requests that were fulfilled in this period
    pub total_locked_and_fulfilled: i64,

    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: Option<f32>,
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

            // Normalize padded strings by parsing as U256 and converting back to clean string
            let normalize = |s: &str| -> String {
                U256::from_str(s).map(|v| v.to_string()).unwrap_or_else(|_| "0".to_string())
            };

            // Normalize all currency fields
            let total_fees_locked = normalize(&summary.total_fees_locked);
            let total_collateral_locked = normalize(&summary.total_collateral_locked);
            let p10_fees_locked = normalize(&summary.p10_fees_locked);
            let p25_fees_locked = normalize(&summary.p25_fees_locked);
            let p50_fees_locked = normalize(&summary.p50_fees_locked);
            let p75_fees_locked = normalize(&summary.p75_fees_locked);
            let p90_fees_locked = normalize(&summary.p90_fees_locked);
            let p95_fees_locked = normalize(&summary.p95_fees_locked);
            let p99_fees_locked = normalize(&summary.p99_fees_locked);

            MarketAggregateEntry {
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                unique_requesters_submitting_requests: summary.unique_requesters_submitting_requests as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                p10_fees_locked: p10_fees_locked.clone(),
                p10_fees_locked_formatted: format_eth(&p10_fees_locked),
                p25_fees_locked: p25_fees_locked.clone(),
                p25_fees_locked_formatted: format_eth(&p25_fees_locked),
                p50_fees_locked: p50_fees_locked.clone(),
                p50_fees_locked_formatted: format_eth(&p50_fees_locked),
                p75_fees_locked: p75_fees_locked.clone(),
                p75_fees_locked_formatted: format_eth(&p75_fees_locked),
                p90_fees_locked: p90_fees_locked.clone(),
                p90_fees_locked_formatted: format_eth(&p90_fees_locked),
                p95_fees_locked: p95_fees_locked.clone(),
                p95_fees_locked_formatted: format_eth(&p95_fees_locked),
                p99_fees_locked: p99_fees_locked.clone(),
                p99_fees_locked_formatted: format_eth(&p99_fees_locked),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
                total_expired: summary.total_expired as i64,
                total_locked_and_expired: summary.total_locked_and_expired as i64,
                total_locked_and_fulfilled: summary.total_locked_and_fulfilled as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
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
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

const MAX_REQUESTS: u32 = 100;
const DEFAULT_REQUESTS_LIMIT: u32 = 20;

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct RequestListParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of requests returned, max 100 (default 20)
    #[serde(default)]
    limit: Option<u32>,

    /// Sort field: "updated_at" or "created_at" (default "updated_at")
    #[serde(default)]
    sort_by: Option<String>,
}

fn encode_request_cursor(cursor: &RequestCursor) -> Result<String, anyhow::Error> {
    let json = serde_json::to_string(cursor)?;
    Ok(BASE64.encode(json))
}

fn decode_request_cursor(cursor_str: &str) -> Result<RequestCursor, anyhow::Error> {
    let json = BASE64.decode(cursor_str)?;
    let cursor: RequestCursor = serde_json::from_slice(&json)?;
    Ok(cursor)
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RequestStatusResponse {
    /// Request digest (unique identifier)
    pub request_digest: String,
    /// Request ID (can be non-unique)
    pub request_id: String,
    /// Current status: submitted, locked, fulfilled, slashed, expired
    pub request_status: String,
    /// Source: onchain, offchain, unknown
    pub source: String,
    /// Client address
    pub client_address: String,
    /// Prover address (if locked)
    pub lock_prover_address: Option<String>,
    /// Fulfill prover address (if fulfilled)
    pub fulfill_prover_address: Option<String>,
    /// Created timestamp (Unix)
    pub created_at: i64,
    /// Last updated timestamp (Unix)
    pub updated_at: i64,
    /// Locked timestamp (Unix)
    pub locked_at: Option<i64>,
    /// Fulfilled timestamp (Unix)
    pub fulfilled_at: Option<i64>,
    /// Slashed timestamp (Unix)
    pub slashed_at: Option<i64>,
    /// Submit block number
    pub submit_block: Option<i64>,
    /// Lock block number
    pub lock_block: Option<i64>,
    /// Fulfill block number
    pub fulfill_block: Option<i64>,
    /// Slashed block number
    pub slashed_block: Option<i64>,
    /// Minimum price (wei)
    pub min_price: String,
    /// Maximum price (wei)
    pub max_price: String,
    /// Lock collateral (wei)
    pub lock_collateral: String,
    /// Ramp up start timestamp
    pub ramp_up_start: i64,
    /// Ramp up period (seconds)
    pub ramp_up_period: i64,
    /// Expires at timestamp
    pub expires_at: i64,
    /// Lock end timestamp
    pub lock_end: i64,
    /// Slash recipient address
    pub slash_recipient: Option<String>,
    /// Slash transferred amount
    pub slash_transferred_amount: Option<String>,
    /// Slash burned amount
    pub slash_burned_amount: Option<String>,
    /// Cycles
    pub cycles: Option<i64>,
    /// Peak prove MHz
    pub peak_prove_mhz: Option<i64>,
    /// Effective prove MHz
    pub effective_prove_mhz: Option<i64>,
    /// Submit transaction hash
    pub submit_tx_hash: Option<String>,
    /// Lock transaction hash
    pub lock_tx_hash: Option<String>,
    /// Fulfill transaction hash
    pub fulfill_tx_hash: Option<String>,
    /// Slash transaction hash
    pub slash_tx_hash: Option<String>,
    /// Image ID
    pub image_id: String,
    /// Image URL
    pub image_url: Option<String>,
    /// Selector
    pub selector: String,
    /// Predicate type
    pub predicate_type: String,
    /// Predicate data (hex)
    pub predicate_data: String,
    /// Input type
    pub input_type: String,
    /// Input data (hex)
    pub input_data: String,
    /// Fulfillment journal (hex)
    pub fulfill_journal: Option<String>,
    /// Fulfillment seal (hex)
    pub fulfill_seal: Option<String>,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RequestListResponse {
    pub data: Vec<RequestStatusResponse>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

fn convert_request_status(status: RequestStatus) -> RequestStatusResponse {
    RequestStatusResponse {
        request_digest: format!("{:x}", status.request_digest),
        request_id: status.request_id,
        request_status: status.request_status.to_string(),
        source: status.source,
        client_address: format!("{:x}", status.client_address),
        lock_prover_address: status.lock_prover_address.map(|addr| format!("{:x}", addr)),
        fulfill_prover_address: status.fulfill_prover_address.map(|addr| format!("{:x}", addr)),
        created_at: status.created_at as i64,
        updated_at: status.updated_at as i64,
        locked_at: status.locked_at.map(|t| t as i64),
        fulfilled_at: status.fulfilled_at.map(|t| t as i64),
        slashed_at: status.slashed_at.map(|t| t as i64),
        submit_block: status.submit_block.map(|b| b as i64),
        lock_block: status.lock_block.map(|b| b as i64),
        fulfill_block: status.fulfill_block.map(|b| b as i64),
        slashed_block: status.slashed_block.map(|b| b as i64),
        min_price: status.min_price,
        max_price: status.max_price,
        lock_collateral: status.lock_collateral,
        ramp_up_start: status.ramp_up_start as i64,
        ramp_up_period: status.ramp_up_period as i64,
        expires_at: status.expires_at as i64,
        lock_end: status.lock_end as i64,
        slash_recipient: status.slash_recipient.map(|addr| format!("{:x}", addr)),
        slash_transferred_amount: status.slash_transferred_amount,
        slash_burned_amount: status.slash_burned_amount,
        cycles: status.cycles.map(|c| c as i64),
        peak_prove_mhz: status.peak_prove_mhz.map(|m| m as i64),
        effective_prove_mhz: status.effective_prove_mhz.map(|m| m as i64),
        submit_tx_hash: status.submit_tx_hash.map(|h| format!("{:x}", h)),
        lock_tx_hash: status.lock_tx_hash.map(|h| format!("{:x}", h)),
        fulfill_tx_hash: status.fulfill_tx_hash.map(|h| format!("{:x}", h)),
        slash_tx_hash: status.slash_tx_hash.map(|h| format!("{:x}", h)),
        image_id: status.image_id,
        image_url: status.image_url,
        selector: status.selector,
        predicate_type: status.predicate_type,
        predicate_data: status.predicate_data,
        input_type: status.input_type,
        input_data: status.input_data,
        fulfill_journal: status.fulfill_journal,
        fulfill_seal: status.fulfill_seal,
    }
}

/// GET /v1/market/requests
/// Returns a paginated list of all requests
#[utoipa::path(
    get,
    path = "/v1/market/requests",
    tag = "Market",
    params(RequestListParams),
    responses(
        (status = 200, description = "List of requests", body = RequestListResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
async fn list_requests(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RequestListParams>,
) -> Response {
    match list_requests_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=10"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_requests_impl(
    state: Arc<AppState>,
    params: RequestListParams,
) -> anyhow::Result<RequestListResponse> {
    let cursor = if let Some(cursor_str) = &params.cursor {
        Some(decode_request_cursor(cursor_str)?)
    } else {
        None
    };

    let limit = params.limit.unwrap_or(DEFAULT_REQUESTS_LIMIT).min(MAX_REQUESTS);

    let sort_by = match params.sort_by.as_deref() {
        Some("created_at") => RequestSortField::CreatedAt,
        Some("updated_at") | None => RequestSortField::UpdatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) = state.market_db.list_requests(cursor, limit, sort_by).await?;

    let data = statuses.into_iter().map(convert_request_status).collect();
    let next_cursor_encoded = next_cursor.as_ref().map(|c| encode_request_cursor(c)).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        data,
        next_cursor: next_cursor_encoded,
        has_more,
    })
}

/// GET /v1/market/requestors/:address/requests
/// Returns requests submitted by a specific requestor
#[utoipa::path(
    get,
    path = "/v1/market/requestors/{address}/requests",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Requestor address"),
        RequestListParams
    ),
    responses(
        (status = 200, description = "List of requests", body = RequestListResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
async fn list_requests_by_requestor(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<RequestListParams>,
) -> Response {
    match list_requests_by_requestor_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=10"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_requests_by_requestor_impl(
    state: Arc<AppState>,
    address: String,
    params: RequestListParams,
) -> anyhow::Result<RequestListResponse> {
    let client_address = Address::from_str(&address)?;

    let cursor = if let Some(cursor_str) = &params.cursor {
        Some(decode_request_cursor(cursor_str)?)
    } else {
        None
    };

    let limit = params.limit.unwrap_or(DEFAULT_REQUESTS_LIMIT).min(MAX_REQUESTS);

    let sort_by = match params.sort_by.as_deref() {
        Some("created_at") => RequestSortField::CreatedAt,
        Some("updated_at") | None => RequestSortField::UpdatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) = state
        .market_db
        .list_requests_by_requestor(client_address, cursor, limit, sort_by)
        .await?;

    let data = statuses.into_iter().map(convert_request_status).collect();
    let next_cursor_encoded = next_cursor.as_ref().map(|c| encode_request_cursor(c)).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        data,
        next_cursor: next_cursor_encoded,
        has_more,
    })
}

/// GET /v1/market/provers/:address/requests
/// Returns requests fulfilled by a specific prover
#[utoipa::path(
    get,
    path = "/v1/market/provers/{address}/requests",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Prover address"),
        RequestListParams
    ),
    responses(
        (status = 200, description = "List of requests", body = RequestListResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
async fn list_requests_by_prover(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<RequestListParams>,
) -> Response {
    match list_requests_by_prover_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=10"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_requests_by_prover_impl(
    state: Arc<AppState>,
    address: String,
    params: RequestListParams,
) -> anyhow::Result<RequestListResponse> {
    let prover_address = Address::from_str(&address)?;

    let cursor = if let Some(cursor_str) = &params.cursor {
        Some(decode_request_cursor(cursor_str)?)
    } else {
        None
    };

    let limit = params.limit.unwrap_or(DEFAULT_REQUESTS_LIMIT).min(MAX_REQUESTS);

    let sort_by = match params.sort_by.as_deref() {
        Some("created_at") => RequestSortField::CreatedAt,
        Some("updated_at") | None => RequestSortField::UpdatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) = state
        .market_db
        .list_requests_by_prover(prover_address, cursor, limit, sort_by)
        .await?;

    let data = statuses.into_iter().map(convert_request_status).collect();
    let next_cursor_encoded = next_cursor.as_ref().map(|c| encode_request_cursor(c)).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        data,
        next_cursor: next_cursor_encoded,
        has_more,
    })
}

/// GET /v1/market/requests/:request_id
/// Returns all requests matching a specific request ID
#[utoipa::path(
    get,
    path = "/v1/market/requests/{request_id}",
    tag = "Market",
    params(
        ("request_id" = String, Path, description = "Request ID (hex)")
    ),
    responses(
        (status = 200, description = "List of requests with this ID", body = Vec<RequestStatusResponse>),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_requests_by_request_id(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> Response {
    match get_requests_by_request_id_impl(state, request_id).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_requests_by_request_id_impl(
    state: Arc<AppState>,
    request_id: String,
) -> anyhow::Result<Vec<RequestStatusResponse>> {
    let statuses = state.market_db.get_requests_by_request_id(&request_id).await?;
    let data = statuses.into_iter().map(convert_request_status).collect();
    Ok(data)
}
