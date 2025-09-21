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

use alloy::{
    primitives::{Address, B256, U256},
    providers::Provider,
    rpc::types::{BlockNumberOrTag, Filter, Log},
    sol_types::SolEvent,
};
use anyhow::Context;
use boundless_povw::{deployments::Deployment, log_updater::IPovwAccounting};
use boundless_zkc::contracts::{IRewards, IStaking, IZKC};
use std::collections::HashMap;

// Block numbers from before contract creation.
/// Mainnet starting block for event queries
pub const MAINNET_FROM_BLOCK: u64 = 23250070;
/// Sepolia starting block for event queries
pub const SEPOLIA_FROM_BLOCK: u64 = 9110040;
/// Chunk size for log queries to avoid rate limiting
pub const LOG_QUERY_CHUNK_SIZE: u64 = 5000;

/// Information about PoVW rewards for a work log
#[derive(Debug, Clone)]
pub struct WorkLogRewardInfo {
    /// The work log ID address
    pub work_log_id: Address,
    /// Amount of work submitted in this epoch
    pub work_submitted: U256,
    /// Percentage of total work (0-100)
    pub percentage: f64,
    /// Rewards before applying cap
    pub uncapped_rewards: U256,
    /// Maximum rewards allowed based on stake
    pub reward_cap: U256,
    /// Actual rewards after applying cap
    pub actual_rewards: U256,
    /// Whether rewards were capped
    pub is_capped: bool,
    /// Amount staked for this work log
    pub staked_amount: U256,
}

/// PoVW rewards information for an epoch
#[derive(Debug, Clone)]
pub struct EpochPoVWRewards {
    /// The epoch number
    pub epoch: U256,
    /// Total work submitted in the epoch
    pub total_work: U256,
    /// Total PoVW emissions for the epoch
    pub povw_emissions: U256,
    /// Rewards breakdown by work log ID
    pub rewards_by_work_log_id: HashMap<Address, WorkLogRewardInfo>,
}

/// Container for all fetched event logs
pub struct AllEventLogs {
    /// Work log update events
    pub work_logs: Vec<Log>,
    /// Epoch finalized events
    pub epoch_finalized_logs: Vec<Log>,
    /// Stake created events
    pub stake_created_logs: Vec<Log>,
    /// Stake added events
    pub stake_added_logs: Vec<Log>,
    /// Unstake initiated events
    pub unstake_initiated_logs: Vec<Log>,
    /// Unstake completed events
    pub unstake_completed_logs: Vec<Log>,
    /// Vote delegation change events
    pub vote_delegation_logs: Vec<Log>,
    /// Reward delegation change events
    pub reward_delegation_logs: Vec<Log>,
    /// PoVW rewards claimed events
    pub povw_claims_logs: Vec<Log>,
    /// Staking rewards claimed events
    pub staking_claims_logs: Vec<Log>,
}

/// Query logs in chunks to avoid rate limiting
pub async fn query_logs_chunked<P: Provider>(
    provider: &P,
    filter: Filter,
    from_block: u64,
    to_block: u64,
) -> anyhow::Result<Vec<Log>> {
    let mut all_logs = Vec::new();
    let mut current_from = from_block;

    while current_from <= to_block {
        let current_to = (current_from + LOG_QUERY_CHUNK_SIZE - 1).min(to_block);

        let chunk_filter = filter
            .clone()
            .from_block(BlockNumberOrTag::Number(current_from))
            .to_block(BlockNumberOrTag::Number(current_to));

        let logs = provider.get_logs(&chunk_filter).await?;
        all_logs.extend(logs);

        current_from = current_to + 1;
    }

    Ok(all_logs)
}

