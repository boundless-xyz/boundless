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

//! Rewards calculation and event processing utilities for ZKC staking and PoVW rewards.

// Declare modules
pub mod cache;
pub mod events;
pub mod povw;
pub mod powers;
pub mod staking;

// Re-export commonly used types
pub use cache::{
    PoVWRewardsCache,
    build_povw_rewards_cache,
    build_epoch_start_end_time_cache,
    build_block_timestamp_cache,
    create_emissions_lookup,
    create_reward_cap_lookup,
    create_staking_amount_lookup,
    create_epoch_lookup,
    create_block_lookup,
};

pub use events::{
    AllEventLogs,
    fetch_all_event_logs,
    query_logs_chunked,
};

pub use povw::{
    WorkLogRewardInfo,
    EpochPoVWRewards,
    compute_povw_rewards_by_work_log_id,
};

pub use staking::{
    StakingPosition,
    EpochStakingPositions,
    compute_staking_positions_by_address,
    get_current_staking_aggregate,
};

pub use powers::{
    DelegationPowers,
    EpochDelegationPowers,
    compute_delegation_powers_by_address,
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
pub const LOG_QUERY_CHUNK_SIZE: u64 = 5000;