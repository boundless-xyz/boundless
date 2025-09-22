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

//! Voting and reward delegation power tracking.

use alloy::{
    primitives::{Address, U256},
    rpc::types::Log,
};
use boundless_zkc::contracts::IRewards;
use std::collections::{HashMap, HashSet};

/// Delegation powers for voting and rewards
#[derive(Debug, Clone)]
pub struct DelegationPowers {
    /// Voting power held
    pub vote_power: U256,
    /// Reward power held
    pub reward_power: U256,
    /// Addresses that have delegated voting to this address
    pub vote_delegators: Vec<Address>,
    /// Addresses that have delegated rewards to this address
    pub reward_delegators: Vec<Address>,
}

/// Delegation powers for all addresses at a specific epoch
#[derive(Debug, Clone)]
pub struct EpochDelegationPowers {
    /// The epoch number
    pub epoch: u64,
    /// Delegation powers by address
    pub powers: HashMap<Address, DelegationPowers>,
}

// Combined event type for processing
#[derive(Debug, Clone)]
enum CombinedDelegationEvent {
    VoteDelegationChange { delegator: Address, new_delegate: Address },
    RewardDelegationChange { delegator: Address, new_delegate: Address },
    VotePowerChange { delegate: Address, new_votes: U256 },
    RewardPowerChange { delegate: Address, new_rewards: U256 },
}

#[derive(Debug, Clone)]
struct TimestampedCombinedEvent {
    event: CombinedDelegationEvent,
    timestamp: u64,
    block_number: u64,
    epoch: u64,
}

