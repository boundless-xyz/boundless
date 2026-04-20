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
use std::{str::FromStr, sync::Arc};

pub use boundless_market::indexer_types::*;

use crate::{
    db::AppState,
    handler::{bad_request_invalid_address, cache_control, handle_error, AddressRole},
    models::{
        EfficiencyAggregateEntry, EfficiencyAggregatesParams, EfficiencyAggregatesResponse,
        EfficiencyRequestEntry, EfficiencyRequestsParams, EfficiencyRequestsResponse,
        EfficiencySummaryParams, EfficiencySummaryResponse, EfficiencyType,
        MoreProfitableSampleEntry,
    },
    utils::{format_eth, format_zkc, is_valid_ethereum_address},
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
        .route("/efficiency", get(get_efficiency_summary))
        .route("/efficiency/aggregates", get(get_efficiency_aggregates))
        .route("/efficiency/requests", get(list_efficiency_requests))
        .route("/efficiency/requests/:request_id", get(get_efficiency_request_by_id))
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

    // Handle epoch aggregation separately since it returns a different type
    if matches!(params.aggregation, AggregationGranularity::Epoch) {
        return get_epoch_market_aggregates_impl(state, cursor, limit, limit_plus_one, sort).await;
    }

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
        AggregationGranularity::Epoch => {
            // Handled by early return above
            unreachable!()
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
            let p5_lock_price_per_cycle = summary.p5_lock_price_per_cycle.to_string();
            let p10_lock_price_per_cycle = summary.p10_lock_price_per_cycle.to_string();
            let p25_lock_price_per_cycle = summary.p25_lock_price_per_cycle.to_string();
            let p50_lock_price_per_cycle = summary.p50_lock_price_per_cycle.to_string();
            let p75_lock_price_per_cycle = summary.p75_lock_price_per_cycle.to_string();
            let p90_lock_price_per_cycle = summary.p90_lock_price_per_cycle.to_string();
            let p95_lock_price_per_cycle = summary.p95_lock_price_per_cycle.to_string();
            let p99_lock_price_per_cycle = summary.p99_lock_price_per_cycle.to_string();

            // Use epoch number from DB (populated by aggregation logic)
            // Only include if timestamp is at or after epoch 0 start
            let epoch_number_start =
                if summary.period_timestamp >= state.epoch_calculator.epoch0_start_time() {
                    Some(summary.epoch_number_period_start)
                } else {
                    None
                };

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
                p5_lock_price_per_cycle: p5_lock_price_per_cycle.clone(),
                p5_lock_price_per_cycle_formatted: format_eth(&p5_lock_price_per_cycle),
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
                total_fixed_cost: summary.total_fixed_cost.to_string(),
                total_fixed_cost_formatted: format_eth(&summary.total_fixed_cost.to_string()),
                total_variable_cost: summary.total_variable_cost.to_string(),
                total_variable_cost_formatted: format_eth(&summary.total_variable_cost.to_string()),
                epoch_number_start,
            }
        })
        .collect();

    // Fetch current market-wide collateral stats
    // 20 ZKC = 20 * 10^18 wei
    let eligible_threshold = U256::from(20) * U256::from(10).pow(U256::from(18));
    let collateral_stats = state.market_db.get_market_collateral_stats(eligible_threshold).await?;
    let total_deposited_str = collateral_stats.total_collateral_deposited.to_string();

    Ok(MarketAggregatesResponse {
        chain_id: state.chain_id,
        aggregation: params.aggregation,
        data,
        next_cursor,
        has_more,
        total_collateral_deposited: total_deposited_str.clone(),
        total_collateral_deposited_formatted: format_zkc(&total_deposited_str),
        eligible_prover_count: collateral_stats.eligible_prover_count,
        active_eligible_prover_count: collateral_stats.active_eligible_prover_count,
    })
}

