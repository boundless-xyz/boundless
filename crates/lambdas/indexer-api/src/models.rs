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

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Health check response
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub service: String,
}

/// Query parameters for pagination
#[derive(Debug, Deserialize, ToSchema, utoipa::IntoParams)]
pub struct PaginationParams {
    /// Number of results to return (default: 50, max: 100)
    #[serde(default = "default_limit")]
    pub limit: u64,

    /// Number of results to skip (default: 0)
    #[serde(default)]
    pub offset: u64,
}

fn default_limit() -> u64 {
    50
}

impl PaginationParams {
    /// Validate and normalize pagination parameters
    pub fn validate(self) -> Self {
        Self {
            limit: self.limit.min(100), // Cap at 100
            offset: self.offset,
        }
    }
}

/// Response for aggregate PoVW rewards leaderboard
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AggregateLeaderboardEntry {
    /// Rank in the leaderboard (1-based, only present in leaderboard contexts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,

    /// Work log ID (Ethereum address)
    pub work_log_id: String,

    /// Total work submitted across all epochs
    pub total_work_submitted: String,

    /// Total work submitted (human-readable)
    pub total_work_submitted_formatted: String,

    /// Total rewards earned across all epochs
    pub total_actual_rewards: String,

    /// Total rewards earned (human-readable)
    pub total_actual_rewards_formatted: String,

    /// Total uncapped rewards earned across all epochs
    pub total_uncapped_rewards: String,

    /// Total uncapped rewards (human-readable)
    pub total_uncapped_rewards_formatted: String,

    /// Number of epochs participated in
    pub epochs_participated: u64,
}

/// Response for epoch-specific PoVW rewards leaderboard
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EpochLeaderboardEntry {
    /// Rank in the leaderboard (1-based, only present in leaderboard contexts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,

    /// Work log ID (Ethereum address)
    pub work_log_id: String,

    /// Epoch number
    pub epoch: u64,

    /// Work submitted in this epoch
    pub work_submitted: String,

    /// Work submitted (human-readable)
    pub work_submitted_formatted: String,

    /// Percentage of total work in epoch
    pub percentage: f64,

    /// Rewards before applying cap
    pub uncapped_rewards: String,

    /// Uncapped rewards (human-readable)
    pub uncapped_rewards_formatted: String,

    /// Maximum rewards allowed based on stake
    pub reward_cap: String,

    /// Reward cap (human-readable)
    pub reward_cap_formatted: String,

    /// Actual rewards after applying cap
    pub actual_rewards: String,

    /// Actual rewards (human-readable)
    pub actual_rewards_formatted: String,

    /// Whether rewards were capped
    pub is_capped: bool,

    /// Staked amount for this work log
    pub staked_amount: String,

    /// Staked amount (human-readable)
    pub staked_amount_formatted: String,
}

/// Response wrapper for leaderboard endpoints
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LeaderboardResponse<T> {
    /// List of leaderboard entries
    pub entries: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMetadata,
}

/// Pagination metadata
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaginationMetadata {
    /// Number of results returned
    pub count: usize,

    /// Offset used
    pub offset: u64,

    /// Limit used
    pub limit: u64,
}

impl<T> LeaderboardResponse<T> {
    pub fn new(entries: Vec<T>, offset: u64, limit: u64) -> Self {
        let count = entries.len();
        Self { entries, pagination: PaginationMetadata { count, offset, limit } }
    }
}

/// Response wrapper for address-specific endpoints with summary
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AddressLeaderboardResponse<T, S> {
    /// List of history entries
    pub entries: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMetadata,

    /// Address summary statistics
    pub summary: S,
}

impl<T, S> AddressLeaderboardResponse<T, S> {
    pub fn new(entries: Vec<T>, offset: u64, limit: u64, summary: S) -> Self {
        let count = entries.len();
        Self { entries, pagination: PaginationMetadata { count, offset, limit }, summary }
    }
}

/// Response for aggregate staking leaderboard
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AggregateStakingEntry {
    /// Rank in the leaderboard (1-based, only present in leaderboard contexts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,

    /// Staker address
    pub staker_address: String,

    /// Total staked amount
    pub total_staked: String,

    /// Total staked (human-readable)
    pub total_staked_formatted: String,

    /// Whether the stake is in withdrawal
    pub is_withdrawing: bool,

    /// Address this staker has delegated rewards to
    pub rewards_delegated_to: Option<String>,

    /// Address this staker has delegated votes to
    pub votes_delegated_to: Option<String>,

    /// Number of epochs participated in
    pub epochs_participated: u64,

    /// Total rewards generated by owned positions
    pub total_rewards_generated: String,

    /// Total rewards generated (human-readable)
    pub total_rewards_generated_formatted: String,
}