/// Fetch all event logs in batches to avoid rate limiting
pub async fn fetch_all_event_logs<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_deployment: &boundless_zkc::deployments::Deployment,
    from_block_num: u64,
    current_block: u64,
) -> anyhow::Result<AllEventLogs> {
    tracing::info!("Fetching blockchain event data ({} blocks)...", current_block - from_block_num);

    // Batch 1: Core stake and work data (5 parallel queries)
    tracing::info!("[1/3] Querying stake and work events...");

    let work_filter = Filter::new()
        .address(deployment.povw_accounting_address)
        .event_signature(IPovwAccounting::WorkLogUpdated::SIGNATURE_HASH);

    let epoch_finalized_filter = Filter::new()
        .address(deployment.povw_accounting_address)
        .event_signature(IPovwAccounting::EpochFinalized::SIGNATURE_HASH);

    let stake_created_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakeCreated(uint256,address,uint256)")),
    );

    let stake_added_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakeAdded(uint256,address,uint256,uint256)")),
    );

    let unstake_initiated_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("UnstakeInitiated(uint256,address,uint256)")),
    );

    let (
        work_logs,
        epoch_finalized_logs,
        stake_created_logs,
        stake_added_logs,
        unstake_initiated_logs,
    ) = tokio::join!(
        async {
            query_logs_chunked(provider, work_filter.clone(), from_block_num, current_block)
                .await
                .context("Failed to get work logs")
        },
        async {
            query_logs_chunked(
                provider,
                epoch_finalized_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get epoch finalized logs")
        },
        async {
            query_logs_chunked(
                provider,
                stake_created_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get stake created logs")
        },
        async {
            query_logs_chunked(provider, stake_added_filter.clone(), from_block_num, current_block)
                .await
                .context("Failed to get stake added logs")
        },
        async {
            query_logs_chunked(
                provider,
                unstake_initiated_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get unstake initiated logs")
        }
    );

    // Batch 2: Delegation and completion (3 parallel queries)
    tracing::info!("[2/3] Querying delegation and unstake completion events...");

    let unstake_completed_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("UnstakeCompleted(uint256,address,uint256)")),
    );

    let vote_delegation_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("DelegateVotesChanged(address,uint256,uint256)")),
    );

    let reward_delegation_filter = Filter::new().address(deployment.vezkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("DelegateRewardsChanged(address,uint256,uint256)")),
    );

    let (unstake_completed_logs, vote_delegation_logs, reward_delegation_logs) = tokio::join!(
        async {
            query_logs_chunked(
                provider,
                unstake_completed_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get unstake completed logs")
        },
        async {
            query_logs_chunked(
                provider,
                vote_delegation_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get vote delegation logs")
        },
        async {
            query_logs_chunked(
                provider,
                reward_delegation_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get reward delegation logs")
        }
    );

    // Batch 3: Reward claims (2 parallel queries)
    tracing::info!("[3/3] Querying reward claim events...");

    let povw_claims_filter = Filter::new().address(zkc_deployment.zkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("PoVWRewardsClaimed(address,uint256)")),
    );

    let staking_claims_filter = Filter::new().address(zkc_deployment.zkc_address).event_signature(
        B256::from(alloy::primitives::keccak256("StakingRewardsClaimed(address,uint256)")),
    );

    let (povw_claims_logs, staking_claims_logs) = tokio::join!(
        async {
            query_logs_chunked(provider, povw_claims_filter.clone(), from_block_num, current_block)
                .await
                .context("Failed to get povw claims logs")
        },
        async {
            query_logs_chunked(
                provider,
                staking_claims_filter.clone(),
                from_block_num,
                current_block,
            )
            .await
            .context("Failed to get staking claims logs")
        }
    );

    tracing::info!("Event data fetched successfully");

    Ok(AllEventLogs {
        work_logs: work_logs?,
        epoch_finalized_logs: epoch_finalized_logs?,
        stake_created_logs: stake_created_logs?,
        stake_added_logs: stake_added_logs?,
        unstake_initiated_logs: unstake_initiated_logs?,
        unstake_completed_logs: unstake_completed_logs?,
        vote_delegation_logs: vote_delegation_logs?,
        reward_delegation_logs: reward_delegation_logs?,
        povw_claims_logs: povw_claims_logs?,
        staking_claims_logs: staking_claims_logs?,
    })
}

/// Compute PoVW rewards for a specific epoch
#[allow(clippy::too_many_arguments)]
pub async fn compute_povw_rewards_by_work_log_id<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_address: Address,
    epoch: U256,
    current_epoch: U256,
    work_log_updated_logs: &[Log],
    epoch_finalized_logs: &[Log],
) -> anyhow::Result<EpochPoVWRewards> {
    let zkc = IZKC::new(zkc_address, provider);
    // Get emissions for the epoch
    let povw_emissions = zkc.getPoVWEmissionsForEpoch(epoch).call().await?;

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

        // // If EpochFinalized had totalWork = 0, calculate from work logs
        // if epoch_total_work == U256::ZERO {
        //     let mut work_sum = U256::ZERO;
        //     for log in work_log_updated_logs {
        //         if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
        //             if U256::from(decoded.inner.data.epochNumber) == epoch {
        //                 // The updateValue in the event is already the correct type
        //                 work_sum += decoded.inner.data.updateValue;
        //             }
        //         }
        //     }
        //     epoch_total_work = work_sum;
        // }

        epoch_total_work
    };

    // Aggregate work by work_log_id for this epoch
    let mut work_by_work_log_id: HashMap<Address, U256> = HashMap::new();
    for log in work_log_updated_logs {
        if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
            if decoded.inner.data.epochNumber == epoch {
                let work_log_id = decoded.inner.data.workLogId;
                // The updateValue in the event is already U256, no conversion needed
                let update_value = decoded.inner.data.updateValue;
                *work_by_work_log_id.entry(work_log_id).or_insert(U256::ZERO) += update_value;
            }
        }
    }

    // Calculate rewards for each work_log_id
    let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
    let staking_contract = IStaking::new(deployment.vezkc_address, provider);
    let mut rewards_by_work_log_id = HashMap::new();

    // Get epoch end time for past epochs (needed for getPastPoVWRewardCap)
    let epoch_end_time = if !is_current_epoch {
        zkc.getEpochEndTime(epoch).call().await?
    } else {
        U256::ZERO // Not used for current epoch
    };

    for (work_log_id, work_submitted) in work_by_work_log_id {
        // Calculate uncapped rewards
        let uncapped_rewards = if total_work > U256::ZERO {
            povw_emissions * work_submitted / total_work
        } else {
            U256::ZERO
        };

        // Get reward cap based on whether it's current or past epoch
        let reward_cap = if is_current_epoch {
            rewards_contract.getPoVWRewardCap(work_log_id).call().await.unwrap_or(U256::ZERO)
        } else {
            rewards_contract
                .getPastPoVWRewardCap(work_log_id, epoch_end_time)
                .call()
                .await
                .unwrap_or(U256::ZERO)
        };

        // Determine actual rewards and if they're capped
        let is_capped = uncapped_rewards > reward_cap && reward_cap > U256::ZERO;
        let actual_rewards = if is_capped {
            reward_cap
        } else {
            uncapped_rewards
        };

        let percentage = if total_work > U256::ZERO {
            (work_submitted * U256::from(10000) / total_work).to::<u64>() as f64 / 100.0
        } else {
            0.0
        };

        // Get staked amount for this work log ID
        let staked_amount = staking_contract
            .getStakedAmountAndWithdrawalTime(work_log_id)
            .call()
            .await
            .map(|result| result.amount)
            .unwrap_or(U256::ZERO);

        tracing::info!("Work log ID: {}, Work submitted: {}, Percentage: {}, Uncapped rewards: {}, Reward cap: {}, Actual rewards: {}, Is capped: {}, Staked amount: {}", work_log_id, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount);
        rewards_by_work_log_id.insert(
            work_log_id,
            WorkLogRewardInfo {
                work_log_id,
                work_submitted,
                percentage,
                uncapped_rewards,
                reward_cap,
                actual_rewards,
                is_capped,
                staked_amount,
            },
        );
    }

    Ok(EpochPoVWRewards {
        epoch,
        total_work,
        povw_emissions,
        rewards_by_work_log_id,
    })
}

