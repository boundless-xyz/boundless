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

//! Voting and reward delegation power tracking.

use alloy::primitives::{Address, U256};
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

// Event types for delegation processing
#[derive(Debug, Clone)]
pub enum DelegationEvent {
    VoteDelegationChange { delegator: Address, new_delegate: Address },
    RewardDelegationChange { delegator: Address, new_delegate: Address },
    VotePowerChange { delegate: Address, new_votes: U256 },
    RewardPowerChange { delegate: Address, new_rewards: U256 },
}

#[derive(Debug, Clone)]
pub struct TimestampedDelegationEvent {
    pub event: DelegationEvent,
    pub timestamp: u64,
    pub block_number: u64,
    pub transaction_index: u64,
    pub log_index: u64,
    pub epoch: u64,
}

/// Compute delegation powers from pre-processed timestamped events
pub fn compute_delegation_powers(
    timestamped_events: &[TimestampedDelegationEvent],
    _current_epoch: u64,
    processing_end_epoch: u64,
) -> anyhow::Result<Vec<EpochDelegationPowers>> {
    // Track current state
    let mut current_vote_powers: HashMap<Address, U256> = HashMap::new();
    let mut current_reward_powers: HashMap<Address, U256> = HashMap::new();
    let mut current_vote_delegations: HashMap<Address, Address> = HashMap::new(); // delegator -> delegate
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
        match &event.event {
            DelegationEvent::VoteDelegationChange { delegator, new_delegate } => {
                if *delegator == *new_delegate {
                    current_vote_delegations.remove(delegator);
                } else {
                    current_vote_delegations.insert(*delegator, *new_delegate);
                }
            }
            DelegationEvent::RewardDelegationChange { delegator, new_delegate } => {
                if *delegator == *new_delegate {
                    current_reward_delegations.remove(delegator);
                } else {
                    current_reward_delegations.insert(*delegator, *new_delegate);
                }
            }
            DelegationEvent::VotePowerChange { delegate, new_votes } => {
                if *new_votes > U256::ZERO {
                    current_vote_powers.insert(*delegate, *new_votes);
                } else {
                    current_vote_powers.remove(delegate);
                }
            }
            DelegationEvent::RewardPowerChange { delegate, new_rewards } => {
                if *new_rewards > U256::ZERO {
                    current_reward_powers.insert(*delegate, *new_rewards);
                } else {
                    current_reward_powers.remove(delegate);
                }
            }
        }

        last_epoch = Some(event.epoch);
    }

    // Capture final state for remaining epochs
    if let Some(last) = last_epoch {
        for epoch in last..=processing_end_epoch {
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
    let all_delegates: HashSet<Address> =
        vote_powers.keys().chain(reward_powers.keys()).copied().collect();

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

        epoch_powers.insert(
            delegate,
            DelegationPowers { vote_power, reward_power, vote_delegators, reward_delegators },
        );
    }

    epoch_powers
}
