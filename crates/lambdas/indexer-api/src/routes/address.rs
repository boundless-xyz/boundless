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
use std::{collections::HashMap, str::FromStr, sync::Arc};

use crate::{
    db::AppState,
    handler::{cache_control, handle_error},
    models::{
        AddressEpochHistory, AddressHistoryParams, AddressHistoryResponse, DelegationPowerData,
        PovwRewardsData, StakingPositionData,
    },
};

/// Create address-specific routes
pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/:address/history", get(get_address_history))
}

/// GET /v1/rewards/address/:address/history
/// Returns complete history for an address across epochs
async fn get_address_history(
    State(state): State<Arc<AppState>>,
    Path(address_str): Path<String>,
    Query(params): Query<AddressHistoryParams>,
) -> Response {
    // Parse and validate address
    let address = match Address::from_str(&address_str) {
        Ok(addr) => addr,
        Err(e) => {
            return handle_error(anyhow::anyhow!("Invalid address format: {}", e)).into_response()
        }
    };

    let params = params.validate();

    match get_address_history_impl(state, address, params).await {
        Ok(response) => {
            let mut res = Json(response).into_response();
            // Cache for 5 minutes (historical data doesn't change frequently)
            res.headers_mut().insert(header::CACHE_CONTROL, cache_control("public, max-age=300"));
            res
        }
        Err(err) => handle_error(err).into_response(),
    }
}

async fn get_address_history_impl(
    state: Arc<AppState>,
    address: Address,
    params: AddressHistoryParams,
) -> anyhow::Result<AddressHistoryResponse> {
    tracing::debug!(
        "Fetching history for address {} with start_epoch={:?}, end_epoch={:?}, limit={}",
        address,
        params.start_epoch,
        params.end_epoch,
        params.limit
    );

    // Fetch all data types in parallel for efficiency
    let (staking_history, povw_history, vote_delegations, reward_delegations) = tokio::try_join!(
        state.rewards_db.get_staking_history_by_address(
            address,
            params.start_epoch,
            params.end_epoch
        ),
        state.rewards_db.get_povw_rewards_history_by_address(
            address,
            params.start_epoch,
            params.end_epoch
        ),
        state.rewards_db.get_vote_delegations_received_history(
            address,
            params.start_epoch,
            params.end_epoch
        ),
        state.rewards_db.get_reward_delegations_received_history(
            address,
            params.start_epoch,
            params.end_epoch
        )
    )?;

    // Build a map of epochs to efficiently combine data
    let mut epochs_map: HashMap<u64, AddressEpochHistory> = HashMap::new();

    // Add staking data
    for position in staking_history {
        epochs_map
            .entry(position.epoch)
            .or_insert_with(|| AddressEpochHistory {
                epoch: position.epoch,
                staking: None,
                povw_rewards: None,
                vote_delegations_received: None,
                reward_delegations_received: None,
            })
            .staking = Some(StakingPositionData {
            staked_amount: position.staked_amount.to_string(),
            is_withdrawing: position.is_withdrawing,
            rewards_delegated_to: position.rewards_delegated_to.map(|a| format!("{:#x}", a)),
            votes_delegated_to: position.votes_delegated_to.map(|a| format!("{:#x}", a)),
            rewards_generated: position.rewards_generated.to_string(),
        });
    }

    // Add PoVW rewards data
    for reward in povw_history {
        epochs_map
            .entry(reward.epoch)
            .or_insert_with(|| AddressEpochHistory {
                epoch: reward.epoch,
                staking: None,
                povw_rewards: None,
                vote_delegations_received: None,
                reward_delegations_received: None,
            })
            .povw_rewards = Some(PovwRewardsData {
            work_submitted: reward.work_submitted.to_string(),
            percentage: reward.percentage,
            uncapped_rewards: reward.uncapped_rewards.to_string(),
            reward_cap: reward.reward_cap.to_string(),
            actual_rewards: reward.actual_rewards.to_string(),
            is_capped: reward.is_capped,
            staked_amount: reward.staked_amount.to_string(),
        });
    }

    // Add vote delegation data
    for delegation in vote_delegations {
        epochs_map
            .entry(delegation.epoch)
            .or_insert_with(|| AddressEpochHistory {
                epoch: delegation.epoch,
                staking: None,
                povw_rewards: None,
                vote_delegations_received: None,
                reward_delegations_received: None,
            })
            .vote_delegations_received = Some(DelegationPowerData {
            power: delegation.vote_power.to_string(),
            delegator_count: delegation.delegator_count,
            delegators: delegation.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
        });
    }

    // Add reward delegation data
    for delegation in reward_delegations {
        epochs_map
            .entry(delegation.epoch)
            .or_insert_with(|| AddressEpochHistory {
                epoch: delegation.epoch,
                staking: None,
                povw_rewards: None,
                vote_delegations_received: None,
                reward_delegations_received: None,
            })
            .reward_delegations_received = Some(DelegationPowerData {
            power: delegation.reward_power.to_string(),
            delegator_count: delegation.delegator_count,
            delegators: delegation.delegators.iter().map(|a| format!("{:#x}", a)).collect(),
        });
    }

    // Convert map to sorted vector (by epoch descending) and apply limit
    let mut epochs: Vec<AddressEpochHistory> = epochs_map.into_values().collect();
    epochs.sort_by(|a, b| b.epoch.cmp(&a.epoch));
    epochs.truncate(params.limit as usize);

    let epoch_count = epochs.len();

    Ok(AddressHistoryResponse { address: format!("{:#x}", address), epochs, epoch_count })
}
