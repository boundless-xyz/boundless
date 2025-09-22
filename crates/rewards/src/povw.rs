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

//! PoVW rewards computation logic.

use alloy::{
    primitives::{Address, U256},
    providers::Provider,
    rpc::types::Log,
};
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
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
    /// Rewards by work log ID
    pub rewards_by_work_log_id: HashMap<Address, WorkLogRewardInfo>,
}

/// Compute PoVW rewards for a specific epoch using cached data
#[allow(clippy::too_many_arguments)]
pub async fn compute_povw_rewards_by_work_log_id<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    epoch: U256,
    current_epoch: U256,
    work_log_updated_logs: &[Log],
    epoch_finalized_logs: &[Log],
    get_emissions: impl Fn(u64) -> anyhow::Result<U256>,
    get_reward_cap: impl Fn(Address, u64, bool) -> anyhow::Result<U256>,
    get_staking_amount: impl Fn(Address) -> anyhow::Result<U256>,
) -> anyhow::Result<EpochPoVWRewards> {
    let epoch_u64 = epoch.to::<u64>();

    // Get emissions for the epoch from cache
    let povw_emissions = get_emissions(epoch_u64)?;

    // Determine if this is the current epoch
    let is_current_epoch = epoch == current_epoch;

    // Get total work for the epoch
    let total_work = if is_current_epoch {
        // For current epoch, use pending epoch from PoVW accounting
        let povw_accounting = IPovwAccounting::new(deployment.povw_accounting_address, provider);
        let pending_epoch = povw_accounting.pendingEpoch().call().await?;
        U256::from(pending_epoch.totalWork)
    } else {
        // For past epochs, get from EpochFinalized events
        let mut epoch_total_work = U256::ZERO;
        for log in epoch_finalized_logs {
            if let Ok(decoded) = log.log_decode::<IPovwAccounting::EpochFinalized>() {
                if U256::from(decoded.inner.data.epoch) == epoch {
                    epoch_total_work = U256::from(decoded.inner.data.totalWork);
                    break;
                }
            }
        }
        epoch_total_work
    };

    // Aggregate work by work_log_id for this epoch
    let mut work_by_work_log_id: HashMap<Address, U256> = HashMap::new();
    for log in work_log_updated_logs {
        if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
            if decoded.inner.data.epochNumber == epoch {
                let work_log_id = decoded.inner.data.workLogId;
                let update_value = decoded.inner.data.updateValue;
                *work_by_work_log_id.entry(work_log_id).or_insert(U256::ZERO) += update_value;
            }
        }
    }

    // Compute rewards for each work log ID
    let mut rewards_by_work_log_id = HashMap::new();

    for (work_log_id, work) in work_by_work_log_id {
        let proportional_rewards = if total_work > U256::ZERO {
            work * povw_emissions / total_work
        } else {
            U256::ZERO
        };

        // Get reward cap from cache
        let reward_cap = get_reward_cap(work_log_id, epoch_u64, is_current_epoch)?;

        // Apply cap
        let capped_rewards = proportional_rewards.min(reward_cap);
        let is_capped = capped_rewards < proportional_rewards;

        // Get staking amount from cache
        let staking_amount = get_staking_amount(work_log_id)?;

        // Get the actual recipient (from WorkLogUpdated event)
        let mut recipient_address = work_log_id;
        for log in work_log_updated_logs {
            if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
                if decoded.inner.data.workLogId == work_log_id && decoded.inner.data.epochNumber == epoch {
                    recipient_address = decoded.inner.data.valueRecipient;
                    break;
                }
            }
        }

        rewards_by_work_log_id.insert(work_log_id, WorkLogRewardInfo {
            work_log_id,
            work,
            proportional_rewards,
            capped_rewards,
            reward_cap,
            is_capped,
            recipient_address,
            staking_amount,
        });
    }

    Ok(EpochPoVWRewards {
        epoch,
        total_work,
        total_emissions: povw_emissions,
        rewards_by_work_log_id,
    })
}