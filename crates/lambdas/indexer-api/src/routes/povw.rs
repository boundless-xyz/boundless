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
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::{str::FromStr, sync::Arc};
use utoipa;

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
    models::{
        AddressLeaderboardResponse, AggregateLeaderboardEntry, EpochLeaderboardEntry,
        EpochPoVWSummary, LeaderboardResponse, PaginationParams, PoVWAddressSummary,
        PoVWSummaryStats,
    },
    utils::{format_cycles, format_zkc},
};

/// Create PoVW routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Aggregate summary endpoint
        .route("/", get(get_povw_summary))
        // Epoch endpoints
        .route("/epochs", get(get_povw_all_epochs_summary))
        .route("/epochs/:epoch", get(get_povw_epoch_summary))
        .route("/epochs/:epoch/addresses", get(get_povw_epoch_leaderboard))
        .route("/epochs/:epoch/addresses/:address", get(get_povw_address_at_epoch))
        // Address endpoints
        .route("/addresses", get(get_povw_all_time_leaderboard))
        .route("/addresses/:address", get(get_povw_address_history))
}

/// GET /v1/povw
/// Returns the aggregate PoVW summary
#[utoipa::path(
    get,
    path = "/v1/povw",
    tag = "PoVW",
    responses(
        (status = 200, description = "PoVW summary statistics", body = PoVWSummaryStats),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_summary(State(state): State<Arc<AppState>>) -> Response {
    match get_povw_summary_impl(state).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_summary_impl(state: Arc<AppState>) -> anyhow::Result<PoVWSummaryStats> {
    tracing::debug!("Fetching PoVW summary stats");

    // Fetch summary stats
    let summary_stats = state
        .rewards_db
        .get_povw_summary_stats()
        .await?
        .ok_or_else(|| anyhow::anyhow!("No PoVW summary data available"))?;

    let work_str = summary_stats.total_work_all_time.to_string();
    let emissions_str = summary_stats.total_emissions_all_time.to_string();
    let capped_str = summary_stats.total_capped_rewards_all_time.to_string();
    let uncapped_str = summary_stats.total_uncapped_rewards_all_time.to_string();

    Ok(PoVWSummaryStats {
        total_epochs_with_work: summary_stats.total_epochs_with_work,
        total_unique_work_log_ids: summary_stats.total_unique_work_log_ids,
        total_work_all_time: work_str.clone(),
        total_work_all_time_formatted: format_cycles(&work_str),
        total_emissions_all_time: emissions_str.clone(),
        total_emissions_all_time_formatted: format_zkc(&emissions_str),
        total_capped_rewards_all_time: capped_str.clone(),
        total_capped_rewards_all_time_formatted: format_zkc(&capped_str),
        total_uncapped_rewards_all_time: uncapped_str.clone(),
        total_uncapped_rewards_all_time_formatted: format_zkc(&uncapped_str),
        last_updated_at: summary_stats.updated_at,
    })
}

/// GET /v1/povw/epochs
/// Returns summary of all epochs
#[utoipa::path(
    get,
    path = "/v1/povw/epochs",
    tag = "PoVW",
    params(
        PaginationParams
    ),
    responses(
        (status = 200, description = "All epochs PoVW summary", body = LeaderboardResponse<EpochPoVWSummary>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_all_epochs_summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_povw_all_epochs_summary_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_all_epochs_summary_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochPoVWSummary>> {
    tracing::debug!(
        "Fetching all epochs summary with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch all epoch summaries
    let summaries =
        state.rewards_db.get_all_epoch_povw_summaries(params.offset, params.limit).await?;

    // Convert to response format
    let entries: Vec<EpochPoVWSummary> = summaries
        .into_iter()
        .map(|summary| {
            let work_str = summary.total_work.to_string();
            let emissions_str = summary.total_emissions.to_string();
            let capped_str = summary.total_capped_rewards.to_string();
            let uncapped_str = summary.total_uncapped_rewards.to_string();
            EpochPoVWSummary {
                epoch: summary.epoch,
                total_work: work_str.clone(),
                total_work_formatted: format_cycles(&work_str),
                total_emissions: emissions_str.clone(),
                total_emissions_formatted: format_zkc(&emissions_str),
                total_capped_rewards: capped_str.clone(),
                total_capped_rewards_formatted: format_zkc(&capped_str),
                total_uncapped_rewards: uncapped_str.clone(),
                total_uncapped_rewards_formatted: format_zkc(&uncapped_str),
                epoch_start_time: summary.epoch_start_time,
                epoch_end_time: summary.epoch_end_time,
                num_participants: summary.num_participants,
                last_updated_at: summary.updated_at,
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/povw/epochs/:epoch
/// Returns summary for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/povw/epochs/{epoch}",
    tag = "PoVW",
    params(
        ("epoch" = u64, Path, description = "Epoch number")
    ),
    responses(
        (status = 200, description = "Epoch PoVW summary", body = EpochPoVWSummary),
        (status = 404, description = "Epoch not found"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_epoch_summary(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
) -> Response {
    match get_povw_epoch_summary_impl(state, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_epoch_summary_impl(
    state: Arc<AppState>,
    epoch: u64,
) -> anyhow::Result<EpochPoVWSummary> {
    tracing::debug!("Fetching PoVW summary for epoch {}", epoch);

    // Fetch epoch summary
    let summary = state
        .rewards_db
        .get_epoch_povw_summary(epoch)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No data available for epoch {}", epoch))?;

    let work_str = summary.total_work.to_string();
    let emissions_str = summary.total_emissions.to_string();
    let capped_str = summary.total_capped_rewards.to_string();
    let uncapped_str = summary.total_uncapped_rewards.to_string();

    Ok(EpochPoVWSummary {
        epoch: summary.epoch,
        total_work: work_str.clone(),
        total_work_formatted: format_cycles(&work_str),
        total_emissions: emissions_str.clone(),
        total_emissions_formatted: format_zkc(&emissions_str),
        total_capped_rewards: capped_str.clone(),
        total_capped_rewards_formatted: format_zkc(&capped_str),
        total_uncapped_rewards: uncapped_str.clone(),
        total_uncapped_rewards_formatted: format_zkc(&uncapped_str),
        epoch_start_time: summary.epoch_start_time,
        epoch_end_time: summary.epoch_end_time,
        num_participants: summary.num_participants,
        last_updated_at: summary.updated_at,
    })
}

/// GET /v1/povw/epochs/:epoch/addresses
/// Returns the leaderboard for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/povw/epochs/{epoch}/addresses",
    tag = "PoVW",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Epoch PoVW leaderboard", body = LeaderboardResponse<EpochLeaderboardEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_epoch_leaderboard(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_povw_epoch_leaderboard_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_epoch_leaderboard_impl(
    state: Arc<AppState>,
    epoch: u64,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochLeaderboardEntry>> {
    tracing::debug!(
        "Fetching epoch {} leaderboard with offset={}, limit={}",
        epoch,
        params.offset,
        params.limit
    );

    // Fetch data from database
    let rewards =
        state.rewards_db.get_povw_rewards_by_epoch(epoch, params.offset, params.limit).await?;

    // Convert to response format with ranks
    let entries: Vec<EpochLeaderboardEntry> = rewards
        .into_iter()
        .enumerate()
        .map(|(index, reward)| {
            let work_str = reward.work_submitted.to_string();
            let uncapped_str = reward.uncapped_rewards.to_string();
            let cap_str = reward.reward_cap.to_string();
            let actual_str = reward.actual_rewards.to_string();
            let staked_str = reward.staked_amount.to_string();
            EpochLeaderboardEntry {
                rank: Some(params.offset + (index as u64) + 1),
                work_log_id: format!("{:#x}", reward.work_log_id),
                epoch: reward.epoch,
                work_submitted: work_str.clone(),
                work_submitted_formatted: format_cycles(&work_str),
                percentage: reward.percentage,
                uncapped_rewards: uncapped_str.clone(),
                uncapped_rewards_formatted: format_zkc(&uncapped_str),
                reward_cap: cap_str.clone(),
                reward_cap_formatted: format_zkc(&cap_str),
                actual_rewards: actual_str.clone(),
                actual_rewards_formatted: format_zkc(&actual_str),
                is_capped: reward.is_capped,
                staked_amount: staked_str.clone(),
                staked_amount_formatted: format_zkc(&staked_str),
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/povw/epochs/:epoch/addresses/:address
/// Returns the PoVW rewards for a specific address at a specific epoch
#[utoipa::path(
    get,
    path = "/v1/povw/epochs/{epoch}/addresses/{address}",
    tag = "PoVW",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        ("address" = String, Path, description = "Ethereum address")
    ),
    responses(
        (status = 200, description = "PoVW rewards for address at epoch", body = Option<EpochLeaderboardEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_address_at_epoch(
    State(state): State<Arc<AppState>>,
    Path((epoch, address_str)): Path<(u64, String)>,
) -> Response {
    // Parse and validate address
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    match get_povw_address_at_epoch_impl(state, epoch, address).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_address_at_epoch_impl(
    state: Arc<AppState>,
    epoch: u64,
    address: Address,
) -> anyhow::Result<Option<EpochLeaderboardEntry>> {
    tracing::debug!("Fetching PoVW rewards for address {} at epoch {}", address, epoch);

    // Fetch PoVW history for the address at specific epoch
    let rewards = state
        .rewards_db
        .get_povw_rewards_history_by_address(address, Some(epoch), Some(epoch))
        .await?;

    if rewards.is_empty() {
        return Ok(None);
    }

    let reward = &rewards[0];
    let work_str = reward.work_submitted.to_string();
    let uncapped_str = reward.uncapped_rewards.to_string();
    let cap_str = reward.reward_cap.to_string();
    let actual_str = reward.actual_rewards.to_string();
    let staked_str = reward.staked_amount.to_string();
    Ok(Some(EpochLeaderboardEntry {
        rank: None, // No rank for individual queries
        work_log_id: format!("{:#x}", reward.work_log_id),
        epoch: reward.epoch,
        work_submitted: work_str.clone(),
        work_submitted_formatted: format_cycles(&work_str),
        percentage: reward.percentage,
        uncapped_rewards: uncapped_str.clone(),
        uncapped_rewards_formatted: format_zkc(&uncapped_str),
        reward_cap: cap_str.clone(),
        reward_cap_formatted: format_zkc(&cap_str),
        actual_rewards: actual_str.clone(),
        actual_rewards_formatted: format_zkc(&actual_str),
        is_capped: reward.is_capped,
        staked_amount: staked_str.clone(),
        staked_amount_formatted: format_zkc(&staked_str),
    }))
}

/// GET /v1/povw/addresses
/// Returns the all-time PoVW leaderboard
#[utoipa::path(
    get,
    path = "/v1/povw/addresses",
    tag = "PoVW",
    params(PaginationParams),
    responses(
        (status = 200, description = "PoVW leaderboard", body = LeaderboardResponse<AggregateLeaderboardEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_all_time_leaderboard(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_povw_all_time_leaderboard_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_all_time_leaderboard_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<AggregateLeaderboardEntry>> {
    tracing::debug!(
        "Fetching all-time PoVW leaderboard with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch data from database
    let aggregates =
        state.rewards_db.get_povw_rewards_aggregate(params.offset, params.limit).await?;

    // Convert to response format with ranks
    let entries: Vec<AggregateLeaderboardEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, agg)| {
            let work_str = agg.total_work_submitted.to_string();
            let actual_str = agg.total_actual_rewards.to_string();
            let uncapped_str = agg.total_uncapped_rewards.to_string();
            AggregateLeaderboardEntry {
                rank: Some(params.offset + (index as u64) + 1),
                work_log_id: format!("{:#x}", agg.work_log_id),
                total_work_submitted: work_str.clone(),
                total_work_submitted_formatted: format_cycles(&work_str),
                total_actual_rewards: actual_str.clone(),
                total_actual_rewards_formatted: format_zkc(&actual_str),
                total_uncapped_rewards: uncapped_str.clone(),
                total_uncapped_rewards_formatted: format_zkc(&uncapped_str),
                epochs_participated: agg.epochs_participated,
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/povw/addresses/:address
/// Returns the PoVW rewards history for a specific address
#[utoipa::path(
    get,
    path = "/v1/povw/addresses/{address}",
    tag = "PoVW",
    params(
        ("address" = String, Path, description = "Work log ID (Ethereum address)"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Address PoVW history", body = AddressLeaderboardResponse<EpochLeaderboardEntry, PoVWAddressSummary>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_povw_address_history(
    State(state): State<Arc<AppState>>,
    Path(address_str): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Response {
    // Parse and validate address
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    let params = params.validate();

    match get_povw_address_history_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_address_history_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<AddressLeaderboardResponse<EpochLeaderboardEntry, PoVWAddressSummary>> {
    tracing::debug!(
        "Fetching PoVW history for address {} with offset={}, limit={}",
        address,
        params.offset,
        params.limit
    );

    // Fetch PoVW history for the address
    let rewards = state.rewards_db.get_povw_rewards_history_by_address(address, None, None).await?;

    // Fetch aggregate summary for this address
    let address_aggregate = state.rewards_db.get_povw_rewards_aggregate_by_address(address).await?;

    // Apply pagination
    let start = params.offset as usize;
    let end = (start + params.limit as usize).min(rewards.len());
    let paginated = if start < rewards.len() { rewards[start..end].to_vec() } else { vec![] };

    // Convert to response format without rank (this is address history, not a leaderboard)
    let entries: Vec<EpochLeaderboardEntry> = paginated
        .into_iter()
        .map(|reward| {
            let work_str = reward.work_submitted.to_string();
            let uncapped_str = reward.uncapped_rewards.to_string();
            let cap_str = reward.reward_cap.to_string();
            let actual_str = reward.actual_rewards.to_string();
            let staked_str = reward.staked_amount.to_string();
            EpochLeaderboardEntry {
                rank: None, // No rank for individual address history
                work_log_id: format!("{:#x}", reward.work_log_id),
                epoch: reward.epoch,
                work_submitted: work_str.clone(),
                work_submitted_formatted: format_cycles(&work_str),
                percentage: reward.percentage,
                uncapped_rewards: uncapped_str.clone(),
                uncapped_rewards_formatted: format_zkc(&uncapped_str),
                reward_cap: cap_str.clone(),
                reward_cap_formatted: format_zkc(&cap_str),
                actual_rewards: actual_str.clone(),
                actual_rewards_formatted: format_zkc(&actual_str),
                is_capped: reward.is_capped,
                staked_amount: staked_str.clone(),
                staked_amount_formatted: format_zkc(&staked_str),
            }
        })
        .collect();

    // Create summary from aggregate if available, otherwise use default
    let summary = if let Some(aggregate) = address_aggregate {
        let work_str = aggregate.total_work_submitted.to_string();
        let actual_str = aggregate.total_actual_rewards.to_string();
        let uncapped_str = aggregate.total_uncapped_rewards.to_string();
        PoVWAddressSummary {
            work_log_id: format!("{:#x}", aggregate.work_log_id),
            total_work_submitted: work_str.clone(),
            total_work_submitted_formatted: format_cycles(&work_str),
            total_actual_rewards: actual_str.clone(),
            total_actual_rewards_formatted: format_zkc(&actual_str),
            total_uncapped_rewards: uncapped_str.clone(),
            total_uncapped_rewards_formatted: format_zkc(&uncapped_str),
            epochs_participated: aggregate.epochs_participated,
        }
    } else {
        // No data for this address - return empty summary
        PoVWAddressSummary {
            work_log_id: format!("{:#x}", address),
            total_work_submitted: "0".to_string(),
            total_work_submitted_formatted: format_cycles("0"),
            total_actual_rewards: "0".to_string(),
            total_actual_rewards_formatted: format_zkc("0"),
            total_uncapped_rewards: "0".to_string(),
            total_uncapped_rewards_formatted: format_zkc("0"),
            epochs_participated: 0,
        }
    };

    Ok(AddressLeaderboardResponse::new(entries, params.offset, params.limit, summary))
}