async fn get_epoch_market_aggregates_impl(
    state: Arc<AppState>,
    cursor: Option<i64>,
    limit: u64,
    limit_plus_one: i64,
    sort: SortDirection,
) -> anyhow::Result<MarketAggregatesResponse> {
    let mut summaries =
        state.market_db.get_epoch_market_summaries(cursor, limit_plus_one, sort).await?;

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
            // Use epoch number from DB (populated by aggregation logic)
            // Only include if timestamp is at or after epoch 0 start
            let epoch_number_start =
                if summary.period_timestamp >= state.epoch_calculator.epoch0_start_time() {
                    Some(summary.epoch_number_period_start)
                } else {
                    None
                };

            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);
            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_collateral_locked = summary.total_collateral_locked.to_string();
            let total_locked_and_expired_collateral =
                summary.total_locked_and_expired_collateral.to_string();
            let p5_lock_price_per_cycle = summary.p5_lock_price_per_cycle.to_string();
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
                p5_lock_price_per_cycle: p5_lock_price_per_cycle.clone(),
                p5_lock_price_per_cycle_formatted: format_eth(&p5_lock_price_per_cycle),
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
                total_fixed_cost: summary.total_fixed_cost.to_string(),
                total_fixed_cost_formatted: format_eth(&summary.total_fixed_cost.to_string()),
                total_variable_cost: summary.total_variable_cost.to_string(),
                total_variable_cost_formatted: format_eth(&summary.total_variable_cost.to_string()),
                epoch_number_start,
            }
        })
        .collect();

    // Fetch current market-wide collateral stats
    let eligible_threshold = U256::from(20) * U256::from(10).pow(U256::from(18));
    let collateral_stats = state.market_db.get_market_collateral_stats(eligible_threshold).await?;
    let total_deposited_str = collateral_stats.total_collateral_deposited.to_string();

    Ok(MarketAggregatesResponse {
        chain_id: state.chain_id,
        aggregation: AggregationGranularity::Epoch,
        data,
        next_cursor,
        has_more,
        total_collateral_deposited: total_deposited_str.clone(),
        total_collateral_deposited_formatted: format_zkc(&total_deposited_str),
        eligible_prover_count: collateral_stats.eligible_prover_count,
        active_eligible_prover_count: collateral_stats.active_eligible_prover_count,
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
                total_fixed_cost: summary.total_fixed_cost.to_string(),
                total_fixed_cost_formatted: format_eth(&summary.total_fixed_cost.to_string()),
                total_variable_cost: summary.total_variable_cost.to_string(),
                total_variable_cost_formatted: format_eth(&summary.total_variable_cost.to_string()),
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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Requestor, &address);
    }
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
            anyhow::bail!("Monthly aggregation is not supported");
        }
        AggregationGranularity::Epoch => {
            state
                .market_db
                .get_epoch_requestor_summaries(
                    requestor_address,
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
            let timestamp_iso = format_timestamp_iso(summary.period_timestamp as i64);

            let total_fees_locked = summary.total_fees_locked.to_string();
            let total_fees_paid = summary.total_fees_paid.to_string();
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

            // Use epoch number from DB (populated by aggregation logic)
            // Only include if timestamp is at or after epoch 0 start
            let epoch_number_start =
                if summary.period_timestamp >= state.epoch_calculator.epoch0_start_time() {
                    Some(summary.epoch_number_period_start)
                } else {
                    None
                };

            RequestorAggregateEntry {
                chain_id: state.chain_id,
                requestor_address: format!("{:#x}", summary.requestor_address),
                timestamp: summary.period_timestamp as i64,
                timestamp_iso,
                total_fulfilled: summary.total_fulfilled as i64,
                unique_provers_locking_requests: summary.unique_provers_locking_requests as i64,
                total_fees_locked: total_fees_locked.clone(),
                total_fees_locked_formatted: format_eth(&total_fees_locked),
                total_fees_paid: total_fees_paid.clone(),
                total_fees_paid_formatted: format_eth(&total_fees_paid),
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
                total_fixed_cost: summary.total_fixed_cost.to_string(),
                total_fixed_cost_formatted: format_eth(&summary.total_fixed_cost.to_string()),
                total_variable_cost: summary.total_variable_cost.to_string(),
                total_variable_cost_formatted: format_eth(&summary.total_variable_cost.to_string()),
                p50_time_to_lock_seconds: summary.p50_time_to_lock_seconds,
                p90_time_to_lock_seconds: summary.p90_time_to_lock_seconds,
                p50_time_to_fulfill_seconds: summary.p50_time_to_fulfill_seconds,
                p90_time_to_fulfill_seconds: summary.p90_time_to_fulfill_seconds,
                epoch_number_start,
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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Requestor, &address);
    }
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
            let total_fees_paid = summary.total_fees_paid.to_string();
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
                total_fees_paid: total_fees_paid.clone(),
                total_fees_paid_formatted: format_eth(&total_fees_paid),
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
                total_fixed_cost: summary.total_fixed_cost.to_string(),
                total_fixed_cost_formatted: format_eth(&summary.total_fixed_cost.to_string()),
                total_variable_cost: summary.total_variable_cost.to_string(),
                total_variable_cost_formatted: format_eth(&summary.total_variable_cost.to_string()),
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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Prover, &address);
    }
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
        AggregationGranularity::Epoch => {
            state
                .market_db
                .get_epoch_prover_summaries(
                    prover_address,
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

            // Use epoch number from DB (populated by aggregation logic)
            // Only include if timestamp is at or after epoch 0 start
            let epoch_number_start =
                if summary.period_timestamp >= state.epoch_calculator.epoch0_start_time() {
                    Some(summary.epoch_number_period_start)
                } else {
                    None
                };

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
                epoch_number_start,
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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Prover, &address);
    }
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

