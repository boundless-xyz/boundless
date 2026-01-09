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

//! Rewards calculation and event processing utilities for ZKC staking and PoVW rewards.

// Declare modules
pub mod cache;
pub mod events;
pub mod povw;
pub mod powers;
pub mod staking;

// Re-export commonly used types
pub use cache::{build_rewards_cache, RewardsCache};

pub use events::{fetch_all_event_logs, query_logs_chunked, AllEventLogs};

pub use povw::{
    compute_povw_rewards, compute_povw_rewards_for_epoch, EpochPoVWRewards, PoVWRewardsResult,
    PoVWSummary, PoVWWorkLogIdSummary, WorkLogRewardInfo,
};

pub use staking::{
    // Main unified function
    compute_staking_data,
    // Legacy functions (for compatibility)
    compute_staking_positions,
    compute_staking_rewards,
    EpochStakingData,
    EpochStakingPositions,
    EpochStakingRewards,
    StakeEvent,
    StakerAggregate,
    StakerRewardInfo,
    StakingDataResult,
    StakingPosition,
    StakingPositionsResult,
    StakingRewardsResult,
    StakingRewardsSummary,
    StakingSummary,
    TimestampedStakeEvent,
};

pub use powers::{
    compute_delegation_powers, DelegationEvent, DelegationPowers, EpochDelegationPowers,
    TimestampedDelegationEvent,
};

/// Time range for an epoch
#[derive(Debug, Clone, Copy)]
pub struct EpochTimeRange {
    pub start_time: u64,
    pub end_time: u64,
}

// Block numbers from before contract creation.
/// Mainnet starting block for event queries
pub const MAINNET_FROM_BLOCK: u64 = 23250070;
/// Sepolia starting block for event queries
pub const SEPOLIA_FROM_BLOCK: u64 = 9110040;
/// Chunk size for log queries to avoid rate limiting
pub const LOG_QUERY_CHUNK_SIZE: u64 = 2500;
