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

use alloy::primitives::Address;
use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::{str::FromStr, sync::Arc};

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
    models::{
        AggregateLeaderboardEntry, EpochLeaderboardEntry, EpochPoVWSummary, LeaderboardResponse,
        PaginationParams, PoVWAddressSummary, PoVWSummaryStats,
    },
    utils::{format_cycles, format_zkc},
};

/// Create PoVW routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Aggregate endpoints
        .route("/", get(get_aggregate_povw))
        // Epoch-specific endpoints
        .route("/epochs/:epoch", get(get_povw_by_epoch))
        // Address-specific endpoints
        .route("/addresses/:address", get(get_povw_history_by_address))
        .route("/addresses/:address/epochs/:epoch", get(get_povw_by_address_and_epoch))
}

/// GET /v1/povw
/// Returns the aggregate PoVW rewards leaderboard
async fn get_aggregate_povw(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_aggregate_povw_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Cache for 60 seconds (current leaderboard updates frequently)
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_aggregate_povw_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<AggregateLeaderboardEntry>> {
    tracing::debug!(
        "Fetching aggregate leaderboard with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch data from database
    let aggregates =
        state.rewards_db.get_povw_rewards_aggregate(params.offset, params.limit).await?;

    // Fetch summary stats
    let summary_stats = state.rewards_db.get_povw_summary_stats().await?;

    // Convert to response format with ranks
    let entries: Vec<AggregateLeaderboardEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, agg)| {
            let work_str = agg.total_work_submitted.to_string();
            let actual_str = agg.total_actual_rewards.to_string();
            let uncapped_str = agg.total_uncapped_rewards.to_string();
            AggregateLeaderboardEntry {
                rank: params.offset + (index as u64) + 1,
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

    // Create response with summary if available
    if let Some(stats) = summary_stats {
        let work_str = stats.total_work_all_time.to_string();
        let emissions_str = stats.total_emissions_all_time.to_string();
        let capped_str = stats.total_capped_rewards_all_time.to_string();
        let uncapped_str = stats.total_uncapped_rewards_all_time.to_string();
        let summary = PoVWSummaryStats {
            total_epochs_with_work: stats.total_epochs_with_work,
            total_unique_work_log_ids: stats.total_unique_work_log_ids,
            total_work_all_time: work_str.clone(),
            total_work_all_time_formatted: format_cycles(&work_str),
            total_emissions_all_time: emissions_str.clone(),
            total_emissions_all_time_formatted: format_zkc(&emissions_str),
            total_capped_rewards_all_time: capped_str.clone(),
            total_capped_rewards_all_time_formatted: format_zkc(&capped_str),
            total_uncapped_rewards_all_time: uncapped_str.clone(),
            total_uncapped_rewards_all_time_formatted: format_zkc(&uncapped_str),
        };
        Ok(LeaderboardResponse::with_summary(
            entries,
            params.offset,
            params.limit,
            serde_json::to_value(summary)?,
        ))
    } else {
        Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
    }
}

/// GET /v1/povw/epochs/:epoch
/// Returns the PoVW rewards for a specific epoch
async fn get_povw_by_epoch(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_povw_by_epoch_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Cache for 5 minutes (historical epochs don't change)
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_by_epoch_impl(
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

    // Fetch epoch summary
    let epoch_summary = state.rewards_db.get_epoch_povw_summary(epoch).await?;

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
                rank: params.offset + (index as u64) + 1,
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

    // Create response with summary if available
    if let Some(summary) = epoch_summary {
        let work_str = summary.total_work.to_string();
        let emissions_str = summary.total_emissions.to_string();
        let capped_str = summary.total_capped_rewards.to_string();
        let uncapped_str = summary.total_uncapped_rewards.to_string();
        let summary_data = EpochPoVWSummary {
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
        };
        Ok(LeaderboardResponse::with_summary(
            entries,
            params.offset,
            params.limit,
            serde_json::to_value(summary_data)?,
        ))
    } else {
        Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
    }
}
/// GET /v1/povw/addresses/:address
/// Returns the PoVW rewards history for a specific address
async fn get_povw_history_by_address(
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

    match get_povw_history_by_address_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_history_by_address_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochLeaderboardEntry>> {
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

    // Convert to response format
    let entries: Vec<EpochLeaderboardEntry> = paginated
        .into_iter()
        .enumerate()
        .map(|(index, reward)| {
            let work_str = reward.work_submitted.to_string();
            let uncapped_str = reward.uncapped_rewards.to_string();
            let cap_str = reward.reward_cap.to_string();
            let actual_str = reward.actual_rewards.to_string();
            let staked_str = reward.staked_amount.to_string();
            EpochLeaderboardEntry {
                rank: params.offset + (index as u64) + 1,
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

    // Create response with summary if available
    if let Some(aggregate) = address_aggregate {
        let work_str = aggregate.total_work_submitted.to_string();
        let actual_str = aggregate.total_actual_rewards.to_string();
        let uncapped_str = aggregate.total_uncapped_rewards.to_string();
        let summary = PoVWAddressSummary {
            work_log_id: format!("{:#x}", aggregate.work_log_id),
            total_work_submitted: work_str.clone(),
            total_work_submitted_formatted: format_cycles(&work_str),
            total_actual_rewards: actual_str.clone(),
            total_actual_rewards_formatted: format_zkc(&actual_str),
            total_uncapped_rewards: uncapped_str.clone(),
            total_uncapped_rewards_formatted: format_zkc(&uncapped_str),
            epochs_participated: aggregate.epochs_participated,
        };
        Ok(LeaderboardResponse::with_summary(
            entries,
            params.offset,
            params.limit,
            serde_json::to_value(summary)?,
        ))
    } else {
        Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
    }
}

/// GET /v1/povw/addresses/:address/epochs/:epoch
/// Returns the PoVW rewards for a specific address at a specific epoch
async fn get_povw_by_address_and_epoch(
    State(state): State<Arc<AppState>>,
    Path((address_str, epoch)): Path<(String, u64)>,
) -> Response {
    // Parse and validate address
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    match get_povw_by_address_and_epoch_impl(state, address, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_povw_by_address_and_epoch_impl(
    state: Arc<AppState>,
    address: Address,
    epoch: u64,
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
        rank: 0, // No rank for individual queries
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
