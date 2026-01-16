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
use boundless_indexer::db::market::{
    RequestCursor, RequestSortField, RequestStatus, SortDirection,
};
use boundless_indexer::db::{IndexerDb, ProversDb, RequestorDb};

const MAX_AGGREGATES: u64 = 500;
const DEFAULT_AGGREGATES_LIMIT: u64 = 50;
const MAX_REQUESTS: u32 = 500;
const DEFAULT_REQUESTS_LIMIT: u32 = 50;

/// Create market routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_indexing_status))
        .route("/aggregates", get(get_market_aggregates))
        .route("/cumulatives", get(get_market_cumulatives))
        .route("/requests", get(list_requests))
        .route("/requests/:request_id", get(get_requests_by_request_id))
        .route("/requestors", get(list_requestors))
        .route("/requestors/:address/requests", get(list_requests_by_requestor))
        .route("/requestors/:address/aggregates", get(get_requestor_aggregates))
        .route("/requestors/:address/cumulatives", get(get_requestor_cumulatives))
        .route("/provers", get(list_provers))
        .route("/provers/:address/requests", get(list_requests_by_prover))
        .route("/provers/:address/aggregates", get(get_prover_aggregates))
        .route("/provers/:address/cumulatives", get(get_prover_cumulatives))
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IndexingStatusResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Last indexed block number
    pub last_indexed_block: u64,
    /// Last indexed block timestamp (Unix timestamp)
    pub last_indexed_block_timestamp: i64,
    /// Last indexed block timestamp (ISO 8601)
    pub last_indexed_block_timestamp_iso: String,
}

