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

//! Staking position tracking and computation.

use alloy::primitives::{Address, U256};
use std::collections::{HashMap, HashSet};

/// Represents a staking position with delegation information
#[derive(Debug, Clone)]
pub struct StakingPosition {
    /// Amount of tokens staked
    pub staked_amount: U256,
    /// Whether the stake is being withdrawn
    pub is_withdrawing: bool,
    /// Address to whom rewards are delegated
    pub rewards_delegated_to: Option<Address>,
    /// Address to whom voting power is delegated
    pub votes_delegated_to: Option<Address>,
}

/// Staking positions for all addresses at a specific epoch
#[derive(Debug, Clone)]
pub struct EpochStakingPositions {
    /// The epoch number
    pub epoch: u64,
    /// Staking positions by address
    pub positions: HashMap<Address, StakingPosition>,
    /// Total staked amount in this epoch
    pub total_staked: U256,
    /// Number of active stakers
    pub num_stakers: usize,
    /// Number of stakers in withdrawal
    pub num_withdrawing: usize,
}

/// Summary statistics across all epochs
#[derive(Debug, Clone)]
pub struct StakingSummary {
    /// Total staked amount in the latest epoch
    pub current_total_staked: U256,
    /// Total unique stakers ever
    pub total_unique_stakers: usize,
    /// Current number of active stakers
    pub current_active_stakers: usize,
    /// Current number of stakers withdrawing
    pub current_withdrawing: usize,
}

/// Result of staking positions computation
#[derive(Debug, Clone)]
pub struct StakingPositionsResult {
    /// Positions by epoch
    pub epoch_positions: Vec<EpochStakingPositions>,
    /// Summary statistics
    pub summary: StakingSummary,
}

// Types used for processing stake events
#[derive(Debug, Clone)]
pub enum StakeEvent {
    Created { owner: Address, amount: U256 },
    Added { owner: Address, new_total: U256 },
    UnstakeInitiated { owner: Address },
    UnstakeCompleted { owner: Address },
    RewardDelegateChanged { delegator: Address, new_delegate: Address },
    VoteDelegateChanged { delegator: Address, new_delegate: Address },
}

#[derive(Debug, Clone)]
pub struct TimestampedStakeEvent {
    pub block_number: u64,
    pub block_timestamp: u64,
    pub transaction_index: u64,
    pub log_index: u64,
    pub epoch: u64,
    pub event: StakeEvent,
}