/// Compute staking positions from event logs
#[allow(clippy::too_many_arguments)]
pub fn compute_staking_positions_by_address(
    stake_created_logs: &[Log],
    stake_added_logs: &[Log],
    unstake_initiated_logs: &[Log],
    unstake_completed_logs: &[Log],
) -> anyhow::Result<(HashMap<Address, U256>, HashMap<Address, bool>)> {
    // Define stake event types for chronological processing
    #[derive(Debug)]
    enum StakeEvent {
        Created { owner: Address, amount: U256 },
        Added { owner: Address, new_total: U256 },
        UnstakeInitiated { owner: Address },
        UnstakeCompleted { owner: Address },
    }

    #[derive(Debug)]
    struct TimestampedStakeEvent {
        block_number: u64,
        transaction_index: u64,
        log_index: u64,
        event: StakeEvent,
    }

    let mut timestamped_events = Vec::new();

    // Collect all StakeCreated events
    for log in stake_created_logs {
        if let Ok(decoded) = log.log_decode::<IStaking::StakeCreated>() {
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                timestamped_events.push(TimestampedStakeEvent {
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    event: StakeEvent::Created {
                        owner: decoded.inner.data.owner,
                        amount: decoded.inner.data.amount,
                    },
                });
            }
        }
    }

    // Collect all StakeAdded events
    for log in stake_added_logs {
        if let Ok(decoded) = log.log_decode::<IStaking::StakeAdded>() {
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                timestamped_events.push(TimestampedStakeEvent {
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    event: StakeEvent::Added {
                        owner: decoded.inner.data.owner,
                        new_total: decoded.inner.data.newTotal,
                    },
                });
            }
        }
    }

    // Collect all UnstakeInitiated events
    for log in unstake_initiated_logs {
        if let Ok(decoded) = log.log_decode::<IStaking::UnstakeInitiated>() {
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                timestamped_events.push(TimestampedStakeEvent {
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    event: StakeEvent::UnstakeInitiated {
                        owner: decoded.inner.data.owner,
                    },
                });
            }
        }
    }

    // Collect all UnstakeCompleted events
    for log in unstake_completed_logs {
        if let Ok(decoded) = log.log_decode::<IStaking::UnstakeCompleted>() {
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                timestamped_events.push(TimestampedStakeEvent {
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    event: StakeEvent::UnstakeCompleted {
                        owner: decoded.inner.data.owner,
                    },
                });
            }
        }
    }

    // Sort events by block number, then transaction index, then log index
    timestamped_events.sort_by_key(|e| (e.block_number, e.transaction_index, e.log_index));

    // Process events in chronological order to build final state
    let mut stakes: HashMap<Address, U256> = HashMap::new();
    let mut withdrawing: HashMap<Address, bool> = HashMap::new();

    for event in timestamped_events {
        match event.event {
            StakeEvent::Created { owner, amount } => {
                // Only insert if not already present (first stake wins)
                stakes.entry(owner).or_insert(amount);
            }
            StakeEvent::Added { owner, new_total } => {
                // StakeAdded always overwrites with the new total
                stakes.insert(owner, new_total);
            }
            StakeEvent::UnstakeInitiated { owner } => {
                withdrawing.insert(owner, true);
            }
            StakeEvent::UnstakeCompleted { owner } => {
                // Remove from both maps when unstake is completed
                stakes.remove(&owner);
                withdrawing.remove(&owner);
            }
        }
    }

    Ok((stakes, withdrawing))
}