fn encode_request_cursor(cursor: &RequestCursor) -> Result<String, anyhow::Error> {
    let json = serde_json::to_string(cursor)?;
    Ok(BASE64.encode(json))
}

fn decode_request_cursor(cursor_str: &str) -> Result<RequestCursor, anyhow::Error> {
    let json = BASE64.decode(cursor_str)?;
    let cursor: RequestCursor = serde_json::from_slice(&json)?;
    Ok(cursor)
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
        fixed_cost: status.fixed_cost.clone(),
        fixed_cost_formatted: status.fixed_cost.as_ref().map(|c| format_eth(c)),
        variable_cost_per_cycle: status.variable_cost_per_cycle.clone(),
        variable_cost_per_cycle_formatted: status
            .variable_cost_per_cycle
            .as_ref()
            .map(|c| format_eth(c)),
        lock_base_fee: status.lock_base_fee,
        fulfill_base_fee: status.fulfill_base_fee,
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

    let deduplicate = params.deduplicate;

    let (statuses, next_cursor) =
        state.market_db.list_requests(cursor, limit, sort_by, deduplicate).await?;

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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Requestor, &address);
    }
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

    let deduplicate = params.deduplicate;

    let (statuses, next_cursor) = state
        .market_db
        .list_requests_by_requestor(client_address, cursor, limit, sort_by, deduplicate)
        .await?;

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
    if !is_valid_ethereum_address(&address) {
        return bad_request_invalid_address(AddressRole::Prover, &address);
    }
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

    let deduplicate = params.deduplicate;

    let (statuses, next_cursor) = state
        .market_db
        .list_requests_by_prover(prover_address, cursor, limit, sort_by, deduplicate)
        .await?;

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
                LeaderboardPeriod::OneHour => "public, max-age=300",
                LeaderboardPeriod::OneDay => "public, max-age=600",
                _ => "public, max-age=900",
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

    // Fetch median lock prices, p50 fixed/variable costs, and last activity times concurrently
    let (median_prices, p50_fixed_costs, p50_variable_costs, last_activities) = tokio::try_join!(
        state.market_db.get_requestor_median_lock_prices(&addresses, start_ts, end_ts),
        state.market_db.get_requestor_p50_fixed_costs(&addresses, start_ts, end_ts),
        state.market_db.get_requestor_p50_variable_costs(&addresses, start_ts, end_ts),
        state.market_db.get_requestor_last_activity_times(&addresses),
    )?;

    // Build response entries
    let data: Vec<RequestorLeaderboardEntry> = entries
        .into_iter()
        .map(|entry| {
            let median = median_prices.get(&entry.requestor_address).cloned();
            let p50_fc = p50_fixed_costs.get(&entry.requestor_address).cloned();
            let p50_vc = p50_variable_costs.get(&entry.requestor_address).cloned();
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
                orders_fulfilled: entry.orders_fulfilled,
                orders_expired: entry.orders_expired,
                orders_not_locked_and_expired: entry.orders_not_locked_and_expired,
                cycles_requested: entry.cycles_requested.to_string(),
                cycles_requested_formatted: format_cycles(entry.cycles_requested),
                median_lock_price_per_cycle: median.map(|m| m.to_string()),
                median_lock_price_per_cycle_formatted: median.map(|m| format_eth(&m.to_string())),
                p50_fixed_cost: p50_fc.map(|m| m.to_string()).unwrap_or_else(|| "0".to_string()),
                p50_fixed_cost_formatted: p50_fc
                    .map(|m| format_eth(&m.to_string()))
                    .unwrap_or_else(|| format_eth("0")),
                p50_variable_cost_per_cycle: p50_vc
                    .map(|m| m.to_string())
                    .unwrap_or_else(|| "0".to_string()),
                p50_variable_cost_per_cycle_formatted: p50_vc
                    .map(|m| format_eth(&m.to_string()))
                    .unwrap_or_else(|| format_eth("0")),
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
                LeaderboardPeriod::OneHour => "public, max-age=300",
                LeaderboardPeriod::OneDay => "public, max-age=600",
                _ => "public, max-age=900",
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

    // Fetch median lock prices, last activity times, and collateral balances concurrently
    let (median_prices, last_activities, collateral_balances, locked_collateral) = tokio::try_join!(
        state.market_db.get_prover_median_lock_prices(&addresses, start_ts, end_ts),
        state.market_db.get_prover_last_activity_times(&addresses),
        state.market_db.get_prover_collateral_balances(&addresses),
        state.market_db.get_prover_currently_locked_collateral(&addresses),
    )?;

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

            let deposited =
                collateral_balances.get(&entry.prover_address).copied().unwrap_or(U256::ZERO);
            let currently_locked =
                locked_collateral.get(&entry.prover_address).copied().unwrap_or(U256::ZERO);
            let available = deposited.saturating_sub(currently_locked);

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
                collateral_deposited_zkc: deposited.to_string(),
                collateral_deposited_zkc_formatted: format_zkc(&deposited.to_string()),
                collateral_available_zkc: available.to_string(),
                collateral_available_zkc_formatted: format_zkc(&available.to_string()),
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

const MAX_EFFICIENCY_RESULTS: u64 = 500;

/// GET /v1/market/efficiency
/// Returns market efficiency summary statistics
#[utoipa::path(
    get,
    path = "/v1/market/efficiency",
    tag = "Market",
    params(EfficiencySummaryParams),
    responses(
        (status = 200, description = "Market efficiency summary", body = EfficiencySummaryResponse),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_efficiency_summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EfficiencySummaryParams>,
) -> Response {
    match get_efficiency_summary_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_efficiency_summary_impl(
    state: Arc<AppState>,
    params: EfficiencySummaryParams,
) -> anyhow::Result<EfficiencySummaryResponse> {
    let summary = match params.r#type {
        EfficiencyType::GasAdjustedWithExclusions => {
            state.efficiency_db.get_efficiency_summary_gas_adjusted_with_exclusions().await?
        }
        EfficiencyType::GasAdjusted => {
            state.efficiency_db.get_efficiency_summary_gas_adjusted().await?
        }
        EfficiencyType::Raw => state.efficiency_db.get_efficiency_summary().await?,
    };

    let last_updated = summary.last_updated.map(|ts| {
        DateTime::<Utc>::from_timestamp(ts, 0)
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_default()
    });

    Ok(EfficiencySummaryResponse {
        latest_hourly_efficiency_rate: summary.latest_hourly_efficiency_rate,
        latest_daily_efficiency_rate: summary.latest_daily_efficiency_rate,
        total_requests_analyzed: summary.total_requests_analyzed,
        most_profitable_locked: summary.most_profitable_locked,
        not_most_profitable_locked: summary.not_most_profitable_locked,
        last_updated,
    })
}

/// GET /v1/market/efficiency/aggregates
/// Returns time-series efficiency aggregates
#[utoipa::path(
    get,
    path = "/v1/market/efficiency/aggregates",
    tag = "Market",
    params(EfficiencyAggregatesParams),
    responses(
        (status = 200, description = "Efficiency aggregates", body = EfficiencyAggregatesResponse),
        (status = 400, description = "Invalid parameters"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_efficiency_aggregates(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EfficiencyAggregatesParams>,
) -> Response {
    match get_efficiency_aggregates_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_efficiency_aggregates_impl(
    state: Arc<AppState>,
    params: EfficiencyAggregatesParams,
) -> anyhow::Result<EfficiencyAggregatesResponse> {
    let limit = params.limit.min(MAX_EFFICIENCY_RESULTS);
    let sort_desc = params.sort.to_lowercase() != "asc";

    // Decode cursor if provided
    let cursor = if let Some(cursor_str) = &params.cursor {
        let decoded =
            BASE64.decode(cursor_str).map_err(|e| anyhow::anyhow!("Invalid cursor: {}", e))?;
        let cursor_val: u64 = String::from_utf8(decoded)
            .map_err(|e| anyhow::anyhow!("Invalid cursor encoding: {}", e))?
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid cursor value: {}", e))?;
        Some(cursor_val)
    } else {
        None
    };

    // Request one extra to determine if more pages exist
    let mut aggregates = match params.r#type {
        EfficiencyType::GasAdjustedWithExclusions => {
            state
                .efficiency_db
                .get_efficiency_aggregates_gas_adjusted_with_exclusions(
                    &params.granularity,
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
        EfficiencyType::GasAdjusted => {
            state
                .efficiency_db
                .get_efficiency_aggregates_gas_adjusted(
                    &params.granularity,
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
        EfficiencyType::Raw => {
            state
                .efficiency_db
                .get_efficiency_aggregates(
                    &params.granularity,
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
    };

    let has_more = aggregates.len() > limit as usize;
    if has_more {
        aggregates.pop();
    }

    let data: Vec<EfficiencyAggregateEntry> = aggregates
        .into_iter()
        .map(|agg| {
            let period_timestamp_iso =
                DateTime::<Utc>::from_timestamp(agg.period_timestamp as i64, 0)
                    .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                    .unwrap_or_default();

            EfficiencyAggregateEntry {
                period_timestamp: agg.period_timestamp,
                period_timestamp_iso,
                num_most_profitable_locked: agg.num_most_profitable_locked,
                num_not_most_profitable_locked: agg.num_not_most_profitable_locked,
                efficiency_rate: agg.efficiency_rate,
            }
        })
        .collect();

    let next_cursor = if has_more && !data.is_empty() {
        let last = data.last().unwrap();
        Some(BASE64.encode(last.period_timestamp.to_string()))
    } else {
        None
    };

    Ok(EfficiencyAggregatesResponse { data, has_more, next_cursor })
}

/// GET /v1/market/efficiency/requests
/// Returns individual request efficiency records
#[utoipa::path(
    get,
    path = "/v1/market/efficiency/requests",
    tag = "Market",
    params(EfficiencyRequestsParams),
    responses(
        (status = 200, description = "Efficiency request records", body = EfficiencyRequestsResponse),
        (status = 400, description = "Invalid parameters"),
        (status = 500, description = "Internal server error")
    )
)]
async fn list_efficiency_requests(
    State(state): State<Arc<AppState>>,
    Query(params): Query<EfficiencyRequestsParams>,
) -> Response {
    match list_efficiency_requests_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn list_efficiency_requests_impl(
    state: Arc<AppState>,
    params: EfficiencyRequestsParams,
) -> anyhow::Result<EfficiencyRequestsResponse> {
    let limit = params.limit.min(MAX_EFFICIENCY_RESULTS);
    let sort_desc = params.sort.to_lowercase() != "asc";

    // Decode cursor if provided
    let cursor = if let Some(cursor_str) = &params.cursor {
        let decoded =
            BASE64.decode(cursor_str).map_err(|e| anyhow::anyhow!("Invalid cursor: {}", e))?;
        let cursor_val: u64 = String::from_utf8(decoded)
            .map_err(|e| anyhow::anyhow!("Invalid cursor encoding: {}", e))?
            .parse()
            .map_err(|e| anyhow::anyhow!("Invalid cursor value: {}", e))?;
        Some(cursor_val)
    } else {
        None
    };

    // Request one extra to determine if more pages exist
    let mut requests = match params.efficiency_type {
        EfficiencyType::GasAdjustedWithExclusions => {
            state
                .efficiency_db
                .get_efficiency_requests_paginated_gas_adjusted_with_exclusions(
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
        EfficiencyType::GasAdjusted => {
            state
                .efficiency_db
                .get_efficiency_requests_paginated_gas_adjusted(
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
        EfficiencyType::Raw => {
            state
                .efficiency_db
                .get_efficiency_requests_paginated(
                    params.before,
                    params.after,
                    limit + 1,
                    cursor,
                    sort_desc,
                )
                .await?
        }
    };

    let has_more = requests.len() > limit as usize;
    if has_more {
        requests.pop();
    }

    let data: Vec<EfficiencyRequestEntry> = requests
        .into_iter()
        .map(|req| {
            let locked_at_iso = DateTime::<Utc>::from_timestamp(req.locked_at as i64, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_default();

            let more_profitable_sample = req.more_profitable_sample.map(|samples| {
                samples
                    .into_iter()
                    .map(|s| MoreProfitableSampleEntry {
                        request_digest: s.request_digest,
                        request_id: s.request_id,
                        requestor_address: s.requestor_address,
                        lock_price_at_time: s.lock_price_at_time,
                        program_cycles: s.program_cycles,
                        price_per_cycle_at_time: s.price_per_cycle_at_time,
                    })
                    .collect()
            });

            EfficiencyRequestEntry {
                request_digest: format!("0x{:x}", req.request_digest),
                request_id: req.request_id.to_string(),
                locked_at: req.locked_at,
                locked_at_iso,
                lock_price: req.lock_price.to_string(),
                lock_price_formatted: format_eth(&req.lock_price.to_string()),
                program_cycles: req.program_cycles.to_string(),
                program_cycles_formatted: format_cycles(req.program_cycles),
                lock_price_per_cycle: req.lock_price_per_cycle.to_string(),
                is_most_profitable: req.is_most_profitable,
                num_requests_more_profitable: req.num_orders_more_profitable,
                num_requests_less_profitable: req.num_orders_less_profitable,
                num_requests_available_unfulfilled: req.num_orders_available_unfulfilled,
                more_profitable_sample,
            }
        })
        .collect();

    let next_cursor = if has_more && !data.is_empty() {
        let last = data.last().unwrap();
        Some(BASE64.encode(last.locked_at.to_string()))
    } else {
        None
    };

    Ok(EfficiencyRequestsResponse { data, has_more, next_cursor })
}

/// GET /v1/market/efficiency/requests/:request_id
/// Returns efficiency details for a specific request
#[utoipa::path(
    get,
    path = "/v1/market/efficiency/requests/{request_id}",
    tag = "Market",
    params(
        ("request_id" = String, Path, description = "Request ID")
    ),
    responses(
        (status = 200, description = "Request efficiency details", body = EfficiencyRequestEntry),
        (status = 404, description = "Request not found"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_efficiency_request_by_id(
    State(state): State<Arc<AppState>>,
    Path(request_id): Path<String>,
) -> Response {
    match get_efficiency_request_by_id_impl(state, request_id).await {
        Ok(Some(response)) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Ok(None) => {
            let error_response = serde_json::json!({
                "error": "Request not found"
            });
            (axum::http::StatusCode::NOT_FOUND, Json(error_response)).into_response()
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_efficiency_request_by_id_impl(
    state: Arc<AppState>,
    request_id: String,
) -> anyhow::Result<Option<EfficiencyRequestEntry>> {
    let req = state.efficiency_db.get_efficiency_request_by_id(&request_id).await?;

    match req {
        Some(req) => {
            let locked_at_iso = DateTime::<Utc>::from_timestamp(req.locked_at as i64, 0)
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
                .unwrap_or_default();

            let more_profitable_sample = req.more_profitable_sample.map(|samples| {
                samples
                    .into_iter()
                    .map(|s| MoreProfitableSampleEntry {
                        request_digest: s.request_digest,
                        request_id: s.request_id,
                        requestor_address: s.requestor_address,
                        lock_price_at_time: s.lock_price_at_time,
                        program_cycles: s.program_cycles,
                        price_per_cycle_at_time: s.price_per_cycle_at_time,
                    })
                    .collect()
            });

            Ok(Some(EfficiencyRequestEntry {
                request_digest: format!("0x{:x}", req.request_digest),
                request_id: req.request_id.to_string(),
                locked_at: req.locked_at,
                locked_at_iso,
                lock_price: req.lock_price.to_string(),
                lock_price_formatted: format_eth(&req.lock_price.to_string()),
                program_cycles: req.program_cycles.to_string(),
                program_cycles_formatted: format_cycles(req.program_cycles),
                lock_price_per_cycle: req.lock_price_per_cycle.to_string(),
                is_most_profitable: req.is_most_profitable,
                num_requests_more_profitable: req.num_orders_more_profitable,
                num_requests_less_profitable: req.num_orders_less_profitable,
                num_requests_available_unfulfilled: req.num_orders_available_unfulfilled,
                more_profitable_sample,
            }))
        }
        None => Ok(None),
    }
}