/// Response for epoch-specific staking leaderboard
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EpochStakingEntry {
    /// Rank in the leaderboard (1-based, only present in leaderboard contexts)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,

    /// Staker address
    pub staker_address: String,

    /// Epoch number
    pub epoch: u64,

    /// Staked amount in this epoch
    pub staked_amount: String,

    /// Staked amount (human-readable)
    pub staked_amount_formatted: String,

    /// Whether the stake was in withdrawal during this epoch
    pub is_withdrawing: bool,

    /// Address this staker had delegated rewards to during this epoch
    pub rewards_delegated_to: Option<String>,

    /// Address this staker had delegated votes to during this epoch
    pub votes_delegated_to: Option<String>,

    /// Rewards generated by this position in this epoch
    pub rewards_generated: String,

    /// Rewards generated (human-readable)
    pub rewards_generated_formatted: String,
}

/// Global PoVW summary statistics
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct PoVWSummaryStats {
    pub total_epochs_with_work: u64,
    pub total_unique_work_log_ids: u64,
    pub total_work_all_time: String,
    pub total_work_all_time_formatted: String,
    pub total_emissions_all_time: String,
    pub total_emissions_all_time_formatted: String,
    pub total_capped_rewards_all_time: String,
    pub total_capped_rewards_all_time_formatted: String,
    pub total_uncapped_rewards_all_time: String,
    pub total_uncapped_rewards_all_time_formatted: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

/// Per-epoch PoVW summary
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct EpochPoVWSummary {
    pub epoch: u64,
    pub total_work: String,
    pub total_work_formatted: String,
    pub total_emissions: String,
    pub total_emissions_formatted: String,
    pub total_capped_rewards: String,
    pub total_capped_rewards_formatted: String,
    pub total_uncapped_rewards: String,
    pub total_uncapped_rewards_formatted: String,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    pub num_participants: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

/// Global staking summary statistics
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct StakingSummaryStats {
    pub current_total_staked: String,
    pub current_total_staked_formatted: String,
    pub total_unique_stakers: u64,
    pub current_active_stakers: u64,
    pub current_withdrawing: u64,
    pub total_staking_emissions_all_time: Option<String>,
    pub total_staking_emissions_all_time_formatted: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

/// Per-epoch staking summary
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct EpochStakingSummary {
    pub epoch: u64,
    pub total_staked: String,
    pub total_staked_formatted: String,
    pub num_stakers: u64,
    pub num_withdrawing: u64,
    pub total_staking_emissions: String,
    pub total_staking_emissions_formatted: String,
    pub total_staking_power: String,
    pub total_staking_power_formatted: String,
    pub num_reward_recipients: u64,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<String>,
}

/// Address-specific staking aggregate summary
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct StakingAddressSummary {
    pub staker_address: String,
    pub total_staked: String,
    pub total_staked_formatted: String,
    pub is_withdrawing: bool,
    pub rewards_delegated_to: Option<String>,
    pub votes_delegated_to: Option<String>,
    pub epochs_participated: u64,
    pub total_rewards_generated: String,
    pub total_rewards_generated_formatted: String,
}

/// Address-specific PoVW aggregate summary
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct PoVWAddressSummary {
    pub work_log_id: String,
    pub total_work_submitted: String,
    pub total_work_submitted_formatted: String,
    pub total_actual_rewards: String,
    pub total_actual_rewards_formatted: String,
    pub total_uncapped_rewards: String,
    pub total_uncapped_rewards_formatted: String,
    pub epochs_participated: u64,
}

/// Summary statistics for vote delegations
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct VoteDelegationSummaryStats {
    pub total_unique_delegates: u64,
    pub total_unique_delegators: u64,
    pub current_total_delegated_power: String,
    pub current_total_delegated_power_formatted: String,
    pub current_active_delegations: u64,
}

/// Summary statistics for reward delegations
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct RewardDelegationSummaryStats {
    pub total_unique_delegates: u64,
    pub total_unique_delegators: u64,
    pub current_total_delegated_power: String,
    pub current_total_delegated_power_formatted: String,
    pub current_active_delegations: u64,
}

/// Per-epoch delegation summary
#[derive(Debug, Serialize, Deserialize, Clone, ToSchema)]
pub struct EpochDelegationSummary {
    pub epoch: u64,
    pub total_delegated_power: String,
    pub total_delegated_power_formatted: String,
    pub num_delegates: u64,
    pub num_delegators: u64,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
}

/// Response for delegation power entries
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct DelegationPowerEntry {
    /// Rank in the leaderboard (1-based), None for individual queries
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rank: Option<u64>,

    /// Delegate address receiving the delegation
    pub delegate_address: String,

    /// Total power delegated
    pub power: String,

    /// Number of delegators
    pub delegator_count: u64,

    /// List of delegator addresses
    pub delegators: Vec<String>,

    /// Number of epochs participated (for aggregates)
    pub epochs_participated: Option<u64>,

    /// Epoch number (for specific epoch data)
    pub epoch: Option<u64>,
}
