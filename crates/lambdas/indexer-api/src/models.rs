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