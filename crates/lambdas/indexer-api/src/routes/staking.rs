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
        AddressLeaderboardResponse, AggregateStakingEntry, EpochStakingEntry, EpochStakingSummary,
        LeaderboardResponse, PaginationParams, StakingAddressSummary, StakingSummaryStats,
    },
    utils::format_zkc,
};

/// Create Staking routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Aggregate summary endpoint
        .route("/", get(get_staking_summary))
        // Epoch endpoints
        .route("/epochs", get(get_staking_all_epochs_summary))
        .route("/epochs/:epoch", get(get_staking_epoch_summary))
        .route("/epochs/:epoch/addresses", get(get_staking_epoch_leaderboard))
        .route("/epochs/:epoch/addresses/:address", get(get_staking_address_at_epoch))
        // Address endpoints
        .route("/addresses", get(get_staking_all_time_leaderboard))
        .route("/addresses/:address", get(get_staking_address_history))
}

/// GET /v1/staking
/// Returns the aggregate staking summary
#[utoipa::path(
    get,
    path = "/v1/staking",
    tag = "Staking",
    responses(
        (status = 200, description = "Staking summary statistics", body = StakingSummaryStats),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_summary(State(state): State<Arc<AppState>>) -> Response {
    match get_staking_summary_impl(state).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_summary_impl(state: Arc<AppState>) -> anyhow::Result<StakingSummaryStats> {
    tracing::debug!("Fetching staking summary stats");

    // Fetch summary stats
    let summary = state
        .rewards_db
        .get_staking_summary_stats()
        .await?
        .ok_or_else(|| anyhow::anyhow!("No staking summary data available"))?;

    let total_str = summary.current_total_staked.to_string();
    let emissions_str = summary
        .total_staking_emissions_all_time
        .map(|v| v.to_string())
        .unwrap_or_else(|| "0".to_string());

    Ok(StakingSummaryStats {
        current_total_staked: total_str.clone(),
        current_total_staked_formatted: format_zkc(&total_str),
        total_unique_stakers: summary.total_unique_stakers,
        current_active_stakers: summary.current_active_stakers,
        current_withdrawing: summary.current_withdrawing,
        total_staking_emissions_all_time: Some(emissions_str.clone()),
        total_staking_emissions_all_time_formatted: Some(format_zkc(&emissions_str)),
        last_updated_at: summary.updated_at,
    })
}

/// GET /v1/staking/epochs
/// Returns summary of all epochs
#[utoipa::path(
    get,
    path = "/v1/staking/epochs",
    tag = "Staking",
    params(
        PaginationParams
    ),
    responses(
        (status = 200, description = "All epochs staking summary", body = LeaderboardResponse<EpochStakingSummary>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_all_epochs_summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_staking_all_epochs_summary_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_all_epochs_summary_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochStakingSummary>> {
    tracing::debug!(
        "Fetching all epochs staking summary with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch all epoch summaries
    let summaries =
        state.rewards_db.get_all_epoch_staking_summaries(params.offset, params.limit).await?;

    // Convert to response format
    let entries: Vec<EpochStakingSummary> = summaries
        .into_iter()
        .map(|summary| {
            let total_str = summary.total_staked.to_string();
            let emissions_str = summary.total_staking_emissions.to_string();
            let power_str = summary.total_staking_power.to_string();
            EpochStakingSummary {
                epoch: summary.epoch,
                total_staked: total_str.clone(),
                total_staked_formatted: format_zkc(&total_str),
                num_stakers: summary.num_stakers,
                num_withdrawing: summary.num_withdrawing,
                total_staking_emissions: emissions_str.clone(),
                total_staking_emissions_formatted: format_zkc(&emissions_str),
                total_staking_power: power_str.clone(),
                total_staking_power_formatted: format_zkc(&power_str),
                num_reward_recipients: summary.num_reward_recipients,
                epoch_start_time: summary.epoch_start_time,
                epoch_end_time: summary.epoch_end_time,
                last_updated_at: summary.updated_at,
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/staking/epochs/:epoch
/// Returns summary for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/staking/epochs/{epoch}",
    tag = "Staking",
    params(
        ("epoch" = u64, Path, description = "Epoch number")
    ),
    responses(
        (status = 200, description = "Epoch staking summary", body = EpochStakingSummary),
        (status = 404, description = "Epoch not found"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_epoch_summary(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
) -> Response {
    match get_staking_epoch_summary_impl(state, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_epoch_summary_impl(
    state: Arc<AppState>,
    epoch: u64,
) -> anyhow::Result<EpochStakingSummary> {
    tracing::debug!("Fetching staking summary for epoch {}", epoch);

    // Fetch epoch summary
    let summary = state
        .rewards_db
        .get_epoch_staking_summary(epoch)
        .await?
        .ok_or_else(|| anyhow::anyhow!("No staking data available for epoch {}", epoch))?;

    let total_str = summary.total_staked.to_string();
    let emissions_str = summary.total_staking_emissions.to_string();
    let power_str = summary.total_staking_power.to_string();

    Ok(EpochStakingSummary {
        epoch: summary.epoch,
        total_staked: total_str.clone(),
        total_staked_formatted: format_zkc(&total_str),
        num_stakers: summary.num_stakers,
        num_withdrawing: summary.num_withdrawing,
        total_staking_emissions: emissions_str.clone(),
        total_staking_emissions_formatted: format_zkc(&emissions_str),
        total_staking_power: power_str.clone(),
        total_staking_power_formatted: format_zkc(&power_str),
        num_reward_recipients: summary.num_reward_recipients,
        epoch_start_time: summary.epoch_start_time,
        epoch_end_time: summary.epoch_end_time,
        last_updated_at: summary.updated_at,
    })
}

/// GET /v1/staking/epochs/:epoch/addresses
/// Returns the staking leaderboard for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/staking/epochs/{epoch}/addresses",
    tag = "Staking",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Epoch staking leaderboard", body = LeaderboardResponse<EpochStakingEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_epoch_leaderboard(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_staking_epoch_leaderboard_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_epoch_leaderboard_impl(
    state: Arc<AppState>,
    epoch: u64,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochStakingEntry>> {
    tracing::debug!(
        "Fetching staking leaderboard for epoch {} with offset={}, limit={}",
        epoch,
        params.offset,
        params.limit
    );

    // Fetch data from database
    let positions =
        state.rewards_db.get_staking_positions_by_epoch(epoch, params.offset, params.limit).await?;

    // Convert to response format with ranks
    let entries: Vec<EpochStakingEntry> = positions
        .into_iter()
        .enumerate()
        .map(|(index, position)| {
            let staked_str = position.staked_amount.to_string();
            let generated_str = position.rewards_generated.to_string();
            EpochStakingEntry {
                rank: Some(params.offset + (index as u64) + 1),
                staker_address: format!("{:#x}", position.staker_address),
                epoch: position.epoch,
                staked_amount: staked_str.clone(),
                staked_amount_formatted: format_zkc(&staked_str),
                is_withdrawing: position.is_withdrawing,
                rewards_delegated_to: position
                    .rewards_delegated_to
                    .map(|addr| format!("{:#x}", addr)),
                votes_delegated_to: position.votes_delegated_to.map(|addr| format!("{:#x}", addr)),
                rewards_generated: generated_str.clone(),
                rewards_generated_formatted: format_zkc(&generated_str),
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/staking/epochs/:epoch/addresses/:address
/// Returns staking data for a specific address at a specific epoch
#[utoipa::path(
    get,
    path = "/v1/staking/epochs/{epoch}/addresses/{address}",
    tag = "Staking",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        ("address" = String, Path, description = "Ethereum address")
    ),
    responses(
        (status = 200, description = "Staking position for address at epoch", body = Option<EpochStakingEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_address_at_epoch(
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

    match get_staking_address_at_epoch_impl(state, epoch, address).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_address_at_epoch_impl(
    state: Arc<AppState>,
    epoch: u64,
    address: Address,
) -> anyhow::Result<Option<EpochStakingEntry>> {
    tracing::debug!("Fetching staking data for address {} at epoch {}", address, epoch);

    // Fetch staking history for the address at specific epoch
    let positions =
        state.rewards_db.get_staking_history_by_address(address, Some(epoch), Some(epoch)).await?;

    if positions.is_empty() {
        return Ok(None);
    }

    let position = &positions[0];
    let staked_str = position.staked_amount.to_string();
    let generated_str = position.rewards_generated.to_string();
    Ok(Some(EpochStakingEntry {
        rank: None, // No rank for individual queries
        staker_address: format!("{:#x}", position.staker_address),
        epoch: position.epoch,
        staked_amount: staked_str.clone(),
        staked_amount_formatted: format_zkc(&staked_str),
        is_withdrawing: position.is_withdrawing,
        rewards_delegated_to: position.rewards_delegated_to.map(|addr| format!("{:#x}", addr)),
        votes_delegated_to: position.votes_delegated_to.map(|addr| format!("{:#x}", addr)),
        rewards_generated: generated_str.clone(),
        rewards_generated_formatted: format_zkc(&generated_str),
    }))
}

/// GET /v1/staking/addresses
/// Returns the all-time staking leaderboard
#[utoipa::path(
    get,
    path = "/v1/staking/addresses",
    tag = "Staking",
    params(PaginationParams),
    responses(
        (status = 200, description = "Staking leaderboard", body = LeaderboardResponse<AggregateStakingEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_all_time_leaderboard(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_staking_all_time_leaderboard_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_all_time_leaderboard_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<AggregateStakingEntry>> {
    tracing::debug!(
        "Fetching all-time staking leaderboard with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch aggregate data from database
    let aggregates =
        state.rewards_db.get_staking_positions_aggregate(params.offset, params.limit).await?;

    // Convert to response format with ranks
    let entries: Vec<AggregateStakingEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, aggregate)| {
            let staked_str = aggregate.total_staked.to_string();
            let generated_str = aggregate.total_rewards_generated.to_string();
            AggregateStakingEntry {
                rank: Some(params.offset + (index as u64) + 1),
                staker_address: format!("{:#x}", aggregate.staker_address),
                total_staked: staked_str.clone(),
                total_staked_formatted: format_zkc(&staked_str),
                is_withdrawing: aggregate.is_withdrawing,
                rewards_delegated_to: aggregate
                    .rewards_delegated_to
                    .map(|addr| format!("{:#x}", addr)),
                votes_delegated_to: aggregate.votes_delegated_to.map(|addr| format!("{:#x}", addr)),
                epochs_participated: aggregate.epochs_participated,
                total_rewards_generated: generated_str.clone(),
                total_rewards_generated_formatted: format_zkc(&generated_str),
            }
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/staking/addresses/:address
/// Returns the staking history for a specific address
#[utoipa::path(
    get,
    path = "/v1/staking/addresses/{address}",
    tag = "Staking",
    params(
        ("address" = String, Path, description = "Ethereum address"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Address staking history", body = AddressLeaderboardResponse<EpochStakingEntry, StakingAddressSummary>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_staking_address_history(
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

    match get_staking_address_history_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_address_history_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<AddressLeaderboardResponse<EpochStakingEntry, StakingAddressSummary>> {
    tracing::debug!(
        "Fetching staking history for address {} with offset={}, limit={}",
        address,
        params.offset,
        params.limit
    );

    // Fetch staking history for the address
    let positions = state.rewards_db.get_staking_history_by_address(address, None, None).await?;

    // Fetch aggregate summary for this address
    let address_aggregate =
        state.rewards_db.get_staking_position_aggregate_by_address(address).await?;

    // Apply pagination
    let start = params.offset as usize;
    let end = (start + params.limit as usize).min(positions.len());
    let paginated = if start < positions.len() { positions[start..end].to_vec() } else { vec![] };

    // Convert to response format
    let entries: Vec<EpochStakingEntry> = paginated
        .into_iter()
        .map(|position| {
            let staked_str = position.staked_amount.to_string();
            let generated_str = position.rewards_generated.to_string();
            EpochStakingEntry {
                rank: None, // No rank for individual address queries
                staker_address: format!("{:#x}", position.staker_address),
                epoch: position.epoch,
                staked_amount: staked_str.clone(),
                staked_amount_formatted: format_zkc(&staked_str),
                is_withdrawing: position.is_withdrawing,
                rewards_delegated_to: position
                    .rewards_delegated_to
                    .map(|addr| format!("{:#x}", addr)),
                votes_delegated_to: position.votes_delegated_to.map(|addr| format!("{:#x}", addr)),
                rewards_generated: generated_str.clone(),
                rewards_generated_formatted: format_zkc(&generated_str),
            }
        })
        .collect();

    // Create summary from aggregate if available, otherwise use default
    let summary = if let Some(aggregate) = address_aggregate {
        let staked_str = aggregate.total_staked.to_string();
        let generated_str = aggregate.total_rewards_generated.to_string();
        StakingAddressSummary {
            staker_address: format!("{:#x}", aggregate.staker_address),
            total_staked: staked_str.clone(),
            total_staked_formatted: format_zkc(&staked_str),
            is_withdrawing: aggregate.is_withdrawing,
            rewards_delegated_to: aggregate.rewards_delegated_to.map(|addr| format!("{:#x}", addr)),
            votes_delegated_to: aggregate.votes_delegated_to.map(|addr| format!("{:#x}", addr)),
            epochs_participated: aggregate.epochs_participated,
            total_rewards_generated: generated_str.clone(),
            total_rewards_generated_formatted: format_zkc(&generated_str),
        }
    } else {
        // No data for this address - return empty summary
        StakingAddressSummary {
            staker_address: format!("{:#x}", address),
            total_staked: "0".to_string(),
            total_staked_formatted: format_zkc("0"),
            is_withdrawing: false,
            rewards_delegated_to: None,
            votes_delegated_to: None,
            epochs_participated: 0,
            total_rewards_generated: "0".to_string(),
            total_rewards_generated_formatted: format_zkc("0"),
        }
    };

    Ok(AddressLeaderboardResponse::new(entries, params.offset, params.limit, summary))
}
