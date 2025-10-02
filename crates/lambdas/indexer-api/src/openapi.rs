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

use crate::models::*;
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Boundless Indexer API",
        version = "1.0.0",
        description = "API for accessing staking, delegation, and Proof of Verifiable Work (PoVW) data for the Boundless protocol.",
        contact(name = "Boundless Development Team")
    ),
    servers(
        (url = "/", description = "Current server")
    ),
    tags(
        (name = "Health", description = "Health check endpoints"),
        (name = "Staking", description = "Staking position and history endpoints"),
        (name = "PoVW", description = "Proof of Verifiable Work rewards endpoints"),
        (name = "Delegations", description = "Vote and reward delegation endpoints")
    ),
    paths(
        // Health check
        crate::handler::health_check,
        // Staking endpoints
        crate::routes::staking::get_staking_summary,
        crate::routes::staking::get_all_epochs_summary,
        crate::routes::staking::get_epoch_summary,
        crate::routes::staking::get_epoch_leaderboard,
        crate::routes::staking::get_address_at_epoch,
        crate::routes::staking::get_all_time_leaderboard,
        crate::routes::staking::get_address_history,
        // PoVW endpoints
        crate::routes::povw::get_povw_summary,
        crate::routes::povw::get_all_epochs_summary,
        crate::routes::povw::get_epoch_summary,
        crate::routes::povw::get_epoch_leaderboard,
        crate::routes::povw::get_address_at_epoch,
        crate::routes::povw::get_all_time_leaderboard,
        crate::routes::povw::get_address_history,
        // Delegation endpoints - Votes
        crate::routes::delegations::get_aggregate_vote_delegations,
        crate::routes::delegations::get_vote_delegations_by_epoch,
        crate::routes::delegations::get_vote_delegation_history_by_address,
        crate::routes::delegations::get_vote_delegation_by_address_and_epoch,
        // Delegation endpoints - Rewards
        crate::routes::delegations::get_aggregate_reward_delegations,
        crate::routes::delegations::get_reward_delegations_by_epoch,
        crate::routes::delegations::get_reward_delegation_history_by_address,
        crate::routes::delegations::get_reward_delegation_by_address_and_epoch,
    ),
    components(schemas(
        // Response models
        StakingSummaryStats,
        PoVWSummaryStats,
        LeaderboardResponse<AggregateStakingEntry>,
        LeaderboardResponse<EpochStakingEntry>,
        LeaderboardResponse<AggregateLeaderboardEntry>,
        LeaderboardResponse<EpochLeaderboardEntry>,
        AddressLeaderboardResponse<EpochStakingEntry, StakingAddressSummary>,
        AddressLeaderboardResponse<EpochLeaderboardEntry, PoVWAddressSummary>,

        // Entry types
        AggregateStakingEntry,
        EpochStakingEntry,
        AggregateLeaderboardEntry,
        EpochLeaderboardEntry,

        // Summary types
        StakingAddressSummary,
        PoVWAddressSummary,
        EpochStakingSummary,
        EpochPoVWSummary,

        // Pagination
        PaginationParams,
        PaginationMetadata,

        // Delegation types
        DelegationPowerEntry,
        EpochDelegationSummary,
        VoteDelegationSummaryStats,
        RewardDelegationSummaryStats,
    ))
)]
pub struct ApiDoc;