/// Compute delegation powers from event logs with epoch awareness
#[allow(clippy::too_many_arguments)]
pub fn compute_delegation_powers_by_address(
    vote_delegation_change_logs: &[Log],
    reward_delegation_change_logs: &[Log],
    vote_power_logs: &[Log],
    reward_power_logs: &[Log],
    get_epoch_for_timestamp: impl Fn(u64) -> anyhow::Result<u64>,
    get_timestamp_for_block: impl Fn(u64) -> anyhow::Result<u64>,
    current_epoch: u64,
) -> anyhow::Result<Vec<EpochDelegationPowers>> {
    let mut timestamped_events = Vec::new();

    // Process vote delegation change events (DelegateChanged)
    for log in vote_delegation_change_logs {
        if log.topics().len() >= 4 {
            let delegator = Address::from_slice(&log.topics()[1][12..]);
            let new_delegate = Address::from_slice(&log.topics()[3][12..]);
            let timestamp = get_timestamp_for_block(log.block_number.unwrap_or_default())?;
            let epoch = get_epoch_for_timestamp(timestamp)?;

            timestamped_events.push(TimestampedCombinedEvent {
                event: CombinedDelegationEvent::VoteDelegationChange { delegator, new_delegate },
                timestamp,
                block_number: log.block_number.unwrap_or_default(),
                epoch,
            });
        }
    }

    // Process reward delegation change events (RewardDelegateChanged)
    for log in reward_delegation_change_logs {
        if let Ok(decoded) = log.log_decode::<IRewards::RewardDelegateChanged>() {
            let delegator = decoded.inner.data.delegator;
            let new_delegate = decoded.inner.data.toDelegate;
            let timestamp = get_timestamp_for_block(log.block_number.unwrap_or_default())?;
            let epoch = get_epoch_for_timestamp(timestamp)?;

            timestamped_events.push(TimestampedCombinedEvent {
                event: CombinedDelegationEvent::RewardDelegationChange { delegator, new_delegate },
                timestamp,
                block_number: log.block_number.unwrap_or_default(),
                epoch,
            });
        }
    }

    // Process vote power change events (DelegateVotesChanged)
    for log in vote_power_logs {
        if log.topics().len() >= 2 {
            let delegate = Address::from_slice(&log.topics()[1][12..]);
            let data_bytes = &log.data().data;
            if data_bytes.len() >= 64 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data_bytes[32..64]);
                let new_votes = U256::from_be_bytes(bytes);
                let timestamp = get_timestamp_for_block(log.block_number.unwrap_or_default())?;
                let epoch = get_epoch_for_timestamp(timestamp)?;

                timestamped_events.push(TimestampedCombinedEvent {
                    event: CombinedDelegationEvent::VotePowerChange { delegate, new_votes },
                    timestamp,
                    block_number: log.block_number.unwrap_or_default(),
                    epoch,
                });
            }
        }
    }

    // Process reward power change events (DelegateRewardsChanged)
    for log in reward_power_logs {
        if let Ok(decoded) = log.log_decode::<IRewards::DelegateRewardsChanged>() {
            let delegate = decoded.inner.data.delegate;
            let new_rewards = decoded.inner.data.newRewards;
            let timestamp = get_timestamp_for_block(log.block_number.unwrap_or_default())?;
            let epoch = get_epoch_for_timestamp(timestamp)?;

            timestamped_events.push(TimestampedCombinedEvent {
                event: CombinedDelegationEvent::RewardPowerChange { delegate, new_rewards },
                timestamp,
                block_number: log.block_number.unwrap_or_default(),
                epoch,
            });
        }
    }

    // Sort events chronologically
    timestamped_events.sort_by(|a, b| {
        a.timestamp.cmp(&b.timestamp)
            .then_with(|| a.block_number.cmp(&b.block_number))
    });

    // Track current state
    let mut current_vote_powers: HashMap<Address, U256> = HashMap::new();
    let mut current_reward_powers: HashMap<Address, U256> = HashMap::new();
    let mut current_vote_delegations: HashMap<Address, Address> = HashMap::new();  // delegator -> delegate
    let mut current_reward_delegations: HashMap<Address, Address> = HashMap::new(); // delegator -> delegate
    let mut epoch_states: HashMap<u64, HashMap<Address, DelegationPowers>> = HashMap::new();
    let mut last_epoch: Option<u64> = None;

    for event in timestamped_events {
        // Capture state at epoch boundaries
        if last_epoch.is_some() && last_epoch != Some(event.epoch) {
            if let Some(last) = last_epoch {
                for epoch in last..event.epoch {
                    let epoch_powers = build_epoch_delegation_powers(
                        &current_vote_powers,
                        &current_reward_powers,
                        &current_vote_delegations,
                        &current_reward_delegations,
                    );
                    epoch_states.insert(epoch, epoch_powers);
                }
            }
        }

        // Apply the event
        match event.event {
            CombinedDelegationEvent::VoteDelegationChange { delegator, new_delegate } => {
                if delegator == new_delegate {
                    current_vote_delegations.remove(&delegator);
                } else {
                    current_vote_delegations.insert(delegator, new_delegate);
                }
            }
            CombinedDelegationEvent::RewardDelegationChange { delegator, new_delegate } => {
                if delegator == new_delegate {
                    current_reward_delegations.remove(&delegator);
                } else {
                    current_reward_delegations.insert(delegator, new_delegate);
                }
            }
            CombinedDelegationEvent::VotePowerChange { delegate, new_votes } => {
                if new_votes > U256::ZERO {
                    current_vote_powers.insert(delegate, new_votes);
                } else {
                    current_vote_powers.remove(&delegate);
                }
            }
            CombinedDelegationEvent::RewardPowerChange { delegate, new_rewards } => {
                if new_rewards > U256::ZERO {
                    current_reward_powers.insert(delegate, new_rewards);
                } else {
                    current_reward_powers.remove(&delegate);
                }
            }
        }

        last_epoch = Some(event.epoch);
    }

    // Capture final state for remaining epochs
    if let Some(last) = last_epoch {
        for epoch in last..=current_epoch {
            let epoch_powers = build_epoch_delegation_powers(
                &current_vote_powers,
                &current_reward_powers,
                &current_vote_delegations,
                &current_reward_delegations,
            );
            epoch_states.insert(epoch, epoch_powers);
        }
    }

    // Convert to Vec<EpochDelegationPowers>
    let mut result: Vec<EpochDelegationPowers> = epoch_states
        .into_iter()
        .map(|(epoch, powers)| EpochDelegationPowers { epoch, powers })
        .collect();

    result.sort_by_key(|e| e.epoch);

    Ok(result)
}

fn build_epoch_delegation_powers(
    vote_powers: &HashMap<Address, U256>,
    reward_powers: &HashMap<Address, U256>,
    vote_delegations: &HashMap<Address, Address>,
    reward_delegations: &HashMap<Address, Address>,
) -> HashMap<Address, DelegationPowers> {
    let mut epoch_powers = HashMap::new();

    // Get all delegates that have either vote or reward power
    let all_delegates: HashSet<Address> = vote_powers.keys()
        .chain(reward_powers.keys())
        .copied()
        .collect();

    for delegate in all_delegates {
        let vote_power = vote_powers.get(&delegate).copied().unwrap_or(U256::ZERO);
        let reward_power = reward_powers.get(&delegate).copied().unwrap_or(U256::ZERO);

        // Find delegators for this delegate
        let vote_delegators: Vec<Address> = vote_delegations
            .iter()
            .filter(|(_, &del)| del == delegate)
            .map(|(delegator, _)| *delegator)
            .collect();

        let reward_delegators: Vec<Address> = reward_delegations
            .iter()
            .filter(|(_, &del)| del == delegate)
            .map(|(delegator, _)| *delegator)
            .collect();

        epoch_powers.insert(delegate, DelegationPowers {
            vote_power,
            reward_power,
            vote_delegators,
            reward_delegators,
        });
    }

    epoch_powers
}