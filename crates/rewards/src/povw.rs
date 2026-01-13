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

//! PoVW rewards computation logic.

use alloy::primitives::{Address, U256};
use std::collections::HashMap;

/// Information about a work log ID's rewards for an epoch
#[derive(Debug, Clone)]
pub struct WorkLogRewardInfo {
    /// The work log ID (address)
    pub work_log_id: Address,
    /// Total work contributed by this work log ID in the epoch
    pub work: U256,
    /// Proportional share of rewards (before cap)
    pub proportional_rewards: U256,
    /// Actual rewards after applying cap
    pub capped_rewards: U256,
    /// The reward cap for this work log ID
    pub reward_cap: U256,
    /// Whether the rewards were capped
    pub is_capped: bool,
    /// Recipient address for the rewards
    pub recipient_address: Address,
    /// Staking amount for the work log ID
    pub staking_amount: U256,
}

/// PoVW rewards for an entire epoch
#[derive(Debug, Clone)]
pub struct EpochPoVWRewards {
    /// The epoch number
    pub epoch: U256,
    /// Total work in the epoch
    pub total_work: U256,
    /// Total emissions for the epoch
    pub total_emissions: U256,
    /// Total capped rewards (sum of all individual capped rewards)
    pub total_capped_rewards: U256,
    /// Total rewards before capping (sum of all proportional rewards)
    pub total_proportional_rewards: U256,
    /// Epoch start time
    pub epoch_start_time: u64,
    /// Epoch end time
    pub epoch_end_time: u64,
    /// Rewards by work log ID
    pub rewards_by_work_log_id: HashMap<Address, WorkLogRewardInfo>,
}

/// Aggregated PoVW rewards for a work log across all epochs
#[derive(Debug, Clone)]
pub struct PoVWWorkLogIdSummary {
    /// The work log ID
    pub work_log_id: Address,
    /// Total work submitted across all epochs
    pub total_work_submitted: U256,
    /// Total actual rewards received (after capping)
    pub total_actual_rewards: U256,
    /// Total uncapped rewards (before capping)
    pub total_uncapped_rewards: U256,
    /// Number of epochs participated in
    pub epochs_participated: u64,
}

/// Summary statistics for PoVW rewards across all epochs
#[derive(Debug, Clone)]
pub struct PoVWSummary {
    /// Total number of epochs with work
    pub total_epochs_with_work: usize,
    /// Total unique work log IDs
    pub total_unique_work_log_ids: usize,
    /// Total work across all epochs
    pub total_work_all_time: U256,
    /// Total emissions across all epochs
    pub total_emissions_all_time: U256,
    /// Total capped rewards distributed
    pub total_capped_rewards_all_time: U256,
    /// Total uncapped rewards (before capping)
    pub total_uncapped_rewards_all_time: U256,
}

/// Result of PoVW rewards computation across all epochs
#[derive(Debug, Clone)]
pub struct PoVWRewardsResult {
    /// Rewards by epoch
    pub epoch_rewards: Vec<EpochPoVWRewards>,
    /// Aggregated rewards by work log ID
    pub summary_by_work_log_id: HashMap<Address, PoVWWorkLogIdSummary>,
    /// Summary statistics
    pub summary: PoVWSummary,
}

/// Compute PoVW rewards for a specific epoch from pre-processed cached data
#[allow(clippy::too_many_arguments)]
pub fn compute_povw_rewards_for_epoch(
    epoch: U256,
    current_epoch: U256,
    work_by_work_log_by_epoch: &HashMap<(Address, u64), U256>,
    work_recipients_by_epoch: &HashMap<(Address, u64), Address>,
    total_work_by_epoch: &HashMap<u64, U256>,
    pending_epoch_total_work: U256,
    povw_emissions_by_epoch: &HashMap<u64, U256>,
    reward_caps: &HashMap<(Address, u64), U256>,
    staking_amounts_by_epoch: &HashMap<(Address, u64), U256>,
    epoch_time_ranges: &HashMap<u64, crate::EpochTimeRange>,
) -> anyhow::Result<EpochPoVWRewards> {
    let epoch_u64 = epoch.to::<u64>();

    // Get emissions for the epoch from cache
    let povw_emissions = povw_emissions_by_epoch
        .get(&epoch_u64)
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Emissions not found for epoch {}", epoch_u64))?;

    // Get epoch time range
    let epoch_time_range = epoch_time_ranges
        .get(&epoch_u64)
        .ok_or_else(|| anyhow::anyhow!("Epoch time range not found for epoch {}", epoch_u64))?;
    let epoch_start_time = epoch_time_range.start_time;
    let epoch_end_time = epoch_time_range.end_time;

    // Determine if this is the current epoch
    let is_current_epoch = epoch == current_epoch;

    // Get total work for the epoch
    let total_work = if is_current_epoch {
        // For current epoch, use pending epoch total work
        pending_epoch_total_work
    } else {
        // For past epochs, get from cached total work
        total_work_by_epoch.get(&epoch_u64).copied().unwrap_or(U256::ZERO)
    };

    // Get work by work_log_id for this epoch from cache
    let mut work_by_work_log_id: HashMap<Address, U256> = HashMap::new();
    for ((work_log_id, work_epoch), work) in work_by_work_log_by_epoch {
        if *work_epoch == epoch_u64 {
            work_by_work_log_id.insert(*work_log_id, *work);
        }
    }

    // Compute rewards for each work log ID
    let mut rewards_by_work_log_id = HashMap::new();
    let mut total_proportional_rewards = U256::ZERO;
    let mut total_capped_rewards = U256::ZERO;

    for (work_log_id, work) in work_by_work_log_id {
        let proportional_rewards =
            if total_work > U256::ZERO { work * povw_emissions / total_work } else { U256::ZERO };

        // Get reward cap from cache
        let reward_cap = reward_caps.get(&(work_log_id, epoch_u64)).copied().ok_or_else(|| {
            anyhow::anyhow!(
                "Reward cap not found for work log {:?} in epoch {}",
                work_log_id,
                epoch_u64
            )
        })?;

        // Apply cap
        let capped_rewards = proportional_rewards.min(reward_cap);
        let is_capped = capped_rewards < proportional_rewards;

        // Get staking amount from cache for this epoch
        let staking_amount =
            staking_amounts_by_epoch.get(&(work_log_id, epoch_u64)).copied().unwrap_or(U256::ZERO);

        // Get the actual recipient from cache
        let recipient_address =
            work_recipients_by_epoch.get(&(work_log_id, epoch_u64)).copied().unwrap_or(work_log_id);

        // Track totals
        total_proportional_rewards += proportional_rewards;
        total_capped_rewards += capped_rewards;

        rewards_by_work_log_id.insert(
            work_log_id,
            WorkLogRewardInfo {
                work_log_id,
                work,
                proportional_rewards,
                capped_rewards,
                reward_cap,
                is_capped,
                recipient_address,
                staking_amount,
            },
        );
    }

    Ok(EpochPoVWRewards {
        epoch,
        total_work,
        total_emissions: povw_emissions,
        total_capped_rewards,
        total_proportional_rewards,
        epoch_start_time,
        epoch_end_time,
        rewards_by_work_log_id,
    })
}

