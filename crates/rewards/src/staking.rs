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

use alloy::{
    primitives::{Address, U256},
    rpc::types::Log,
};
use boundless_zkc::contracts::{IRewards, IStaking};
use std::collections::HashMap;

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
}

// Internal types used for processing
#[derive(Debug)]
enum StakeEvent {
    Created { owner: Address, amount: U256 },
    Added { owner: Address, new_total: U256 },
    UnstakeInitiated { owner: Address },
    UnstakeCompleted { owner: Address },
    RewardDelegateChanged { delegator: Address, new_delegate: Address },
    VoteDelegateChanged { delegator: Address, new_delegate: Address },
}

#[derive(Debug)]
struct TimestampedStakeEvent {
    block_number: u64,
    #[allow(dead_code)]
    block_timestamp: u64,
    transaction_index: u64,
    log_index: u64,
    epoch: u64,
    event: StakeEvent,
}

/// Compute staking positions by address with epoch awareness
#[allow(clippy::too_many_arguments)]
pub fn compute_staking_positions_by_address(
    stake_created_logs: &[Log],
    stake_added_logs: &[Log],
    unstake_initiated_logs: &[Log],
    unstake_completed_logs: &[Log],
    vote_delegation_change_logs: &[Log],
    reward_delegation_change_logs: &[Log],
    get_epoch_for_timestamp: impl Fn(u64) -> anyhow::Result<u64>,
    get_timestamp_for_block: impl Fn(u64) -> anyhow::Result<u64>,
    current_epoch: u64,
) -> anyhow::Result<Vec<EpochStakingPositions>> {
    // Helper function to process logs into timestamped events
    fn process_event_log<F>(
        logs: &[Log],
        decode_and_create: F,
        get_timestamp_for_block: &impl Fn(u64) -> anyhow::Result<u64>,
        get_epoch_for_timestamp: &impl Fn(u64) -> anyhow::Result<u64>,
        events: &mut Vec<TimestampedStakeEvent>,
    ) -> anyhow::Result<()>
    where
        F: Fn(&Log) -> Option<StakeEvent>,
    {
        for log in logs {
            if let Some(event) = decode_and_create(log) {
                if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                    (log.block_number, log.transaction_index, log.log_index)
                {
                    let timestamp = get_timestamp_for_block(block_num)?;
                    let epoch = get_epoch_for_timestamp(timestamp)?;

                    events.push(TimestampedStakeEvent {
                        block_number: block_num,
                        block_timestamp: timestamp,
                        transaction_index: tx_idx,
                        log_index: log_idx,
                        epoch,
                        event,
                    });
                }
            }
        }
        Ok(())
    }

    let mut timestamped_events = Vec::new();

    // Process StakeCreated events
    process_event_log(
        stake_created_logs,
        |log| {
            log.log_decode::<IStaking::StakeCreated>().ok().map(|decoded| StakeEvent::Created {
                owner: decoded.inner.data.owner,
                amount: decoded.inner.data.amount,
            })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Process StakeAdded events
    process_event_log(
        stake_added_logs,
        |log| {
            log.log_decode::<IStaking::StakeAdded>().ok().map(|decoded| StakeEvent::Added {
                owner: decoded.inner.data.owner,
                new_total: decoded.inner.data.newTotal,
            })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Process UnstakeInitiated events
    process_event_log(
        unstake_initiated_logs,
        |log| {
            log.log_decode::<IStaking::UnstakeInitiated>()
                .ok()
                .map(|decoded| StakeEvent::UnstakeInitiated { owner: decoded.inner.data.owner })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Process UnstakeCompleted events
    process_event_log(
        unstake_completed_logs,
        |log| {
            log.log_decode::<IStaking::UnstakeCompleted>()
                .ok()
                .map(|decoded| StakeEvent::UnstakeCompleted { owner: decoded.inner.data.owner })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Process VoteDelegateChanged events
    process_event_log(
        vote_delegation_change_logs,
        |log| {
            // For DelegateChanged(address indexed delegator, address indexed fromDelegate, address indexed toDelegate)
            if log.topics().len() >= 4 {
                let delegator = Address::from_slice(&log.topics()[1][12..]);
                let new_delegate = Address::from_slice(&log.topics()[3][12..]);
                Some(StakeEvent::VoteDelegateChanged { delegator, new_delegate })
            } else {
                None
            }
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Process RewardDelegateChanged events
    process_event_log(
        reward_delegation_change_logs,
        |log| {
            log.log_decode::<IRewards::RewardDelegateChanged>().ok().map(|decoded| {
                StakeEvent::RewardDelegateChanged {
                    delegator: decoded.inner.data.delegator,
                    new_delegate: decoded.inner.data.toDelegate,
                }
            })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut timestamped_events,
    )?;

    // Sort events by block number, then transaction index, then log index
    timestamped_events.sort_by_key(|e| (e.block_number, e.transaction_index, e.log_index));

    // Track state per epoch
    let mut epoch_states: HashMap<u64, HashMap<Address, StakingPosition>> = HashMap::new();

    // Current state as we process events
    let mut current_stakes: HashMap<Address, U256> = HashMap::new();
    let mut current_withdrawing: HashMap<Address, bool> = HashMap::new();
    let mut current_vote_delegations: HashMap<Address, Address> = HashMap::new();
    let mut current_reward_delegations: HashMap<Address, Address> = HashMap::new();

    // Process events and capture state at epoch boundaries
    let mut last_epoch: Option<u64> = None;

    for event in timestamped_events {
        // If we've moved to a new epoch, capture the state
        let event_epoch = event.epoch;
        if last_epoch.is_none() || last_epoch.unwrap() < event_epoch {
            // Save state for all epochs between last and current
            let start = last_epoch.unwrap_or(0);
            for epoch in start..event_epoch {
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
                            rewards_delegated_to: current_reward_delegations.get(address).copied(),
                            votes_delegated_to: current_vote_delegations.get(address).copied(),
                        },
                    );
                }

                epoch_states.insert(epoch, epoch_positions);
            }
            last_epoch = Some(event_epoch);
        }

        // Apply the event to update current state
        match event.event {
            StakeEvent::Created { owner, amount } => {
                current_stakes.insert(owner, amount);
                current_withdrawing.remove(&owner);
            }
            StakeEvent::Added { owner, new_total } => {
                current_stakes.insert(owner, new_total);
            }
            StakeEvent::UnstakeInitiated { owner } => {
                current_withdrawing.insert(owner, true);
            }
            StakeEvent::UnstakeCompleted { owner } => {
                current_stakes.remove(&owner);
                current_withdrawing.remove(&owner);
                current_vote_delegations.remove(&owner);
                current_reward_delegations.remove(&owner);
            }
            StakeEvent::VoteDelegateChanged { delegator, new_delegate } => {
                if new_delegate == Address::ZERO {
                    current_vote_delegations.remove(&delegator);
                } else {
                    current_vote_delegations.insert(delegator, new_delegate);
                }
            }
            StakeEvent::RewardDelegateChanged { delegator, new_delegate } => {
                if new_delegate == Address::ZERO {
                    current_reward_delegations.remove(&delegator);
                } else {
                    current_reward_delegations.insert(delegator, new_delegate);
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

    // Convert to vector sorted by epoch
    let mut result: Vec<EpochStakingPositions> = epoch_states
        .into_iter()
        .map(|(epoch, positions)| EpochStakingPositions { epoch, positions })
        .collect();
    result.sort_by_key(|e| e.epoch);

    Ok(result)
}

/// Get current staking aggregate from epoch positions
pub fn get_current_staking_aggregate(
    epoch_positions: &[EpochStakingPositions],
) -> (HashMap<Address, U256>, HashMap<Address, bool>) {
    // Get the latest epoch's data
    if let Some(latest) = epoch_positions.last() {
        let mut stakes = HashMap::new();
        let mut withdrawing = HashMap::new();

        for (address, position) in &latest.positions {
            stakes.insert(*address, position.staked_amount);
            if position.is_withdrawing {
                withdrawing.insert(*address, true);
            }
        }

        (stakes, withdrawing)
    } else {
        (HashMap::new(), HashMap::new())
    }
}
