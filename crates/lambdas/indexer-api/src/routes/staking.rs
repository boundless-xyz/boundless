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
    handler::handle_error,
    models::{AggregateStakingEntry, EpochStakingEntry, LeaderboardResponse, PaginationParams},
};

/// Create staking routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Aggregate endpoints
        .route("/", get(get_aggregate_staking))
        // Epoch-specific endpoints
        .route("/epochs/:epoch", get(get_staking_by_epoch))
        // Address-specific endpoints
        .route("/addresses/:address", get(get_staking_history_by_address))
        .route("/addresses/:address/epochs/:epoch", get(get_staking_by_address_and_epoch))
}

/// GET /v1/staking
/// Returns the aggregate staking leaderboard
async fn get_aggregate_staking(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_aggregate_staking_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Cache for 60 seconds (current leaderboard updates frequently)
            res.headers_mut().insert(header::CACHE_CONTROL, "public, max-age=60".parse().unwrap());
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_aggregate_staking_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<AggregateStakingEntry>> {
    tracing::debug!(
        "Fetching aggregate staking leaderboard with offset={}, limit={}",
        params.offset,
        params.limit
    );

    // Fetch data from database
    let aggregates =
        state.rewards_db.get_staking_positions_aggregate(params.offset, params.limit).await?;

    // Convert to response format with ranks
    let entries: Vec<AggregateStakingEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, agg)| AggregateStakingEntry {
            rank: params.offset + (index as u64) + 1,
            staker_address: format!("{:#x}", agg.staker_address),
            total_staked: agg.total_staked.to_string(),
            is_withdrawing: agg.is_withdrawing,
            rewards_delegated_to: agg.rewards_delegated_to.map(|a| format!("{:#x}", a)),
            votes_delegated_to: agg.votes_delegated_to.map(|a| format!("{:#x}", a)),
            epochs_participated: agg.epochs_participated,
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/staking/epochs/:epoch
/// Returns the staking positions for a specific epoch
async fn get_staking_by_epoch(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_staking_by_epoch_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Cache for 5 minutes (historical epochs don't change)
            res.headers_mut().insert(header::CACHE_CONTROL, "public, max-age=300".parse().unwrap());
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_by_epoch_impl(
    state: Arc<AppState>,
    epoch: u64,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochStakingEntry>> {
    tracing::debug!(
        "Fetching epoch {} staking leaderboard with offset={}, limit={}",
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
        .map(|(index, position)| EpochStakingEntry {
            rank: params.offset + (index as u64) + 1,
            staker_address: format!("{:#x}", position.staker_address),
            epoch: position.epoch,
            staked_amount: position.staked_amount.to_string(),
            is_withdrawing: position.is_withdrawing,
            rewards_delegated_to: position.rewards_delegated_to.map(|a| format!("{:#x}", a)),
            votes_delegated_to: position.votes_delegated_to.map(|a| format!("{:#x}", a)),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}
/// GET /v1/staking/addresses/:address
/// Returns the staking history for a specific address
async fn get_staking_history_by_address(
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

    match get_staking_history_by_address_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, "public, max-age=300".parse().unwrap());
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_history_by_address_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<EpochStakingEntry>> {
    tracing::debug!(
        "Fetching staking history for address {} with offset={}, limit={}",
        address,
        params.offset,
        params.limit
    );

    // Fetch staking history for the address
    let positions = state.rewards_db.get_staking_history_by_address(address, None, None).await?;

    // Apply pagination
    let start = params.offset as usize;
    let end = (start + params.limit as usize).min(positions.len());
    let paginated = if start < positions.len() { positions[start..end].to_vec() } else { vec![] };

    // Convert to response format
    let entries: Vec<EpochStakingEntry> = paginated
        .into_iter()
        .enumerate()
        .map(|(index, position)| EpochStakingEntry {
            rank: params.offset + (index as u64) + 1,
            staker_address: format!("{:#x}", position.staker_address),
            epoch: position.epoch,
            staked_amount: position.staked_amount.to_string(),
            is_withdrawing: position.is_withdrawing,
            rewards_delegated_to: position.rewards_delegated_to.map(|a| format!("{:#x}", a)),
            votes_delegated_to: position.votes_delegated_to.map(|a| format!("{:#x}", a)),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/staking/addresses/:address/epochs/:epoch
/// Returns the staking position for a specific address at a specific epoch
async fn get_staking_by_address_and_epoch(
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

    match get_staking_by_address_and_epoch_impl(state, address, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, "public, max-age=300".parse().unwrap());
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_staking_by_address_and_epoch_impl(
    state: Arc<AppState>,
    address: Address,
    epoch: u64,
) -> anyhow::Result<Option<EpochStakingEntry>> {
    tracing::debug!("Fetching staking position for address {} at epoch {}", address, epoch);

    // Fetch staking history for the address at specific epoch
    let positions =
        state.rewards_db.get_staking_history_by_address(address, Some(epoch), Some(epoch)).await?;

    if positions.is_empty() {
        return Ok(None);
    }

    let position = &positions[0];
    Ok(Some(EpochStakingEntry {
        rank: 0, // No rank for individual queries
        staker_address: format!("{:#x}", position.staker_address),
        epoch: position.epoch,
        staked_amount: position.staked_amount.to_string(),
        is_withdrawing: position.is_withdrawing,
        rewards_delegated_to: position.rewards_delegated_to.map(|a| format!("{:#x}", a)),
        votes_delegated_to: position.votes_delegated_to.map(|a| format!("{:#x}", a)),
    }))
}
