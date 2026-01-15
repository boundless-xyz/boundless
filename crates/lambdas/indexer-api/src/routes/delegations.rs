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
    models::{DelegationPowerEntry, LeaderboardResponse, PaginationParams},
};

/// Create delegation routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Vote delegation endpoints
        .route("/votes/epochs/:epoch/addresses", get(get_vote_delegations_by_epoch))
        .route(
            "/votes/epochs/:epoch/addresses/:address",
            get(get_vote_delegation_by_address_and_epoch),
        )
        .route("/votes/addresses", get(get_aggregate_vote_delegations))
        .route("/votes/addresses/:address", get(get_vote_delegation_history_by_address))
        // Reward delegation endpoints
        .route("/rewards/epochs/:epoch/addresses", get(get_reward_delegations_by_epoch))
        .route(
            "/rewards/epochs/:epoch/addresses/:address",
            get(get_reward_delegation_by_address_and_epoch),
        )
        .route("/rewards/addresses", get(get_aggregate_reward_delegations))
        .route("/rewards/addresses/:address", get(get_reward_delegation_history_by_address))
}

// ===== VOTE DELEGATION ENDPOINTS =====

/// GET /v1/delegations/votes/addresses
/// Returns the current aggregate vote delegation powers
#[utoipa::path(
    get,
    path = "/v1/delegations/votes/addresses",
    tag = "Delegations",
    params(
        PaginationParams
    ),
    responses(
        (status = 200, description = "Aggregate vote delegation powers", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_aggregate_vote_delegations(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_aggregate_vote_delegations_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_aggregate_vote_delegations_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching aggregate vote delegation powers with offset={}, limit={}",
        params.offset,
        params.limit
    );

    let aggregates =
        state.rewards_db.get_vote_delegation_powers_aggregate(params.offset, params.limit).await?;

    let entries: Vec<DelegationPowerEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, agg)| DelegationPowerEntry {
            rank: Some(params.offset + (index as u64) + 1),
            delegate_address: format!("{:#x}", agg.delegate_address),
            power: agg.total_vote_power.to_string(),
            delegator_count: agg.delegator_count,
            delegators: agg.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: Some(agg.epochs_participated),
            epoch: None,
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/votes/epochs/:epoch/addresses
/// Returns vote delegation powers for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/delegations/votes/epochs/{epoch}/addresses",
    tag = "Delegations",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Vote delegation powers for epoch", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_vote_delegations_by_epoch(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_vote_delegations_by_epoch_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_vote_delegations_by_epoch_impl(
    state: Arc<AppState>,
    epoch: u64,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching vote delegation powers for epoch {} with offset={}, limit={}",
        epoch,
        params.offset,
        params.limit
    );

    let powers = state
        .rewards_db
        .get_vote_delegation_powers_by_epoch(epoch, params.offset, params.limit)
        .await?;

    let entries: Vec<DelegationPowerEntry> = powers
        .into_iter()
        .enumerate()
        .map(|(index, power)| DelegationPowerEntry {
            rank: Some(params.offset + (index as u64) + 1),
            delegate_address: format!("{:#x}", power.delegate_address),
            power: power.vote_power.to_string(),
            delegator_count: power.delegator_count,
            delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: None,
            epoch: Some(epoch),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/votes/addresses/:address
/// Returns vote delegation history for a specific address
#[utoipa::path(
    get,
    path = "/v1/delegations/votes/addresses/{address}",
    tag = "Delegations",
    params(
        ("address" = String, Path, description = "Ethereum address"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Vote delegation history for address", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_vote_delegation_history_by_address(
    State(state): State<Arc<AppState>>,
    Path(address_str): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    let params = params.validate();

    match get_vote_delegation_history_by_address_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_vote_delegation_history_by_address_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching vote delegation history for address {} with offset={}, limit={}",
        address,
        params.offset,
        params.limit
    );

    let history =
        state.rewards_db.get_vote_delegations_received_history(address, None, None).await?;

    // Apply pagination
    let start = params.offset as usize;
    let end = (start + params.limit as usize).min(history.len());
    let paginated = if start < history.len() { history[start..end].to_vec() } else { vec![] };

    let entries: Vec<DelegationPowerEntry> = paginated
        .into_iter()
        .map(|power| DelegationPowerEntry {
            rank: None,
            delegate_address: format!("{:#x}", power.delegate_address),
            power: power.vote_power.to_string(),
            delegator_count: power.delegator_count,
            delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: None,
            epoch: Some(power.epoch),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/votes/epochs/:epoch/addresses/:address
/// Returns vote delegation for a specific address at a specific epoch
#[utoipa::path(
    get,
    path = "/v1/delegations/votes/epochs/{epoch}/addresses/{address}",
    tag = "Delegations",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        ("address" = String, Path, description = "Ethereum address")
    ),
    responses(
        (status = 200, description = "Vote delegation for address at epoch", body = Option<DelegationPowerEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_vote_delegation_by_address_and_epoch(
    State(state): State<Arc<AppState>>,
    Path((epoch, address_str)): Path<(u64, String)>,
) -> Response {
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    match get_vote_delegation_by_address_and_epoch_impl(state, address, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_vote_delegation_by_address_and_epoch_impl(
    state: Arc<AppState>,
    address: Address,
    epoch: u64,
) -> anyhow::Result<Option<DelegationPowerEntry>> {
    tracing::debug!("Fetching vote delegation for address {} at epoch {}", address, epoch);

    let history = state
        .rewards_db
        .get_vote_delegations_received_history(address, Some(epoch), Some(epoch))
        .await?;

    if history.is_empty() {
        return Ok(None);
    }

    let power = &history[0];
    Ok(Some(DelegationPowerEntry {
        rank: None,
        delegate_address: format!("{:#x}", power.delegate_address),
        power: power.vote_power.to_string(),
        delegator_count: power.delegator_count,
        delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
        epochs_participated: None,
        epoch: Some(power.epoch),
    }))
}

// ===== REWARD DELEGATION ENDPOINTS =====

/// GET /v1/delegations/rewards/addresses
/// Returns the current aggregate reward delegation powers
#[utoipa::path(
    get,
    path = "/v1/delegations/rewards/addresses",
    tag = "Delegations",
    params(
        PaginationParams
    ),
    responses(
        (status = 200, description = "Aggregate reward delegation powers", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_aggregate_reward_delegations(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_aggregate_reward_delegations_impl(state, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=60"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_aggregate_reward_delegations_impl(
    state: Arc<AppState>,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching aggregate reward delegation powers with offset={}, limit={}",
        params.offset,
        params.limit
    );

    let aggregates = state
        .rewards_db
        .get_reward_delegation_powers_aggregate(params.offset, params.limit)
        .await?;

    let entries: Vec<DelegationPowerEntry> = aggregates
        .into_iter()
        .enumerate()
        .map(|(index, agg)| DelegationPowerEntry {
            rank: Some(params.offset + (index as u64) + 1),
            delegate_address: format!("{:#x}", agg.delegate_address),
            power: agg.total_reward_power.to_string(),
            delegator_count: agg.delegator_count,
            delegators: agg.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: Some(agg.epochs_participated),
            epoch: None,
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/rewards/epochs/:epoch/addresses
/// Returns reward delegation powers for a specific epoch
#[utoipa::path(
    get,
    path = "/v1/delegations/rewards/epochs/{epoch}/addresses",
    tag = "Delegations",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Reward delegation powers for epoch", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_reward_delegations_by_epoch(
    State(state): State<Arc<AppState>>,
    Path(epoch): Path<u64>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let params = params.validate();

    match get_reward_delegations_by_epoch_impl(state, epoch, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_reward_delegations_by_epoch_impl(
    state: Arc<AppState>,
    epoch: u64,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching reward delegation powers for epoch {} with offset={}, limit={}",
        epoch,
        params.offset,
        params.limit
    );

    let powers = state
        .rewards_db
        .get_reward_delegation_powers_by_epoch(epoch, params.offset, params.limit)
        .await?;

    let entries: Vec<DelegationPowerEntry> = powers
        .into_iter()
        .enumerate()
        .map(|(index, power)| DelegationPowerEntry {
            rank: Some(params.offset + (index as u64) + 1),
            delegate_address: format!("{:#x}", power.delegate_address),
            power: power.reward_power.to_string(),
            delegator_count: power.delegator_count,
            delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: None,
            epoch: Some(epoch),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/rewards/addresses/:address
/// Returns reward delegation history for a specific address
#[utoipa::path(
    get,
    path = "/v1/delegations/rewards/addresses/{address}",
    tag = "Delegations",
    params(
        ("address" = String, Path, description = "Ethereum address"),
        PaginationParams
    ),
    responses(
        (status = 200, description = "Reward delegation history for address", body = LeaderboardResponse<DelegationPowerEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_reward_delegation_history_by_address(
    State(state): State<Arc<AppState>>,
    Path(address_str): Path<String>,
    Query(params): Query<PaginationParams>,
) -> Response {
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    let params = params.validate();

    match get_reward_delegation_history_by_address_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_reward_delegation_history_by_address_impl(
    state: Arc<AppState>,
    address: Address,
    params: PaginationParams,
) -> anyhow::Result<LeaderboardResponse<DelegationPowerEntry>> {
    tracing::debug!(
        "Fetching reward delegation history for address {} with offset={}, limit={}",
        address,
        params.offset,
        params.limit
    );

    let history =
        state.rewards_db.get_reward_delegations_received_history(address, None, None).await?;

    // Apply pagination
    let start = params.offset as usize;
    let end = (start + params.limit as usize).min(history.len());
    let paginated = if start < history.len() { history[start..end].to_vec() } else { vec![] };

    let entries: Vec<DelegationPowerEntry> = paginated
        .into_iter()
        .map(|power| DelegationPowerEntry {
            rank: None,
            delegate_address: format!("{:#x}", power.delegate_address),
            power: power.reward_power.to_string(),
            delegator_count: power.delegator_count,
            delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
            epochs_participated: None,
            epoch: Some(power.epoch),
        })
        .collect();

    Ok(LeaderboardResponse::new(entries, params.offset, params.limit))
}

/// GET /v1/delegations/rewards/epochs/:epoch/addresses/:address
/// Returns reward delegation for a specific address at a specific epoch
#[utoipa::path(
    get,
    path = "/v1/delegations/rewards/epochs/{epoch}/addresses/{address}",
    tag = "Delegations",
    params(
        ("epoch" = u64, Path, description = "Epoch number"),
        ("address" = String, Path, description = "Ethereum address")
    ),
    responses(
        (status = 200, description = "Reward delegation for address at epoch", body = Option<DelegationPowerEntry>),
        (status = 400, description = "Invalid address format"),
        (status = 500, description = "Internal server error")
    )
)]
async fn get_reward_delegation_by_address_and_epoch(
    State(state): State<Arc<AppState>>,
    Path((epoch, address_str)): Path<(u64, String)>,
) -> Response {
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    match get_reward_delegation_by_address_and_epoch_impl(state, address, epoch).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_reward_delegation_by_address_and_epoch_impl(
    state: Arc<AppState>,
    address: Address,
    epoch: u64,
) -> anyhow::Result<Option<DelegationPowerEntry>> {
    tracing::debug!("Fetching reward delegation for address {} at epoch {}", address, epoch);

    let history = state
        .rewards_db
        .get_reward_delegations_received_history(address, Some(epoch), Some(epoch))
        .await?;

    if history.is_empty() {
        return Ok(None);
    }

    let power = &history[0];
    Ok(Some(DelegationPowerEntry {
        rank: None,
        delegate_address: format!("{:#x}", power.delegate_address),
        power: power.reward_power.to_string(),
        delegator_count: power.delegator_count,
        delegators: power.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
        epochs_participated: None,
        epoch: Some(power.epoch),
    }))
}