/// Compute staking positions from pre-processed timestamped events
pub fn compute_staking_positions(
    timestamped_events: &[TimestampedStakeEvent],
    current_epoch: u64,
) -> anyhow::Result<StakingPositionsResult> {
    // Tracking current state
    let mut current_stakes: HashMap<Address, U256> = HashMap::new();
    let mut current_withdrawing: HashMap<Address, bool> = HashMap::new();
    let mut current_vote_delegations: HashMap<Address, Address> = HashMap::new();
    let mut current_reward_delegations: HashMap<Address, Address> = HashMap::new();

    // Store positions by epoch
    let mut epoch_states: HashMap<u64, HashMap<Address, StakingPosition>> = HashMap::new();

    // Process events chronologically (they should already be sorted)
    let mut last_epoch: Option<u64> = None;

    for event in timestamped_events {
        let event_epoch = event.epoch;

        // When we enter a new epoch, save the current state for all previous epochs
        if let Some(prev_epoch) = last_epoch {
            if event_epoch > prev_epoch {
                // Save state for all epochs between last and current
                for epoch in prev_epoch + 1..=event_epoch {
                    let mut epoch_positions = HashMap::new();

                    for (address, amount) in &current_stakes {
                        epoch_positions.insert(
                            *address,
                            StakingPosition {
                                staked_amount: *amount,
                                is_withdrawing: current_withdrawing
                                    .get(address)
                                    .copied()
                                    .unwrap_or(false),
                                rewards_delegated_to: current_reward_delegations
                                    .get(address)
                                    .copied(),
                                votes_delegated_to: current_vote_delegations.get(address).copied(),
                            },
                        );
                    }

                    epoch_states.insert(epoch, epoch_positions);
                }
            }
        } else if event_epoch > 0 {
            // First event is not in epoch 0, save empty state for earlier epochs
            for epoch in 0..event_epoch {
                epoch_states.insert(epoch, HashMap::new());
            }

            // Save current state for the event's epoch
            let mut epoch_positions = HashMap::new();
            for (address, amount) in &current_stakes {
                epoch_positions.insert(
                    *address,
                    StakingPosition {
                        staked_amount: *amount,
                        is_withdrawing: current_withdrawing.get(address).copied().unwrap_or(false),
                        rewards_delegated_to: current_reward_delegations.get(address).copied(),
                        votes_delegated_to: current_vote_delegations.get(address).copied(),
                    },
                );
            }

            epoch_states.insert(event_epoch, epoch_positions);
        }
        last_epoch = Some(event_epoch);

        // Apply the event to update current state
        match &event.event {
            StakeEvent::Created { owner, amount } => {
                current_stakes.insert(*owner, *amount);
                current_withdrawing.remove(owner);
            }
            StakeEvent::Added { owner, new_total } => {
                current_stakes.insert(*owner, *new_total);
            }
            StakeEvent::UnstakeInitiated { owner } => {
                current_withdrawing.insert(*owner, true);
            }
            StakeEvent::UnstakeCompleted { owner } => {
                current_stakes.remove(owner);
                current_withdrawing.remove(owner);
                current_vote_delegations.remove(owner);
                current_reward_delegations.remove(owner);
            }
            StakeEvent::VoteDelegateChanged { delegator, new_delegate } => {
                if *new_delegate == Address::ZERO {
                    current_vote_delegations.remove(delegator);
                } else {
                    current_vote_delegations.insert(*delegator, *new_delegate);
                }
            }
            StakeEvent::RewardDelegateChanged { delegator, new_delegate } => {
                if *new_delegate == Address::ZERO {
                    current_reward_delegations.remove(delegator);
                } else {
                    current_reward_delegations.insert(*delegator, *new_delegate);
                }
            }
        }
    }

    // Save state for any remaining epochs up to current
    let final_epoch = last_epoch.unwrap_or(0);
    for epoch in final_epoch..=current_epoch {
        let mut epoch_positions = HashMap::new();

        for (address, amount) in &current_stakes {
            epoch_positions.insert(
                *address,
                StakingPosition {
                    staked_amount: *amount,
                    is_withdrawing: current_withdrawing.get(address).copied().unwrap_or(false),
                    rewards_delegated_to: current_reward_delegations.get(address).copied(),
                    votes_delegated_to: current_vote_delegations.get(address).copied(),
                },
            );
        }

        epoch_states.insert(epoch, epoch_positions);
    }

    // Convert to vector sorted by epoch with computed totals
    let mut epoch_positions_vec: Vec<EpochStakingPositions> = epoch_states
        .into_iter()
        .map(|(epoch, positions)| {
            let total_staked =
                positions.values().map(|p| p.staked_amount).fold(U256::ZERO, |a, b| a + b);
            let num_stakers = positions.len();
            let num_withdrawing = positions.values().filter(|p| p.is_withdrawing).count();

            EpochStakingPositions { epoch, positions, total_staked, num_stakers, num_withdrawing }
        })
        .collect();
    epoch_positions_vec.sort_by_key(|e| e.epoch);

    // Compute summary statistics
    let mut all_stakers: HashSet<Address> = HashSet::new();

    for epoch_data in &epoch_positions_vec {
        all_stakers.extend(epoch_data.positions.keys());
    }

    let current_data = epoch_positions_vec.last();
    let current_total_staked = current_data.map(|d| d.total_staked).unwrap_or(U256::ZERO);
    let current_active_stakers = current_data.map(|d| d.num_stakers).unwrap_or(0);
    let current_withdrawing = current_data.map(|d| d.num_withdrawing).unwrap_or(0);

    let summary = StakingSummary {
        current_total_staked,
        total_unique_stakers: all_stakers.len(),
        current_active_stakers,
        current_withdrawing,
    };

    Ok(StakingPositionsResult { epoch_positions: epoch_positions_vec, summary })
}
