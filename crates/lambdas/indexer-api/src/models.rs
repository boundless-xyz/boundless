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

use serde::{Deserialize, Serialize};

/// Query parameters for pagination
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Serialize)]
pub struct AggregateLeaderboardEntry {
    /// Rank in the leaderboard (1-based)
    pub rank: u64,

    /// Work log ID (Ethereum address)
    pub work_log_id: String,

    /// Total work submitted across all epochs
    pub total_work_submitted: String,

    /// Total rewards earned across all epochs
    pub total_actual_rewards: String,

    /// Total uncapped rewards earned across all epochs
    pub total_uncapped_rewards: String,

    /// Number of epochs participated in
    pub epochs_participated: u64,
}

/// Response for epoch-specific PoVW rewards leaderboard
#[derive(Debug, Serialize)]
pub struct EpochLeaderboardEntry {
    /// Rank in the leaderboard (1-based)
    pub rank: u64,

    /// Work log ID (Ethereum address)
    pub work_log_id: String,

    /// Epoch number
    pub epoch: u64,

    /// Work submitted in this epoch
    pub work_submitted: String,

    /// Percentage of total work in epoch
    pub percentage: f64,

    /// Rewards before applying cap
    pub uncapped_rewards: String,

    /// Maximum rewards allowed based on stake
    pub reward_cap: String,

    /// Actual rewards after applying cap
    pub actual_rewards: String,

    /// Whether rewards were capped
    pub is_capped: bool,

    /// Staked amount for this work log
    pub staked_amount: String,
}

/// Response wrapper for leaderboard endpoints
#[derive(Debug, Serialize)]
pub struct LeaderboardResponse<T> {
    /// List of leaderboard entries
    pub entries: Vec<T>,

    /// Pagination metadata
    pub pagination: PaginationMetadata,
}

/// Pagination metadata
#[derive(Debug, Serialize)]
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
        Self {
            entries,
            pagination: PaginationMetadata {
                count,
                offset,
                limit,
            },
        }
    }
}

/// Response for aggregate staking leaderboard
#[derive(Debug, Serialize)]
pub struct AggregateStakingEntry {
    /// Rank in the leaderboard (1-based)
    pub rank: u64,

    /// Staker address
    pub staker_address: String,

    /// Total staked amount
    pub total_staked: String,

    /// Whether the stake is in withdrawal
    pub is_withdrawing: bool,

    /// Address this staker has delegated rewards to
    pub rewards_delegated_to: Option<String>,

    /// Address this staker has delegated votes to
    pub votes_delegated_to: Option<String>,

    /// Number of epochs participated in
    pub epochs_participated: u64,
}

/// Response for epoch-specific staking leaderboard
#[derive(Debug, Serialize)]
pub struct EpochStakingEntry {
    /// Rank in the leaderboard (1-based)
    pub rank: u64,

    /// Staker address
    pub staker_address: String,

    /// Epoch number
    pub epoch: u64,

    /// Staked amount in this epoch
    pub staked_amount: String,

    /// Whether the stake was in withdrawal during this epoch
    pub is_withdrawing: bool,

    /// Address this staker had delegated rewards to during this epoch
    pub rewards_delegated_to: Option<String>,

    /// Address this staker had delegated votes to during this epoch
    pub votes_delegated_to: Option<String>,
}

/// Query parameters for address history endpoint
#[derive(Debug, Deserialize)]
pub struct AddressHistoryParams {
    /// Starting epoch (inclusive)
    pub start_epoch: Option<u64>,

    /// Ending epoch (inclusive)
    pub end_epoch: Option<u64>,

    /// Maximum number of epochs to return (default: 100, max: 1000)
    #[serde(default = "default_history_limit")]
    pub limit: u64,
}

fn default_history_limit() -> u64 {
    100
}

impl AddressHistoryParams {
    /// Validate and normalize parameters
    pub fn validate(self) -> Self {
        Self {
            start_epoch: self.start_epoch,
            end_epoch: self.end_epoch,
            limit: self.limit.min(1000), // Cap at 1000 epochs
        }
    }
}

/// Complete history for an address across epochs
#[derive(Debug, Serialize)]
pub struct AddressHistoryResponse {
    /// The address being queried
    pub address: String,

    /// History entries by epoch (sorted by epoch descending)
    pub epochs: Vec<AddressEpochHistory>,

    /// Total number of epochs returned
    pub epoch_count: usize,
}

/// History for a single epoch for an address
#[derive(Debug, Serialize)]
pub struct AddressEpochHistory {
    /// Epoch number
    pub epoch: u64,

    /// Staking position in this epoch
    pub staking: Option<StakingPositionData>,

    /// PoVW rewards earned in this epoch
    pub povw_rewards: Option<PovwRewardsData>,

    /// Vote delegations received in this epoch
    pub vote_delegations_received: Option<DelegationPowerData>,

    /// Reward delegations received in this epoch
    pub reward_delegations_received: Option<DelegationPowerData>,
}

/// Staking position data for an epoch
#[derive(Debug, Serialize)]
pub struct StakingPositionData {
    /// Amount staked
    pub staked_amount: String,

    /// Whether the stake is in withdrawal
    pub is_withdrawing: bool,

    /// Address rewards are delegated to
    pub rewards_delegated_to: Option<String>,

    /// Address votes are delegated to
    pub votes_delegated_to: Option<String>,
}

/// PoVW rewards data for an epoch
#[derive(Debug, Serialize)]
pub struct PovwRewardsData {
    /// Work submitted
    pub work_submitted: String,

    /// Percentage of total work
    pub percentage: f64,

    /// Rewards before cap
    pub uncapped_rewards: String,

    /// Reward cap based on stake
    pub reward_cap: String,

    /// Actual rewards after cap
    pub actual_rewards: String,

    /// Whether rewards were capped
    pub is_capped: bool,

    /// Staked amount used for cap calculation
    pub staked_amount: String,
}

/// Delegation power data for an epoch
#[derive(Debug, Serialize)]
pub struct DelegationPowerData {
    /// Total power delegated
    pub power: String,

    /// Number of delegators
    pub delegator_count: u64,

    /// List of delegator addresses
    pub delegators: Vec<String>,
}

/// Response for delegation power entries
#[derive(Debug, Serialize)]
pub struct DelegationPowerEntry {
    /// Rank in the leaderboard (1-based)
    pub rank: u64,

    /// Delegate address receiving the delegation
    pub delegate_address: String,

    /// Total power delegated
    pub power: String,

    /// Number of delegators
    pub delegator_count: u64,

    /// List of delegator addresses
    pub delegators: Vec<String>,

    /// Number of epochs participated (for aggregates) or epoch number (for history)
    pub epochs_participated: Option<u64>,
}