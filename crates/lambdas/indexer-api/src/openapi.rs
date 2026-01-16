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
        (name = "Delegations", description = "Vote and reward delegation endpoints"),
        (name = "Market", description = "Market activity aggregates and statistics")
    ),
    paths(
        // Health check
        crate::handler::health_check,
        // Staking endpoints
        crate::routes::staking::get_staking_summary,
        crate::routes::staking::get_staking_all_epochs_summary,
        crate::routes::staking::get_staking_epoch_summary,
        crate::routes::staking::get_staking_epoch_leaderboard,
        crate::routes::staking::get_staking_address_at_epoch,
        crate::routes::staking::get_staking_all_time_leaderboard,
        crate::routes::staking::get_staking_address_history,
        // PoVW endpoints
        crate::routes::povw::get_povw_summary,
        crate::routes::povw::get_povw_all_epochs_summary,
        crate::routes::povw::get_povw_epoch_summary,
        crate::routes::povw::get_povw_epoch_leaderboard,
        crate::routes::povw::get_povw_address_at_epoch,
        crate::routes::povw::get_povw_all_time_leaderboard,
        crate::routes::povw::get_povw_address_history,
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
        // Market endpoints
        crate::routes::market::get_indexing_status,
        crate::routes::market::get_market_aggregates,
        crate::routes::market::get_market_cumulatives,
        crate::routes::market::list_requests,
        crate::routes::market::get_requests_by_request_id,
        crate::routes::market::list_requestors,
        crate::routes::market::list_provers,
        crate::routes::market::list_requests_by_requestor,
        crate::routes::market::get_requestor_aggregates,
        crate::routes::market::get_requestor_cumulatives,
        crate::routes::market::list_requests_by_prover,
        crate::routes::market::get_prover_aggregates,
        crate::routes::market::get_prover_cumulatives,
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

        // Market types
        crate::routes::market::IndexingStatusResponse,
        crate::routes::market::MarketAggregatesParams,
        crate::routes::market::MarketAggregateEntry,
        crate::routes::market::MarketAggregatesResponse,
        crate::routes::market::MarketCumulativesParams,
        crate::routes::market::MarketCumulativeEntry,
        crate::routes::market::MarketCumulativesResponse,
        crate::routes::market::RequestorAggregatesParams,
        crate::routes::market::RequestorAggregateEntry,
        crate::routes::market::RequestorAggregatesResponse,
        crate::routes::market::RequestorCumulativesParams,
        crate::routes::market::RequestorCumulativeEntry,
        crate::routes::market::RequestorCumulativesResponse,
        crate::routes::market::RequestListParams,
        crate::routes::market::RequestStatusResponse,
        crate::routes::market::RequestListResponse,
        crate::routes::market::AggregationGranularity,
        crate::routes::market::LeaderboardPeriod,
        crate::routes::market::RequestorLeaderboardParams,
        crate::routes::market::RequestorLeaderboardEntry,
        crate::routes::market::RequestorLeaderboardResponse,
        crate::routes::market::RequestorLeaderboardCursor,
        crate::routes::market::ProverLeaderboardEntry,
        crate::routes::market::ProverLeaderboardResponse,
        crate::routes::market::ProverLeaderboardCursor,
        crate::routes::market::ProverAggregatesParams,
        crate::routes::market::ProverAggregateEntry,
        crate::routes::market::ProverAggregatesResponse,
        crate::routes::market::ProverCumulativesParams,
        crate::routes::market::ProverCumulativeEntry,
        crate::routes::market::ProverCumulativesResponse,
    ))
)]
pub struct ApiDoc;