/// Compute PoVW rewards for all epochs and generate aggregates
#[allow(clippy::too_many_arguments)]
pub fn compute_povw_rewards(
    current_epoch: u64,
    processing_end_epoch: u64,
    work_by_work_log_by_epoch: &HashMap<(Address, u64), U256>,
    work_recipients_by_epoch: &HashMap<(Address, u64), Address>,
    total_work_by_epoch: &HashMap<u64, U256>,
    pending_epoch_total_work: U256,
    povw_emissions_by_epoch: &HashMap<u64, U256>,
    reward_caps: &HashMap<(Address, u64), U256>,
    staking_amounts_by_epoch: &HashMap<(Address, u64), U256>,
    epoch_time_ranges: &HashMap<u64, crate::EpochTimeRange>,
) -> anyhow::Result<PoVWRewardsResult> {
    let mut epoch_rewards = Vec::new();
    let mut aggregates_by_work_log: HashMap<Address, PoVWWorkLogIdSummary> = HashMap::new();

    // Statistics for summary
    let mut total_epochs_with_work = 0;
    let mut total_work_all_time = U256::ZERO;
    let mut total_emissions_all_time = U256::ZERO;
    let mut total_capped_rewards_all_time = U256::ZERO;
    let mut total_uncapped_rewards_all_time = U256::ZERO;

    // Process each epoch from 0 to processing_end_epoch
    for epoch_num in 0..=processing_end_epoch {
        let epoch_result = compute_povw_rewards_for_epoch(
            U256::from(epoch_num),
            U256::from(current_epoch),
            work_by_work_log_by_epoch,
            work_recipients_by_epoch,
            total_work_by_epoch,
            pending_epoch_total_work,
            povw_emissions_by_epoch,
            reward_caps,
            staking_amounts_by_epoch,
            epoch_time_ranges,
        )?;

        // Update summary statistics
        if epoch_result.total_work > U256::ZERO {
            total_epochs_with_work += 1;
        }
        total_work_all_time += epoch_result.total_work;
        total_emissions_all_time += epoch_result.total_emissions;
        total_capped_rewards_all_time += epoch_result.total_capped_rewards;
        total_uncapped_rewards_all_time += epoch_result.total_proportional_rewards;

        // Update aggregates for each work log ID in this epoch
        for (work_log_id, info) in &epoch_result.rewards_by_work_log_id {
            let entry = aggregates_by_work_log.entry(*work_log_id).or_insert_with(|| {
                PoVWWorkLogIdSummary {
                    work_log_id: *work_log_id,
                    total_work_submitted: U256::ZERO,
                    total_actual_rewards: U256::ZERO,
                    total_uncapped_rewards: U256::ZERO,
                    epochs_participated: 0,
                }
            });

            entry.total_work_submitted += info.work;
            entry.total_actual_rewards += info.capped_rewards;
            entry.total_uncapped_rewards += info.proportional_rewards;
            if info.work > U256::ZERO {
                entry.epochs_participated += 1;
            }
        }

        epoch_rewards.push(epoch_result);
    }

    let summary = PoVWSummary {
        total_epochs_with_work,
        total_unique_work_log_ids: aggregates_by_work_log.len(),
        total_work_all_time,
        total_emissions_all_time,
        total_capped_rewards_all_time,
        total_uncapped_rewards_all_time,
    };

    Ok(PoVWRewardsResult { epoch_rewards, summary_by_work_log_id: aggregates_by_work_log, summary })
}