/// GET /v1/market
/// Returns the current indexing status
#[utoipa::path(
    get,
    path = "/v1/market",
    tag = "Market",
    responses(
        (status = 200, description = "Indexing status", body = IndexingStatusResponse),
        (status = 404, description = "No indexing data available"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_indexing_status(State(state): State<Arc<AppState>>) -> Response {
    match get_indexing_status_impl(state).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=10"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_indexing_status_impl(state: Arc<AppState>) -> anyhow::Result<IndexingStatusResponse> {
    let last_block = state
        .market_db
        .get_last_block()
        .await?
        .ok_or_else(|| anyhow::anyhow!("No indexing data available"))?;

    // TODO

    // let timestamp = state
    //     .market_db
    //     .get_block_timestamp(last_block)
    //     .await?
    //     .ok_or_else(|| anyhow::anyhow!("Block timestamp not found"))?;

    let timestamp_i64 = 0;

    Ok(IndexingStatusResponse {
        chain_id: state.chain_id,
        last_indexed_block: last_block,
        last_indexed_block_timestamp: timestamp_i64,
        last_indexed_block_timestamp_iso: format_timestamp_iso(timestamp_i64),
    })
}

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

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct MarketCumulativesParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of cumulatives returned, max 1000 (default 100)
    #[serde(default)]
    limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc" - newest first)
    #[serde(default)]
    sort: Option<String>,

    /// Unix timestamp to fetch cumulatives before this time
    #[serde(default)]
    before: Option<i64>,

    /// Unix timestamp to fetch cumulatives after this time
    #[serde(default)]
    after: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct RequestorAggregatesParams {
    /// Aggregation granularity: hourly, daily, or weekly (monthly not supported)
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

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct RequestorCumulativesParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of cumulatives returned, max 1000 (default 100)
    #[serde(default)]
    limit: Option<u64>,

    /// Sort order: "asc" or "desc" (default "desc" - newest first)
    #[serde(default)]
    sort: Option<String>,

    /// Unix timestamp to fetch cumulatives before this time
    #[serde(default)]
    before: Option<i64>,

    /// Unix timestamp to fetch cumulatives after this time
    #[serde(default)]
    after: Option<i64>,
}

// Leaderboard period enum for filtering by time range
#[derive(Debug, Clone, Copy, Default, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LeaderboardPeriod {
    /// Last 1 hour
    #[serde(rename = "1h")]
    OneHour,
    /// Last 1 day (24 hours)
    #[serde(rename = "1d")]
    OneDay,
    /// Last 3 days
    #[serde(rename = "3d")]
    ThreeDays,
    /// Last 7 days
    #[serde(rename = "7d")]
    SevenDays,
    /// All time
    #[default]
    #[serde(rename = "all")]
    AllTime,
}

impl std::fmt::Display for LeaderboardPeriod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OneHour => write!(f, "1h"),
            Self::OneDay => write!(f, "1d"),
            Self::ThreeDays => write!(f, "3d"),
            Self::SevenDays => write!(f, "7d"),
            Self::AllTime => write!(f, "all"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct RequestorLeaderboardParams {
    /// Time period: 1h, 1d, 3d, 7d, or all (default: all)
    #[serde(default)]
    period: LeaderboardPeriod,

    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of results returned, max 100 (default 50)
    #[serde(default)]
    limit: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorLeaderboardCursor {
    pub orders_requested: u64,
    pub address: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorLeaderboardEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address (checksummed)
    pub requestor_address: String,
    /// Total orders requested in the period
    pub orders_requested: u64,
    /// Total orders locked in the period
    pub orders_locked: u64,
    /// Total cycles requested (as string)
    pub cycles_requested: String,
    /// Total cycles requested (formatted for display)
    pub cycles_requested_formatted: String,
    /// Median lock price per cycle (as string, null if no locked orders)
    pub median_lock_price_per_cycle: Option<String>,
    /// Median lock price per cycle (formatted for display)
    pub median_lock_price_per_cycle_formatted: Option<String>,
    /// Acceptance rate (locked / (locked + not_locked_and_expired)) as percentage
    pub acceptance_rate: f32,
    /// Locked order fulfillment rate (locked and fulfilled / (locked and fulfilled + locked and expired)) as percentage
    pub locked_order_fulfillment_rate: f32,
    /// Locked order fulfillment rate adjusted (deduplicated by input_data and image_url) as percentage
    pub locked_orders_fulfillment_rate_adjusted: f32,
    /// Last activity timestamp (Unix)
    pub last_activity_time: i64,
    /// Last activity timestamp (ISO 8601)
    pub last_activity_time_iso: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorLeaderboardResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Time period for the leaderboard
    pub period: String,
    /// Start timestamp of the query period (Unix)
    pub period_start: i64,
    /// End timestamp of the query period (Unix)
    pub period_end: i64,
    /// Leaderboard entries sorted by cycles
    pub data: Vec<RequestorLeaderboardEntry>,
    /// Cursor for next page, null if no more results
    pub next_cursor: Option<String>,
    /// Whether there are more results available
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverLeaderboardCursor {
    pub fees_earned: String,
    pub address: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverLeaderboardEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Prover address (checksummed)
    pub prover_address: String,
    /// Total orders locked in the period
    pub orders_locked: u64,
    /// Total orders fulfilled in the period
    pub orders_fulfilled: u64,
    /// Total cycles proven (as string)
    pub cycles: String,
    /// Total cycles proven (formatted for display)
    pub cycles_formatted: String,
    /// Total fees earned (as string)
    pub fees_earned: String,
    /// Total fees earned (formatted for display)
    pub fees_earned_formatted: String,
    /// Total collateral earned from slashing (as string)
    pub collateral_earned: String,
    /// Total collateral earned (formatted for display)
    pub collateral_earned_formatted: String,
    /// Median lock price per cycle (as string, null if no locked orders)
    pub median_lock_price_per_cycle: Option<String>,
    /// Median lock price per cycle (formatted for display)
    pub median_lock_price_per_cycle_formatted: Option<String>,
    /// Best effective proving speed in MHz (max of effective_prove_mhz across all fulfilled requests)
    pub best_effective_prove_mhz: f64,
    /// Locked order fulfillment rate as percentage (0-100)
    pub locked_order_fulfillment_rate: f32,
    /// Last activity timestamp (Unix)
    pub last_activity_time: i64,
    /// Last activity timestamp (ISO 8601)
    pub last_activity_time_iso: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverLeaderboardResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Time period for the leaderboard
    pub period: String,
    /// Start timestamp of the query period (Unix)
    pub period_start: i64,
    /// End timestamp of the query period (Unix)
    pub period_end: i64,
    /// Leaderboard entries sorted by fees earned
    pub data: Vec<ProverLeaderboardEntry>,
    /// Cursor for next page, null if no more results
    pub next_cursor: Option<String>,
    /// Whether there are more results available
    pub has_more: bool,
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

fn encode_requestor_leaderboard_cursor(
    cursor: &RequestorLeaderboardCursor,
) -> Result<String, anyhow::Error> {
    let json = serde_json::to_string(cursor)?;
    Ok(BASE64.encode(json))
}

fn decode_requestor_leaderboard_cursor(
    cursor_str: &str,
) -> Result<RequestorLeaderboardCursor, anyhow::Error> {
    let json = BASE64.decode(cursor_str)?;
    let cursor: RequestorLeaderboardCursor = serde_json::from_slice(&json)?;
    Ok(cursor)
}

fn encode_prover_leaderboard_cursor(
    cursor: &ProverLeaderboardCursor,
) -> Result<String, anyhow::Error> {
    let json = serde_json::to_string(cursor)?;
    Ok(BASE64.encode(json))
}

fn decode_prover_leaderboard_cursor(
    cursor_str: &str,
) -> Result<ProverLeaderboardCursor, anyhow::Error> {
    let json = BASE64.decode(cursor_str)?;
    let cursor: ProverLeaderboardCursor = serde_json::from_slice(&json)?;
    Ok(cursor)
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MarketAggregateEntry {
    /// Chain ID
    pub chain_id: u64,
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

    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,

    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,

    /// 10th percentile lock price per cycle (as string)
    pub p10_lock_price_per_cycle: String,

    /// 10th percentile lock price per cycle (formatted for display)
    pub p10_lock_price_per_cycle_formatted: String,

    /// 25th percentile lock price per cycle (as string)
    pub p25_lock_price_per_cycle: String,

    /// 25th percentile lock price per cycle (formatted for display)
    pub p25_lock_price_per_cycle_formatted: String,

    /// Median (p50) lock price per cycle (as string)
    pub p50_lock_price_per_cycle: String,

    /// Median (p50) lock price per cycle (formatted for display)
    pub p50_lock_price_per_cycle_formatted: String,

    /// 75th percentile lock price per cycle (as string)
    pub p75_lock_price_per_cycle: String,

    /// 75th percentile lock price per cycle (formatted for display)
    pub p75_lock_price_per_cycle_formatted: String,

    /// 90th percentile lock price per cycle (as string)
    pub p90_lock_price_per_cycle: String,

    /// 90th percentile lock price per cycle (formatted for display)
    pub p90_lock_price_per_cycle_formatted: String,

    /// 95th percentile lock price per cycle (as string)
    pub p95_lock_price_per_cycle: String,

    /// 95th percentile lock price per cycle (formatted for display)
    pub p95_lock_price_per_cycle_formatted: String,

    /// 99th percentile lock price per cycle (as string)
    pub p99_lock_price_per_cycle: String,

    /// 99th percentile lock price per cycle (formatted for display)
    pub p99_lock_price_per_cycle_formatted: String,

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

    /// Total number of secondary fulfillments in this period
    pub total_secondary_fulfillments: i64,

    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,

    /// Adjusted fulfillment rate for locked orders, deduplicated by (input_data, image_url) (percentage)
    pub locked_orders_fulfillment_rate_adjusted: f32,

    /// Total program cycles computed across all fulfilled requests in this period
    pub total_program_cycles: String,

    /// Total cycles (program + overhead) computed across all fulfilled requests in this period
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MarketAggregatesResponse {
    /// Chain ID
    pub chain_id: u64,
    /// The aggregation granularity used: hourly, daily, weekly, or monthly
    pub aggregation: AggregationGranularity,
    pub data: Vec<MarketAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MarketCumulativeEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Timestamp for this cumulative snapshot (Unix timestamp)
    pub timestamp: i64,

    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,

    /// Total number of fulfilled orders (cumulative)
    pub total_fulfilled: i64,

    /// Unique provers who locked requests (cumulative)
    pub unique_provers_locking_requests: i64,

    /// Unique requesters who submitted requests (cumulative)
    pub unique_requesters_submitting_requests: i64,

    /// Total fees locked (as string)
    pub total_fees_locked: String,

    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,

    /// Total collateral locked (as string)
    pub total_collateral_locked: String,

    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,

    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,

    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,

    /// Total number of requests submitted (cumulative)
    pub total_requests_submitted: i64,

    /// Total number of requests submitted onchain (cumulative)
    pub total_requests_submitted_onchain: i64,

    /// Total number of requests submitted offchain (cumulative)
    pub total_requests_submitted_offchain: i64,

    /// Total number of requests locked (cumulative)
    pub total_requests_locked: i64,

    /// Total number of requests slashed (cumulative)
    pub total_requests_slashed: i64,

    /// Total number of requests that expired (cumulative)
    pub total_expired: i64,

    /// Total number of locked requests that expired (cumulative)
    pub total_locked_and_expired: i64,

    /// Total number of locked requests that were fulfilled (cumulative)
    pub total_locked_and_fulfilled: i64,

    /// Total number of secondary fulfillments (cumulative)
    pub total_secondary_fulfillments: i64,

    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,

    /// Adjusted fulfillment rate for locked orders, deduplicated by (input_data, image_url) (percentage)
    pub locked_orders_fulfillment_rate_adjusted: f32,

    /// Total program cycles computed across all fulfilled requests (cumulative)
    pub total_program_cycles: String,

    /// Total cycles (program + overhead) computed across all fulfilled requests (cumulative)
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MarketCumulativesResponse {
    /// Chain ID
    pub chain_id: u64,
    pub data: Vec<MarketCumulativeEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorAggregateEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// Timestamp for this aggregate period (Unix timestamp)
    pub timestamp: i64,

    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,

    /// Total number of fulfilled orders in this period
    pub total_fulfilled: i64,

    /// Unique provers who locked requests in this period
    pub unique_provers_locking_requests: i64,

    /// Total fees locked (as string)
    pub total_fees_locked: String,

    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,

    /// Total collateral locked (as string)
    pub total_collateral_locked: String,

    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,

    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,

    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,

    /// 10th percentile lock price per cycle (as string)
    pub p10_lock_price_per_cycle: String,

    /// 10th percentile lock price per cycle (formatted for display)
    pub p10_lock_price_per_cycle_formatted: String,

    /// 25th percentile lock price per cycle (as string)
    pub p25_lock_price_per_cycle: String,

    /// 25th percentile lock price per cycle (formatted for display)
    pub p25_lock_price_per_cycle_formatted: String,

    /// Median (p50) lock price per cycle (as string)
    pub p50_lock_price_per_cycle: String,

    /// Median (p50) lock price per cycle (formatted for display)
    pub p50_lock_price_per_cycle_formatted: String,

    /// 75th percentile lock price per cycle (as string)
    pub p75_lock_price_per_cycle: String,

    /// 75th percentile lock price per cycle (formatted for display)
    pub p75_lock_price_per_cycle_formatted: String,

    /// 90th percentile lock price per cycle (as string)
    pub p90_lock_price_per_cycle: String,

    /// 90th percentile lock price per cycle (formatted for display)
    pub p90_lock_price_per_cycle_formatted: String,

    /// 95th percentile lock price per cycle (as string)
    pub p95_lock_price_per_cycle: String,

    /// 95th percentile lock price per cycle (formatted for display)
    pub p95_lock_price_per_cycle_formatted: String,

    /// 99th percentile lock price per cycle (as string)
    pub p99_lock_price_per_cycle: String,

    /// 99th percentile lock price per cycle (formatted for display)
    pub p99_lock_price_per_cycle_formatted: String,

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

    /// Total number of secondary fulfillments in this period
    pub total_secondary_fulfillments: i64,

    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,

    /// Adjusted fulfillment rate for locked orders, deduplicated by (input_data, image_url) (percentage)
    pub locked_orders_fulfillment_rate_adjusted: f32,

    /// Total program cycles computed across all fulfilled requests in this period
    pub total_program_cycles: String,

    /// Total cycles (program + overhead) computed across all fulfilled requests in this period
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorAggregatesResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// The aggregation granularity used: hourly, daily, or weekly
    pub aggregation: AggregationGranularity,
    pub data: Vec<RequestorAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorCumulativeEntry {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    /// Timestamp for this cumulative snapshot (Unix timestamp)
    pub timestamp: i64,

    /// ISO 8601 formatted timestamp
    pub timestamp_iso: String,

    /// Total number of fulfilled orders (cumulative)
    pub total_fulfilled: i64,

    /// Unique provers who locked requests (cumulative)
    pub unique_provers_locking_requests: i64,

    /// Total fees locked (as string)
    pub total_fees_locked: String,

    /// Total fees locked (formatted for display)
    pub total_fees_locked_formatted: String,

    /// Total collateral locked (as string)
    pub total_collateral_locked: String,

    /// Total collateral locked (formatted for display)
    pub total_collateral_locked_formatted: String,

    /// Total collateral from locked requests that expired (as string)
    pub total_locked_and_expired_collateral: String,

    /// Total collateral from locked requests that expired (formatted for display)
    pub total_locked_and_expired_collateral_formatted: String,

    /// Total number of requests submitted (cumulative)
    pub total_requests_submitted: i64,

    /// Total number of requests submitted onchain (cumulative)
    pub total_requests_submitted_onchain: i64,

    /// Total number of requests submitted offchain (cumulative)
    pub total_requests_submitted_offchain: i64,

    /// Total number of requests locked (cumulative)
    pub total_requests_locked: i64,

    /// Total number of requests slashed (cumulative)
    pub total_requests_slashed: i64,

    /// Total number of requests that expired (cumulative)
    pub total_expired: i64,

    /// Total number of locked requests that expired (cumulative)
    pub total_locked_and_expired: i64,

    /// Total number of locked requests that were fulfilled (cumulative)
    pub total_locked_and_fulfilled: i64,

    /// Total number of secondary fulfillments (cumulative)
    pub total_secondary_fulfillments: i64,

    /// Fulfillment rate for locked orders (percentage)
    pub locked_orders_fulfillment_rate: f32,

    /// Adjusted fulfillment rate for locked orders, deduplicated by (input_data, image_url) (percentage)
    pub locked_orders_fulfillment_rate_adjusted: f32,

    /// Total program cycles computed across all fulfilled requests (cumulative)
    pub total_program_cycles: String,

    /// Total cycles (program + overhead) computed across all fulfilled requests (cumulative)
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestorCumulativesResponse {
    /// Chain ID
    pub chain_id: u64,
    /// Requestor address
    pub requestor_address: String,
    pub data: Vec<RequestorCumulativeEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema, utoipa::IntoParams, Clone)]
pub struct ProverAggregatesParams {
    #[serde(default)]
    aggregation: AggregationGranularity,

    #[serde(default)]
    cursor: Option<String>,

    #[serde(default)]
    limit: Option<u64>,

    #[serde(default)]
    sort: Option<String>,

    #[serde(default)]
    before: Option<i64>,

    #[serde(default)]
    after: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverAggregateEntry {
    pub chain_id: u64,
    pub prover_address: String,
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub total_requests_locked: i64,
    pub total_requests_fulfilled: i64,
    pub total_unique_requestors: i64,
    pub total_fees_earned: String,
    pub total_fees_earned_formatted: String,
    pub total_collateral_locked: String,
    pub total_collateral_locked_formatted: String,
    pub total_collateral_slashed: String,
    pub total_collateral_slashed_formatted: String,
    pub total_collateral_earned: String,
    pub total_collateral_earned_formatted: String,
    pub total_requests_locked_and_expired: i64,
    pub total_requests_locked_and_fulfilled: i64,
    pub locked_orders_fulfillment_rate: f32,
    pub p10_lock_price_per_cycle: String,
    pub p10_lock_price_per_cycle_formatted: String,
    pub p25_lock_price_per_cycle: String,
    pub p25_lock_price_per_cycle_formatted: String,
    pub p50_lock_price_per_cycle: String,
    pub p50_lock_price_per_cycle_formatted: String,
    pub p75_lock_price_per_cycle: String,
    pub p75_lock_price_per_cycle_formatted: String,
    pub p90_lock_price_per_cycle: String,
    pub p90_lock_price_per_cycle_formatted: String,
    pub p95_lock_price_per_cycle: String,
    pub p95_lock_price_per_cycle_formatted: String,
    pub p99_lock_price_per_cycle: String,
    pub p99_lock_price_per_cycle_formatted: String,
    pub total_program_cycles: String,
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverAggregatesResponse {
    pub chain_id: u64,
    pub prover_address: String,
    pub aggregation: AggregationGranularity,
    pub data: Vec<ProverAggregateEntry>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema, utoipa::IntoParams, Clone)]
pub struct ProverCumulativesParams {
    #[serde(default)]
    cursor: Option<String>,

    #[serde(default)]
    limit: Option<u64>,

    #[serde(default)]
    sort: Option<String>,

    #[serde(default)]
    before: Option<i64>,

    #[serde(default)]
    after: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverCumulativeEntry {
    pub chain_id: u64,
    pub prover_address: String,
    pub timestamp: i64,
    pub timestamp_iso: String,
    pub total_requests_locked: i64,
    pub total_requests_fulfilled: i64,
    pub total_unique_requestors: i64,
    pub total_fees_earned: String,
    pub total_fees_earned_formatted: String,
    pub total_collateral_locked: String,
    pub total_collateral_locked_formatted: String,
    pub total_collateral_slashed: String,
    pub total_collateral_slashed_formatted: String,
    pub total_collateral_earned: String,
    pub total_collateral_earned_formatted: String,
    pub total_requests_locked_and_expired: i64,
    pub total_requests_locked_and_fulfilled: i64,
    pub locked_orders_fulfillment_rate: f32,
    pub total_program_cycles: String,
    pub total_cycles: String,
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProverCumulativesResponse {
    pub chain_id: u64,
    pub prover_address: String,
    pub data: Vec<ProverCumulativeEntry>,
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
    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
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
        AggregationGranularity::Hourly => {
            state
                .market_db
                .get_hourly_market_summaries(
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Daily => {
            state
                .market_db
                .get_daily_market_summaries(
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Weekly => {
            state
                .market_db
                .get_weekly_market_summaries(
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Monthly => {
            state
                .market_db
                .get_monthly_market_summaries(
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
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

            // Convert U256 fields to strings (all currency fields are now U256 in struct)
            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_locked_and_expired_collateral =
                summary.total_locked_and_expired_collateral.to_string();
            let p10_lock_price_per_cycle = summary.p10_lock_price_per_cycle.to_string();
            let p25_lock_price_per_cycle = summary.p25_lock_price_per_cycle.to_string();
            let p50_lock_price_per_cycle = summary.p50_lock_price_per_cycle.to_string();
            let p75_lock_price_per_cycle = summary.p75_lock_price_per_cycle.to_string();
            let p90_lock_price_per_cycle = summary.p90_lock_price_per_cycle.to_string();
            let p95_lock_price_per_cycle = summary.p95_lock_price_per_cycle.to_string();
            let p99_lock_price_per_cycle = summary.p99_lock_price_per_cycle.to_string();

            MarketAggregateEntry {
                chain_id: state.chain_id,
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                unique_requesters_submitting_requests: summary.unique_requesters_submitting_requests
                    as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_locked_and_expired_collateral: total_locked_and_expired_collateral.clone(),
                total_locked_and_expired_collateral_formatted: format_zkc(
                    &total_locked_and_expired_collateral,
                ),
                p10_lock_price_per_cycle: p10_lock_price_per_cycle.clone(),
                p10_lock_price_per_cycle_formatted: format_eth(&p10_lock_price_per_cycle),
                p25_lock_price_per_cycle: p25_lock_price_per_cycle.clone(),
                p25_lock_price_per_cycle_formatted: format_eth(&p25_lock_price_per_cycle),
                p50_lock_price_per_cycle: p50_lock_price_per_cycle.clone(),
                p50_lock_price_per_cycle_formatted: format_eth(&p50_lock_price_per_cycle),
                p75_lock_price_per_cycle: p75_lock_price_per_cycle.clone(),
                p75_lock_price_per_cycle_formatted: format_eth(&p75_lock_price_per_cycle),
                p90_lock_price_per_cycle: p90_lock_price_per_cycle.clone(),
                p90_lock_price_per_cycle_formatted: format_eth(&p90_lock_price_per_cycle),
                p95_lock_price_per_cycle: p95_lock_price_per_cycle.clone(),
                p95_lock_price_per_cycle_formatted: format_eth(&p95_lock_price_per_cycle),
                p99_lock_price_per_cycle: p99_lock_price_per_cycle.clone(),
                p99_lock_price_per_cycle_formatted: format_eth(&p99_lock_price_per_cycle),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
                total_expired: summary.total_expired as i64,
                total_locked_and_expired: summary.total_locked_and_expired as i64,
                total_locked_and_fulfilled: summary.total_locked_and_fulfilled as i64,
                total_secondary_fulfillments: summary.total_secondary_fulfillments as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted: 0.0,
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(MarketAggregatesResponse {
        chain_id: state.chain_id,
        aggregation: params.aggregation,
        data,
        next_cursor,
        has_more,
    })
}

/// GET /v1/market/cumulatives
/// Returns all-time market statistics over time with pagination
#[utoipa::path(
    get,
    path = "/v1/market/cumulatives",
    tag = "Market",
    params(
        MarketCumulativesParams
    ),
    responses(
        (status = 200, description = "Market cumulatives", body = MarketCumulativesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_market_cumulatives(
    State(state): State<Arc<AppState>>,
    Query(params): Query<MarketCumulativesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_market_cumulatives_impl(state, params).await {
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

async fn get_market_cumulatives_impl(
    state: Arc<AppState>,
    params: MarketCumulativesParams,
) -> anyhow::Result<MarketCumulativesResponse> {
    tracing::debug!(
        "Fetching market cumulatives: cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    // Parse cursor if provided
    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
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

    // Fetch all-time market summaries
    let mut summaries = state
        .market_db
        .get_all_time_market_summaries(cursor, limit_plus_one, sort, params.before, params.after)
        .await?;

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

            // Convert U256 fields to strings (all currency fields are now U256 in struct)
            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_locked_and_expired_collateral =
                summary.total_locked_and_expired_collateral.to_string();

            MarketCumulativeEntry {
                chain_id: state.chain_id,
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                unique_requesters_submitting_requests: summary.unique_requesters_submitting_requests
                    as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_locked_and_expired_collateral: total_locked_and_expired_collateral.clone(),
                total_locked_and_expired_collateral_formatted: format_zkc(
                    &total_locked_and_expired_collateral,
                ),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
                total_expired: summary.total_expired as i64,
                total_locked_and_expired: summary.total_locked_and_expired as i64,
                total_locked_and_fulfilled: summary.total_locked_and_fulfilled as i64,
                total_secondary_fulfillments: summary.total_secondary_fulfillments as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted: 0.0,
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(MarketCumulativesResponse { chain_id: state.chain_id, data, next_cursor, has_more })
}

/// GET /v1/market/requestors/:address/aggregates
/// Returns aggregated requestor data for the specified time period
#[utoipa::path(
    get,
    path = "/v1/market/requestors/{address}/aggregates",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Requestor address"),
        RequestorAggregatesParams
    ),
    responses(
        (status = 200, description = "Requestor aggregates", body = RequestorAggregatesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_requestor_aggregates(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<RequestorAggregatesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_requestor_aggregates_impl(state, address, params).await {
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

async fn get_requestor_aggregates_impl(
    state: Arc<AppState>,
    address: String,
    params: RequestorAggregatesParams,
) -> anyhow::Result<RequestorAggregatesResponse> {
    let requestor_address = Address::from_str(&address)?;

    // Validate aggregation is not monthly
    if matches!(params.aggregation, AggregationGranularity::Monthly) {
        anyhow::bail!("Monthly aggregation is not supported for requestor aggregates");
    }

    tracing::debug!(
        "Fetching requestor aggregates: address={}, aggregation={}, cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        address,
        params.aggregation,
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    // Parse cursor if provided
    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
    let limit = if limit > MAX_AGGREGATES { MAX_AGGREGATES } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    // Parse sort direction
    let sort = match params.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => anyhow::bail!("Invalid sort direction. Must be 'asc' or 'desc'"),
    };

    // Request one extra item to efficiently determine if more pages exist
    let limit_plus_one = limit_i64 + 1;

    // Route to appropriate database method based on aggregation type
    let mut summaries = match params.aggregation {
        AggregationGranularity::Hourly => {
            state
                .market_db
                .get_hourly_requestor_summaries(
                    requestor_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Daily => {
            state
                .market_db
                .get_daily_requestor_summaries(
                    requestor_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Weekly => {
            state
                .market_db
                .get_weekly_requestor_summaries(
                    requestor_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Monthly => {
            // This should never happen due to validation above, but include for completeness
            anyhow::bail!("Monthly aggregation is not supported");
        }
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
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_locked_and_expired_collateral =
                summary.total_locked_and_expired_collateral.to_string();
            let p10_lock_price_per_cycle = summary.p10_lock_price_per_cycle.to_string();
            let p25_lock_price_per_cycle = summary.p25_lock_price_per_cycle.to_string();
            let p50_lock_price_per_cycle = summary.p50_lock_price_per_cycle.to_string();
            let p75_lock_price_per_cycle = summary.p75_lock_price_per_cycle.to_string();
            let p90_lock_price_per_cycle = summary.p90_lock_price_per_cycle.to_string();
            let p95_lock_price_per_cycle = summary.p95_lock_price_per_cycle.to_string();
            let p99_lock_price_per_cycle = summary.p99_lock_price_per_cycle.to_string();

            RequestorAggregateEntry {
                chain_id: state.chain_id,
                requestor_address: format!("{:#x}", summary.requestor_address),
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_locked_and_expired_collateral: total_locked_and_expired_collateral.clone(),
                total_locked_and_expired_collateral_formatted: format_zkc(
                    &total_locked_and_expired_collateral,
                ),
                p10_lock_price_per_cycle: p10_lock_price_per_cycle.clone(),
                p10_lock_price_per_cycle_formatted: format_eth(&p10_lock_price_per_cycle),
                p25_lock_price_per_cycle: p25_lock_price_per_cycle.clone(),
                p25_lock_price_per_cycle_formatted: format_eth(&p25_lock_price_per_cycle),
                p50_lock_price_per_cycle: p50_lock_price_per_cycle.clone(),
                p50_lock_price_per_cycle_formatted: format_eth(&p50_lock_price_per_cycle),
                p75_lock_price_per_cycle: p75_lock_price_per_cycle.clone(),
                p75_lock_price_per_cycle_formatted: format_eth(&p75_lock_price_per_cycle),
                p90_lock_price_per_cycle: p90_lock_price_per_cycle.clone(),
                p90_lock_price_per_cycle_formatted: format_eth(&p90_lock_price_per_cycle),
                p95_lock_price_per_cycle: p95_lock_price_per_cycle.clone(),
                p95_lock_price_per_cycle_formatted: format_eth(&p95_lock_price_per_cycle),
                p99_lock_price_per_cycle: p99_lock_price_per_cycle.clone(),
                p99_lock_price_per_cycle_formatted: format_eth(&p99_lock_price_per_cycle),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
                total_expired: summary.total_expired as i64,
                total_locked_and_expired: summary.total_locked_and_expired as i64,
                total_locked_and_fulfilled: summary.total_locked_and_fulfilled as i64,
                total_secondary_fulfillments: summary.total_secondary_fulfillments as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted: summary
                    .locked_orders_fulfillment_rate_adjusted,
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(RequestorAggregatesResponse {
        chain_id: state.chain_id,
        requestor_address: format!("{:#x}", requestor_address),
        aggregation: params.aggregation,
        data,
        next_cursor,
        has_more,
    })
}

/// GET /v1/market/requestors/:address/cumulatives
/// Returns all-time requestor statistics over time with pagination
#[utoipa::path(
    get,
    path = "/v1/market/requestors/{address}/cumulatives",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Requestor address"),
        RequestorCumulativesParams
    ),
    responses(
        (status = 200, description = "Requestor cumulatives", body = RequestorCumulativesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_requestor_cumulatives(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<RequestorCumulativesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_requestor_cumulatives_impl(state, address, params).await {
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

async fn get_requestor_cumulatives_impl(
    state: Arc<AppState>,
    address: String,
    params: RequestorCumulativesParams,
) -> anyhow::Result<RequestorCumulativesResponse> {
    let requestor_address = Address::from_str(&address)?;

    tracing::debug!(
        "Fetching requestor cumulatives: address={}, cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        address,
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    // Parse cursor if provided
    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
    let limit = if limit > MAX_AGGREGATES { MAX_AGGREGATES } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    // Parse sort direction
    let sort = match params.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => anyhow::bail!("Invalid sort direction. Must be 'asc' or 'desc'"),
    };

    // Request one extra item to efficiently determine if more pages exist
    let limit_plus_one = limit_i64 + 1;

    // Fetch all-time requestor summaries
    let mut summaries = state
        .market_db
        .get_all_time_requestor_summaries(
            requestor_address,
            cursor,
            limit_plus_one,
            sort,
            params.before,
            params.after,
        )
        .await?;

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
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_locked_and_expired_collateral =
                summary.total_locked_and_expired_collateral.to_string();

            RequestorCumulativeEntry {
                chain_id: state.chain_id,
                requestor_address: format!("{:#x}", summary.requestor_address),
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_locked_and_expired_collateral: total_locked_and_expired_collateral.clone(),
                total_locked_and_expired_collateral_formatted: format_zkc(
                    &total_locked_and_expired_collateral,
                ),
                total_requests_submitted: summary.total_requests_submitted as i64,
                total_requests_submitted_onchain: summary.total_requests_submitted_onchain as i64,
                total_requests_submitted_offchain: summary.total_requests_submitted_offchain as i64,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_slashed: summary.total_requests_slashed as i64,
                total_expired: summary.total_expired as i64,
                total_locked_and_expired: summary.total_locked_and_expired as i64,
                total_locked_and_fulfilled: summary.total_locked_and_fulfilled as i64,
                total_secondary_fulfillments: summary.total_secondary_fulfillments as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted: summary
                    .locked_orders_fulfillment_rate_adjusted,
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(RequestorCumulativesResponse {
        chain_id: state.chain_id,
        requestor_address: format!("{:#x}", requestor_address),
        data,
        next_cursor,
        has_more,
    })
}

/// GET /v1/market/provers/:address/aggregates
/// Returns aggregated prover data for the specified time period
#[utoipa::path(
    get,
    path = "/v1/market/provers/{address}/aggregates",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Prover address"),
        ProverAggregatesParams
    ),
    responses(
        (status = 200, description = "Prover aggregates", body = ProverAggregatesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_prover_aggregates(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<ProverAggregatesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_prover_aggregates_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let is_recent = params_clone.before.is_none_or(|before| before > now - 86400);
            let cache_duration =
                if is_recent { "public, max-age=60" } else { "public, max-age=300" };
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control(cache_duration));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_prover_aggregates_impl(
    state: Arc<AppState>,
    address: String,
    params: ProverAggregatesParams,
) -> anyhow::Result<ProverAggregatesResponse> {
    let prover_address = Address::from_str(&address)?;

    if matches!(params.aggregation, AggregationGranularity::Monthly) {
        anyhow::bail!("Monthly aggregation is not supported for prover aggregates");
    }

    tracing::debug!(
        "Fetching prover aggregates: address={}, aggregation={}, cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        address,
        params.aggregation,
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
    let limit = if limit > MAX_AGGREGATES { MAX_AGGREGATES } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    let sort = match params.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => anyhow::bail!("Invalid sort direction. Must be 'asc' or 'desc'"),
    };

    let limit_plus_one = limit_i64 + 1;

    let mut summaries = match params.aggregation {
        AggregationGranularity::Hourly => {
            state
                .market_db
                .get_hourly_prover_summaries(
                    prover_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Daily => {
            state
                .market_db
                .get_daily_prover_summaries(
                    prover_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Weekly => {
            state
                .market_db
                .get_weekly_prover_summaries(
                    prover_address,
                    cursor,
                    limit_plus_one,
                    sort,
                    params.before,
                    params.after,
                )
                .await?
        }
        AggregationGranularity::Monthly => {
            anyhow::bail!("Monthly aggregation is not supported");
        }
    };

    let has_more = summaries.len() > limit as usize;
    if has_more {
        summaries.pop();
    }

    let next_cursor = if has_more && !summaries.is_empty() {
        let last_summary = summaries.last().unwrap();
        Some(encode_cursor(last_summary.period_timestamp as i64)?)
    } else {
        None
    };

    let data = summaries
        .into_iter()
        .map(|summary| {
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            let total_fees_earned = summary.total_fees_earned.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_collateral_slashed = summary.total_collateral_slashed.to_string();
            let total_collateral_earned = summary.total_collateral_earned.to_string();
            let p10_lock_price_per_cycle = summary.p10_lock_price_per_cycle.to_string();
            let p25_lock_price_per_cycle = summary.p25_lock_price_per_cycle.to_string();
            let p50_lock_price_per_cycle = summary.p50_lock_price_per_cycle.to_string();
            let p75_lock_price_per_cycle = summary.p75_lock_price_per_cycle.to_string();
            let p90_lock_price_per_cycle = summary.p90_lock_price_per_cycle.to_string();
            let p95_lock_price_per_cycle = summary.p95_lock_price_per_cycle.to_string();
            let p99_lock_price_per_cycle = summary.p99_lock_price_per_cycle.to_string();

            ProverAggregateEntry {
                chain_id: state.chain_id,
                prover_address: format!("{:#x}", summary.prover_address),
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_fulfilled: summary.total_requests_fulfilled as i64,
                total_unique_requestors: summary.total_unique_requestors as i64,
                total_fees_earned: total_fees_earned.clone(),
                total_fees_earned_formatted: format_eth(&total_fees_earned),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_collateral_slashed: total_collateral_slashed.clone(),
                total_collateral_slashed_formatted: format_zkc(&total_collateral_slashed),
                total_collateral_earned: total_collateral_earned.clone(),
                total_collateral_earned_formatted: format_zkc(&total_collateral_earned),
                total_requests_locked_and_expired: summary.total_requests_locked_and_expired as i64,
                total_requests_locked_and_fulfilled: summary.total_requests_locked_and_fulfilled
                    as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                p10_lock_price_per_cycle: p10_lock_price_per_cycle.clone(),
                p10_lock_price_per_cycle_formatted: format_eth(&p10_lock_price_per_cycle),
                p25_lock_price_per_cycle: p25_lock_price_per_cycle.clone(),
                p25_lock_price_per_cycle_formatted: format_eth(&p25_lock_price_per_cycle),
                p50_lock_price_per_cycle: p50_lock_price_per_cycle.clone(),
                p50_lock_price_per_cycle_formatted: format_eth(&p50_lock_price_per_cycle),
                p75_lock_price_per_cycle: p75_lock_price_per_cycle.clone(),
                p75_lock_price_per_cycle_formatted: format_eth(&p75_lock_price_per_cycle),
                p90_lock_price_per_cycle: p90_lock_price_per_cycle.clone(),
                p90_lock_price_per_cycle_formatted: format_eth(&p90_lock_price_per_cycle),
                p95_lock_price_per_cycle: p95_lock_price_per_cycle.clone(),
                p95_lock_price_per_cycle_formatted: format_eth(&p95_lock_price_per_cycle),
                p99_lock_price_per_cycle: p99_lock_price_per_cycle.clone(),
                p99_lock_price_per_cycle_formatted: format_eth(&p99_lock_price_per_cycle),
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(ProverAggregatesResponse {
        chain_id: state.chain_id,
        prover_address: format!("{:#x}", prover_address),
        aggregation: params.aggregation,
        data,
        next_cursor,
        has_more,
    })
}

/// GET /v1/market/provers/:address/cumulatives
/// Returns all-time prover statistics over time with pagination
#[utoipa::path(
    get,
    path = "/v1/market/provers/{address}/cumulatives",
    tag = "Market",
    params(
        ("address" = String, Path, description = "Prover address"),
        ProverCumulativesParams
    ),
    responses(
        (status = 200, description = "Prover cumulatives", body = ProverCumulativesResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn get_prover_cumulatives(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<ProverCumulativesParams>,
) -> Response {
    let params_clone = params.clone();
    match get_prover_cumulatives_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let is_recent = params_clone.before.is_none_or(|before| before > now - 86400);
            let cache_duration =
                if is_recent { "public, max-age=60" } else { "public, max-age=300" };
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control(cache_duration));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_prover_cumulatives_impl(
    state: Arc<AppState>,
    address: String,
    params: ProverCumulativesParams,
) -> anyhow::Result<ProverCumulativesResponse> {
    let prover_address = Address::from_str(&address)?;

    tracing::debug!(
        "Fetching prover cumulatives: address={}, cursor={:?}, limit={:?}, sort={:?}, before={:?}, after={:?}",
        address,
        params.cursor,
        params.limit,
        params.sort,
        params.before,
        params.after
    );

    let cursor =
        if let Some(cursor_str) = &params.cursor { Some(decode_cursor(cursor_str)?) } else { None };

    let limit = params.limit.unwrap_or(DEFAULT_AGGREGATES_LIMIT);
    let limit = if limit > MAX_AGGREGATES { MAX_AGGREGATES } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    let sort = match params.sort.as_deref() {
        Some("asc") => SortDirection::Asc,
        Some("desc") | None => SortDirection::Desc,
        _ => anyhow::bail!("Invalid sort direction. Must be 'asc' or 'desc'"),
    };

    let limit_plus_one = limit_i64 + 1;

    let mut summaries = state
        .market_db
        .get_all_time_prover_summaries(
            prover_address,
            cursor,
            limit_plus_one,
            sort,
            params.before,
            params.after,
        )
        .await?;

    let has_more = summaries.len() > limit as usize;
    if has_more {
        summaries.pop();
    }

    let next_cursor = if has_more && !summaries.is_empty() {
        let last_summary = summaries.last().unwrap();
        Some(encode_cursor(last_summary.period_timestamp as i64)?)
    } else {
        None
    };

    let data = summaries
        .into_iter()
        .map(|summary| {
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            let total_fees_earned = summary.total_fees_earned.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_collateral_slashed = summary.total_collateral_slashed.to_string();
            let total_collateral_earned = summary.total_collateral_earned.to_string();

            ProverCumulativeEntry {
                chain_id: state.chain_id,
                prover_address: format!("{:#x}", summary.prover_address),
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_requests_locked: summary.total_requests_locked as i64,
                total_requests_fulfilled: summary.total_requests_fulfilled as i64,
                total_unique_requestors: summary.total_unique_requestors as i64,
                total_fees_earned: total_fees_earned.clone(),
                total_fees_earned_formatted: format_eth(&total_fees_earned),
                total_collateral_locked: total_collateral_locked.clone(),
                total_collateral_locked_formatted: format_zkc(&total_collateral_locked),
                total_collateral_slashed: total_collateral_slashed.clone(),
                total_collateral_slashed_formatted: format_zkc(&total_collateral_slashed),
                total_collateral_earned: total_collateral_earned.clone(),
                total_collateral_earned_formatted: format_zkc(&total_collateral_earned),
                total_requests_locked_and_expired: summary.total_requests_locked_and_expired as i64,
                total_requests_locked_and_fulfilled: summary.total_requests_locked_and_fulfilled
                    as i64,
                locked_orders_fulfillment_rate: summary.locked_orders_fulfillment_rate,
                total_program_cycles: summary.total_program_cycles.to_string(),
                total_cycles: summary.total_cycles.to_string(),
            }
        })
        .collect();

    Ok(ProverCumulativesResponse {
        chain_id: state.chain_id,
        prover_address: format!("{:#x}", prover_address),
        data,
        next_cursor,
        has_more,
    })
}

/// Format Unix timestamp as ISO 8601 string (UTC)
fn format_timestamp_iso(timestamp: i64) -> String {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

#[derive(Debug, Clone, Deserialize, utoipa::IntoParams, utoipa::ToSchema)]
pub struct RequestListParams {
    /// Base64-encoded cursor from previous response for pagination
    #[serde(default)]
    cursor: Option<String>,

    /// Limit of requests returned, max 500 (default 50)
    #[serde(default)]
    limit: Option<u32>,

    /// Sort field: "updated_at" or "created_at" (default "created_at")
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

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestStatusResponse {
    /// Chain ID
    pub chain_id: u64,
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
    /// Created timestamp (ISO 8601)
    pub created_at_iso: String,
    /// Last updated timestamp (Unix)
    pub updated_at: i64,
    /// Last updated timestamp (ISO 8601)
    pub updated_at_iso: String,
    /// Locked timestamp (Unix)
    pub locked_at: Option<i64>,
    /// Locked timestamp (ISO 8601)
    pub locked_at_iso: Option<String>,
    /// Fulfilled timestamp (Unix)
    pub fulfilled_at: Option<i64>,
    /// Fulfilled timestamp (ISO 8601)
    pub fulfilled_at_iso: Option<String>,
    /// Slashed timestamp (Unix)
    pub slashed_at: Option<i64>,
    /// Slashed timestamp (ISO 8601)
    pub slashed_at_iso: Option<String>,
    /// Lock prover delivered proof timestamp (Unix)
    pub lock_prover_delivered_proof_at: Option<i64>,
    /// Lock prover delivered proof timestamp (ISO 8601)
    pub lock_prover_delivered_proof_at_iso: Option<String>,
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
    /// Minimum price (formatted)
    pub min_price_formatted: String,
    /// Maximum price (wei)
    pub max_price: String,
    /// Maximum price (formatted)
    pub max_price_formatted: String,
    /// Lock collateral (wei)
    pub lock_collateral: String,
    /// Lock collateral (formatted)
    pub lock_collateral_formatted: String,
    /// Lock price (wei)
    pub lock_price: Option<String>,
    /// Lock price (formatted)
    pub lock_price_formatted: Option<String>,
    /// Lock price per cycle (wei)
    pub lock_price_per_cycle: Option<String>,
    /// Lock price per cycle (formatted)
    pub lock_price_per_cycle_formatted: Option<String>,
    /// Ramp up start timestamp
    pub ramp_up_start: i64,
    /// Ramp up start timestamp (ISO 8601)
    pub ramp_up_start_iso: String,
    /// Ramp up period (seconds)
    pub ramp_up_period: i64,
    /// Expires at timestamp
    pub expires_at: i64,
    /// Expires at timestamp (ISO 8601)
    pub expires_at_iso: String,
    /// Lock end timestamp
    pub lock_end: i64,
    /// Lock end timestamp (ISO 8601)
    pub lock_end_iso: String,
    /// Slash recipient address
    pub slash_recipient: Option<String>,
    /// Slash transferred amount
    pub slash_transferred_amount: Option<String>,
    /// Slash transferred amount (formatted)
    pub slash_transferred_amount_formatted: Option<String>,
    /// Slash burned amount
    pub slash_burned_amount: Option<String>,
    /// Slash burned amount (formatted)
    pub slash_burned_amount_formatted: Option<String>,
    /// Program cycles (guest program only)
    pub program_cycles: Option<String>,
    /// Total cycles (program + overhead)
    pub total_cycles: Option<String>,
    /// Effective prove MHz (from requestor perspective: fulfilled_at - created_at)
    pub effective_prove_mhz: Option<f64>,
    /// Prover effective prove MHz (from prover perspective: fulfilled_at - locked_at or lock_end)
    pub prover_effective_prove_mhz: Option<f64>,
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

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RequestListResponse {
    /// Chain ID
    pub chain_id: u64,
    pub data: Vec<RequestStatusResponse>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
}

fn convert_request_status(status: RequestStatus, chain_id: u64) -> RequestStatusResponse {
    let created_at = status.created_at as i64;
    let updated_at = status.updated_at as i64;
    let locked_at = status.locked_at.map(|t| t as i64);
    let fulfilled_at = status.fulfilled_at.map(|t| t as i64);
    let slashed_at = status.slashed_at.map(|t| t as i64);
    let lock_prover_delivered_proof_at = status.lock_prover_delivered_proof_at.map(|t| t as i64);
    let ramp_up_start = status.ramp_up_start as i64;
    let expires_at = status.expires_at as i64;
    let lock_end = status.lock_end as i64;

    RequestStatusResponse {
        chain_id,
        request_digest: format!("{:#x}", status.request_digest),
        request_id: format!("0x{:x}", status.request_id),
        request_status: status.request_status.to_string(),
        source: status.source,
        client_address: format!("{:#x}", status.client_address),
        lock_prover_address: status.lock_prover_address.map(|addr| format!("{:#x}", addr)),
        fulfill_prover_address: status.fulfill_prover_address.map(|addr| format!("{:#x}", addr)),
        created_at,
        created_at_iso: format_timestamp_iso(created_at),
        updated_at,
        updated_at_iso: format_timestamp_iso(updated_at),
        locked_at,
        locked_at_iso: locked_at.map(format_timestamp_iso),
        fulfilled_at,
        fulfilled_at_iso: fulfilled_at.map(format_timestamp_iso),
        slashed_at,
        slashed_at_iso: slashed_at.map(format_timestamp_iso),
        lock_prover_delivered_proof_at,
        lock_prover_delivered_proof_at_iso: lock_prover_delivered_proof_at
            .map(format_timestamp_iso),
        submit_block: status.submit_block.map(|b| b as i64),
        lock_block: status.lock_block.map(|b| b as i64),
        fulfill_block: status.fulfill_block.map(|b| b as i64),
        slashed_block: status.slashed_block.map(|b| b as i64),
        min_price: status.min_price.clone(),
        min_price_formatted: format_eth(&status.min_price),
        max_price: status.max_price.clone(),
        max_price_formatted: format_eth(&status.max_price),
        lock_collateral: status.lock_collateral.clone(),
        lock_collateral_formatted: format_zkc(&status.lock_collateral),
        lock_price: status.lock_price.clone(),
        lock_price_formatted: status.lock_price.as_ref().map(|p| format_eth(p)),
        lock_price_per_cycle: status.lock_price_per_cycle.clone(),
        lock_price_per_cycle_formatted: status.lock_price_per_cycle.as_ref().map(|p| format_eth(p)),
        ramp_up_start,
        ramp_up_start_iso: format_timestamp_iso(ramp_up_start),
        ramp_up_period: status.ramp_up_period as i64,
        expires_at,
        expires_at_iso: format_timestamp_iso(expires_at),
        lock_end,
        lock_end_iso: format_timestamp_iso(lock_end),
        slash_recipient: status.slash_recipient.map(|addr| format!("{:#x}", addr)),
        slash_transferred_amount: status.slash_transferred_amount.clone(),
        slash_transferred_amount_formatted: status
            .slash_transferred_amount
            .as_ref()
            .map(|a| format_zkc(a)),
        slash_burned_amount: status.slash_burned_amount.clone(),
        slash_burned_amount_formatted: status.slash_burned_amount.as_ref().map(|a| format_zkc(a)),
        program_cycles: status.program_cycles.as_ref().map(|c| c.to_string()),
        total_cycles: status.total_cycles.as_ref().map(|c| c.to_string()),
        effective_prove_mhz: status.effective_prove_mhz,
        prover_effective_prove_mhz: status.prover_effective_prove_mhz,
        submit_tx_hash: status.submit_tx_hash.map(|h| format!("{:#x}", h)),
        lock_tx_hash: status.lock_tx_hash.map(|h| format!("{:#x}", h)),
        fulfill_tx_hash: status.fulfill_tx_hash.map(|h| format!("{:#x}", h)),
        slash_tx_hash: status.slash_tx_hash.map(|h| format!("{:#x}", h)),
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
        Some("updated_at") => RequestSortField::UpdatedAt,
        Some("created_at") | None => RequestSortField::CreatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) = state.market_db.list_requests(cursor, limit, sort_by).await?;

    let data =
        statuses.into_iter().map(|s| convert_request_status(s, state.chain_id)).collect::<Vec<_>>();
    let next_cursor_encoded = next_cursor.as_ref().map(encode_request_cursor).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        chain_id: state.chain_id,
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
        Some("updated_at") => RequestSortField::UpdatedAt,
        Some("created_at") | None => RequestSortField::CreatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) =
        state.market_db.list_requests_by_requestor(client_address, cursor, limit, sort_by).await?;

    let data =
        statuses.into_iter().map(|s| convert_request_status(s, state.chain_id)).collect::<Vec<_>>();
    let next_cursor_encoded = next_cursor.as_ref().map(encode_request_cursor).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        chain_id: state.chain_id,
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
        Some("updated_at") => RequestSortField::UpdatedAt,
        Some("created_at") | None => RequestSortField::CreatedAt,
        _ => anyhow::bail!("Invalid sort_by. Must be 'updated_at' or 'created_at'"),
    };

    let (statuses, next_cursor) =
        state.market_db.list_requests_by_prover(prover_address, cursor, limit, sort_by).await?;

    let data =
        statuses.into_iter().map(|s| convert_request_status(s, state.chain_id)).collect::<Vec<_>>();
    let next_cursor_encoded = next_cursor.as_ref().map(encode_request_cursor).transpose()?;
    let has_more = next_cursor.is_some();

    Ok(RequestListResponse {
        chain_id: state.chain_id,
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
    // Parse as U256 hex string, supporting both with and without 0x prefix
    let request_id_hex = request_id.strip_prefix("0x").unwrap_or(&request_id);
    let parsed = U256::from_str_radix(request_id_hex, 16)
        .map_err(|e| anyhow::anyhow!("Invalid request_id format: {}", e))?;

    // Convert to hex string (without 0x) for database query (matches DB storage format)
    let normalized_id = format!("{:x}", parsed);

    let statuses = state.market_db.get_requests_by_request_id(&normalized_id).await?;
    let data =
        statuses.into_iter().map(|s| convert_request_status(s, state.chain_id)).collect::<Vec<_>>();
    Ok(data)
}

const MAX_LEADERBOARD: u64 = 100;
const DEFAULT_LEADERBOARD_LIMIT: u64 = 50;

/// GET /v1/market/requestors
/// Returns a paginated leaderboard of requestors with aggregated stats for the specified time period
#[utoipa::path(
    get,
    path = "/v1/market/requestors",
    tag = "Market",
    params(RequestorLeaderboardParams),
    responses(
        (status = 200, description = "Requestor leaderboard", body = RequestorLeaderboardResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Internal server error")
    )
)]
pub async fn list_requestors(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RequestorLeaderboardParams>,
) -> Response {
    let period = params.period;
    match list_requestors_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Use shorter cache for recent periods, longer for all-time
            let cache_duration = match period {
                LeaderboardPeriod::OneHour => "public, max-age=60",
                LeaderboardPeriod::OneDay => "public, max-age=120",
                _ => "public, max-age=300",
            };
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control(cache_duration));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_requestors_impl(
    state: Arc<AppState>,
    params: RequestorLeaderboardParams,
) -> anyhow::Result<RequestorLeaderboardResponse> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Calculate time range based on period
    let (start_ts, end_ts, use_hourly_table) = match params.period {
        LeaderboardPeriod::OneHour => {
            let hour_start = (now / 3600) * 3600;
            (hour_start, hour_start + 3600, true)
        }
        LeaderboardPeriod::OneDay => {
            let day_start = (now / 86400) * 86400;
            (day_start, day_start + 86400, true)
        }
        LeaderboardPeriod::ThreeDays => {
            let day_start = (now / 86400) * 86400;
            (day_start - 2 * 86400, day_start + 86400, false)
        }
        LeaderboardPeriod::SevenDays => {
            let day_start = (now / 86400) * 86400;
            (day_start - 6 * 86400, day_start + 86400, false)
        }
        LeaderboardPeriod::AllTime => (0, now + 86400, false),
    };

    // Parse cursor if provided
    let (cursor_orders, cursor_address) = if let Some(cursor_str) = &params.cursor {
        let cursor = decode_requestor_leaderboard_cursor(cursor_str)?;
        let address = Address::from_str(&cursor.address)
            .map_err(|e| anyhow::anyhow!("Invalid cursor address: {}", e))?;
        (Some(cursor.orders_requested), Some(address))
    } else {
        (None, None)
    };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_LEADERBOARD_LIMIT);
    let limit = if limit > MAX_LEADERBOARD { MAX_LEADERBOARD } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    // Request one extra item to determine if more pages exist
    let limit_plus_one = limit_i64 + 1;

    // Fetch leaderboard from DB (sorted by orders_requested DESC)
    let mut entries = state
        .market_db
        .get_requestor_leaderboard(
            start_ts,
            end_ts,
            use_hourly_table,
            cursor_orders,
            cursor_address,
            limit_plus_one,
        )
        .await?;

    let has_more = entries.len() > limit as usize;
    if has_more {
        entries.pop();
    }

    // Get addresses for batch queries
    let addresses: Vec<Address> = entries.iter().map(|e| e.requestor_address).collect();

    // Fetch median lock prices and last activity times in batch
    let median_prices =
        state.market_db.get_requestor_median_lock_prices(&addresses, start_ts, end_ts).await?;
    let last_activities = state.market_db.get_requestor_last_activity_times(&addresses).await?;

    // Build response entries
    let data: Vec<RequestorLeaderboardEntry> = entries
        .into_iter()
        .map(|entry| {
            let median = median_prices.get(&entry.requestor_address).cloned();
            let last_activity = last_activities
                .get(&entry.requestor_address)
                .cloned()
                .unwrap_or(entry.last_activity_time);

            let last_activity_iso = DateTime::<Utc>::from_timestamp(last_activity as i64, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_default();

            RequestorLeaderboardEntry {
                chain_id: state.chain_id,
                requestor_address: format!("{:#x}", entry.requestor_address),
                orders_requested: entry.orders_requested,
                orders_locked: entry.orders_locked,
                cycles_requested: entry.cycles_requested.to_string(),
                cycles_requested_formatted: format_cycles(entry.cycles_requested),
                median_lock_price_per_cycle: median.map(|m| m.to_string()),
                median_lock_price_per_cycle_formatted: median.map(|m| format_eth(&m.to_string())),
                acceptance_rate: entry.acceptance_rate,
                locked_order_fulfillment_rate: entry.locked_order_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted: entry
                    .locked_orders_fulfillment_rate_adjusted,
                last_activity_time: last_activity as i64,
                last_activity_time_iso: last_activity_iso,
            }
        })
        .collect();

    // Generate next cursor if there are more results
    let next_cursor = if has_more && !data.is_empty() {
        let last = data.last().unwrap();
        Some(encode_requestor_leaderboard_cursor(&RequestorLeaderboardCursor {
            orders_requested: last.orders_requested,
            address: last.requestor_address.clone(),
        })?)
    } else {
        None
    };

    Ok(RequestorLeaderboardResponse {
        chain_id: state.chain_id,
        period: params.period.to_string(),
        period_start: start_ts as i64,
        period_end: end_ts as i64,
        data,
        next_cursor,
        has_more,
    })
}

fn format_cycles(cycles: U256) -> String {
    // Format as billions/millions/thousands with appropriate suffix
    let value = cycles.to_string();
    if let Ok(num) = value.parse::<u128>() {
        if num >= 1_000_000_000_000 {
            format!("{:.2}T", num as f64 / 1_000_000_000_000.0)
        } else if num >= 1_000_000_000 {
            format!("{:.2}B", num as f64 / 1_000_000_000.0)
        } else if num >= 1_000_000 {
            format!("{:.2}M", num as f64 / 1_000_000.0)
        } else if num >= 1_000 {
            format!("{:.2}K", num as f64 / 1_000.0)
        } else {
            value
        }
    } else {
        value
    }
}

#[utoipa::path(
    get,
    path = "/v1/market/provers",
    params(
        ("period" = Option<LeaderboardPeriod>, Query, description = "Time period for leaderboard"),
        ("cursor" = Option<String>, Query, description = "Pagination cursor"),
        ("limit" = Option<u64>, Query, description = "Max results per page (max 100)")
    ),
    responses(
        (status = 200, description = "Prover leaderboard", body = ProverLeaderboardResponse),
        (status = 500, description = "Internal Server Error")
    ),
    tag = "Market"
)]
pub async fn list_provers(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RequestorLeaderboardParams>,
) -> Response {
    let period = params.period;
    match list_provers_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            let cache_duration = match period {
                LeaderboardPeriod::OneHour => "public, max-age=60",
                LeaderboardPeriod::OneDay => "public, max-age=120",
                _ => "public, max-age=300",
            };
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control(cache_duration));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_provers_impl(
    state: Arc<AppState>,
    params: RequestorLeaderboardParams,
) -> anyhow::Result<ProverLeaderboardResponse> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Calculate time range based on period
    let (start_ts, end_ts, use_hourly_table) = match params.period {
        LeaderboardPeriod::OneHour => {
            let hour_start = (now / 3600) * 3600;
            (hour_start, hour_start + 3600, true)
        }
        LeaderboardPeriod::OneDay => {
            let day_start = (now / 86400) * 86400;
            (day_start, day_start + 86400, true)
        }
        LeaderboardPeriod::ThreeDays => {
            let day_start = (now / 86400) * 86400;
            (day_start - 2 * 86400, day_start + 86400, false)
        }
        LeaderboardPeriod::SevenDays => {
            let day_start = (now / 86400) * 86400;
            (day_start - 6 * 86400, day_start + 86400, false)
        }
        LeaderboardPeriod::AllTime => (0, now + 86400, false),
    };

    // Parse cursor if provided
    let (cursor_fees, cursor_address) = if let Some(cursor_str) = &params.cursor {
        let cursor = decode_prover_leaderboard_cursor(cursor_str)?;
        let address = Address::from_str(&cursor.address)
            .map_err(|e| anyhow::anyhow!("Invalid cursor address: {}", e))?;
        let fees = U256::from_str(&cursor.fees_earned)
            .map_err(|e| anyhow::anyhow!("Invalid cursor fees: {}", e))?;
        (Some(fees), Some(address))
    } else {
        (None, None)
    };

    // Apply limit with max and default
    let limit = params.limit.unwrap_or(DEFAULT_LEADERBOARD_LIMIT);
    let limit = if limit > MAX_LEADERBOARD { MAX_LEADERBOARD } else { limit };
    let limit_i64 = i64::try_from(limit)?;

    // Request one extra item to determine if more pages exist
    let limit_plus_one = limit_i64 + 1;

    // Fetch leaderboard from DB (sorted by fees_earned DESC)
    let mut entries = state
        .market_db
        .get_prover_leaderboard(
            start_ts,
            end_ts,
            use_hourly_table,
            cursor_fees,
            cursor_address,
            limit_plus_one,
        )
        .await?;

    let has_more = entries.len() > limit as usize;
    if has_more {
        entries.pop();
    }

    // Get addresses for batch queries
    let addresses: Vec<Address> = entries.iter().map(|e| e.prover_address).collect();

    // Fetch median lock prices and last activity times in batch
    let median_prices =
        state.market_db.get_prover_median_lock_prices(&addresses, start_ts, end_ts).await?;
    let last_activities = state.market_db.get_prover_last_activity_times(&addresses).await?;

    // Build response entries
    let data: Vec<ProverLeaderboardEntry> = entries
        .into_iter()
        .map(|entry| {
            let median = median_prices.get(&entry.prover_address).cloned();
            let last_activity = last_activities
                .get(&entry.prover_address)
                .cloned()
                .unwrap_or(entry.last_activity_time);

            let last_activity_iso = DateTime::<Utc>::from_timestamp(last_activity as i64, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_default();

            ProverLeaderboardEntry {
                chain_id: state.chain_id,
                prover_address: format!("{:#x}", entry.prover_address),
                orders_locked: entry.orders_locked,
                orders_fulfilled: entry.orders_fulfilled,
                cycles: entry.cycles.to_string(),
                cycles_formatted: format_cycles(entry.cycles),
                fees_earned: entry.fees_earned.to_string(),
                fees_earned_formatted: format_eth(&entry.fees_earned.to_string()),
                collateral_earned: entry.collateral_earned.to_string(),
                collateral_earned_formatted: format_zkc(&entry.collateral_earned.to_string()),
                median_lock_price_per_cycle: median.map(|m| m.to_string()),
                median_lock_price_per_cycle_formatted: median.map(|m| format_eth(&m.to_string())),
                best_effective_prove_mhz: entry.best_effective_prove_mhz,
                locked_order_fulfillment_rate: entry.locked_order_fulfillment_rate,
                last_activity_time: last_activity as i64,
                last_activity_time_iso: last_activity_iso,
            }
        })
        .collect();

    // Generate next cursor if there are more results
    let next_cursor = if has_more && !data.is_empty() {
        let last = data.last().unwrap();
        Some(encode_prover_leaderboard_cursor(&ProverLeaderboardCursor {
            fees_earned: last.fees_earned.clone(),
            address: last.prover_address.clone(),
        })?)
    } else {
        None
    };

    Ok(ProverLeaderboardResponse {
        chain_id: state.chain_id,
        period: params.period.to_string(),
        period_start: start_ts as i64,
        period_end: end_ts as i64,
        data,
        next_cursor,
        has_more,
    })
}