/// Compute reward and vote delegation powers from event logs
pub fn compute_rewards_and_votes_powers(
    vote_logs: &[Log],
    rewards_logs: &[Log],
) -> anyhow::Result<(HashMap<Address, U256>, HashMap<Address, U256>)> {
    let mut vote_powers: HashMap<Address, U256> = HashMap::new();

    for log in vote_logs {
        // TODO: Get artifact for DelegateVotesChanged from OZ, use manual parsing for now
        if log.topics().len() >= 2 {
            let delegate = Address::from_slice(&log.topics()[1][12..]);
            let data_bytes = &log.data().data;
            if data_bytes.len() >= 64 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data_bytes[32..64]);
                let new_votes = U256::from_be_bytes(bytes);
                // Only track non-zero powers (zero means delegation removed)
                if new_votes > U256::ZERO {
                    vote_powers.insert(delegate, new_votes);
                } else {
                    vote_powers.remove(&delegate);
                }
            }
        }
    }

    let mut reward_powers: HashMap<Address, U256> = HashMap::new();

    for log in rewards_logs {
        if let Ok(decoded) = log.log_decode::<IRewards::DelegateRewardsChanged>() {
            let delegate = decoded.inner.data.delegate;
            let new_rewards = decoded.inner.data.newRewards;
            // Only track non-zero powers (zero means delegation removed)
            if new_rewards > U256::ZERO {
                reward_powers.insert(delegate, new_rewards);
            } else {
                reward_powers.remove(&delegate);
            }
        }
    }

    Ok((vote_powers, reward_powers))
}