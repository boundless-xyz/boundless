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

//! Caching and prefetching utilities for rewards computation.

use alloy::{
    primitives::{Address, U256},
    providers::Provider,
    rpc::types::{BlockNumberOrTag, Log},
};
use boundless_povw::deployments::Deployment;
use boundless_zkc::contracts::{IRewards, IStaking, IZKC};
use futures_util::future::try_join_all;
use std::collections::{HashMap, HashSet};

use crate::{
    powers::{DelegationEvent, TimestampedDelegationEvent},
    staking::{StakeEvent, TimestampedStakeEvent},
    EpochTimeRange,
};
use boundless_povw::log_updater::IPovwAccounting;

/// Contains all the necessary data for the rewards computations
#[derive(Debug, Clone, Default)]
pub struct RewardsCache {
    /// PoVW emissions by epoch number
    pub povw_emissions_by_epoch: HashMap<u64, U256>,
    /// Staking emissions by epoch number
    pub staking_emissions_by_epoch: HashMap<u64, U256>,
    /// Reward caps by (work_log_id, epoch) - includes both historical and current
    pub reward_caps: HashMap<(Address, u64), U256>,
    /// Epoch time ranges (start and end times) by epoch number
    pub epoch_time_ranges: HashMap<u64, EpochTimeRange>,
    /// Block timestamps by block number
    pub block_timestamps: HashMap<u64, u64>,
    /// Timestamped stake events, sorted by (block_number, transaction_index, log_index)
    pub timestamped_stake_events: Vec<TimestampedStakeEvent>,
    /// Timestamped delegation events, sorted by (block_number, transaction_index, log_index)
    pub timestamped_delegation_events: Vec<TimestampedDelegationEvent>,
    /// Work by work log ID and epoch
    pub work_by_work_log_by_epoch: HashMap<(Address, u64), U256>,
    /// Total work by epoch
    pub total_work_by_epoch: HashMap<u64, U256>,
    /// Work recipients by work log ID and epoch
    pub work_recipients_by_epoch: HashMap<(Address, u64), Address>,
    /// Individual staking power by (staker_address, epoch)
    pub staking_power_by_address_by_epoch: HashMap<(Address, u64), U256>,
    /// Total staking power by epoch
    pub total_staking_power_by_epoch: HashMap<u64, U256>,
    /// Staking amounts by (staker_address, epoch) - actual staked balances from positions
    pub staking_amounts_by_epoch: HashMap<(Address, u64), U256>,
}

/// For the given epochs, pre-fetches all the necessary data for the rewards computations
/// Uses multicall to batch requests and processes all event logs
pub async fn build_rewards_cache<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_address: Address,
    epochs_to_process: &[u64],
    current_epoch: u64,
    end_epoch: Option<u64>,
    all_event_logs: &crate::AllEventLogs,
) -> anyhow::Result<RewardsCache> {
    let mut cache = RewardsCache::default();

    // Extract unique work log IDs from the work logs
    let mut unique_work_log_ids = std::collections::HashSet::new();
    for log in &all_event_logs.work_logs {
        if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
            unique_work_log_ids.insert(decoded.inner.data.workLogId);
        }
    }
    let work_log_ids: Vec<Address> = unique_work_log_ids.into_iter().collect();

    let zkc = IZKC::new(zkc_address, provider);
    let rewards_contract = IRewards::new(deployment.vezkc_address, provider);

    // Batch 1: Fetch all epoch emissions (both PoVW and staking) using dynamic multicall
    if !epochs_to_process.is_empty() {
        tracing::debug!(
            "Fetching PoVW and staking emissions for {} epochs using multicall",
            epochs_to_process.len()
        );

        // Process in chunks to avoid hitting multicall limits
        const CHUNK_SIZE: usize = 50; // Smaller chunks since we're fetching both types
        for chunk in epochs_to_process.chunks(CHUNK_SIZE) {
            // Fetch PoVW emissions
            let mut povw_multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IZKC::getPoVWEmissionsForEpochCall>(
            );

            for &epoch_num in chunk {
                povw_multicall =
                    povw_multicall.add_dynamic(zkc.getPoVWEmissionsForEpoch(U256::from(epoch_num)));
            }

            let povw_results: Vec<U256> = povw_multicall.aggregate().await?;

            // Fetch staking emissions
            let mut staking_multicall =
                provider
                    .multicall()
                    .dynamic::<boundless_zkc::contracts::IZKC::getStakingEmissionsForEpochCall>();

            for &epoch_num in chunk {
                staking_multicall = staking_multicall
                    .add_dynamic(zkc.getStakingEmissionsForEpoch(U256::from(epoch_num)));
            }

            let staking_results: Vec<U256> = staking_multicall.aggregate().await?;

            // Process results - zip with input epochs
            for (i, &epoch_num) in chunk.iter().enumerate() {
                cache.povw_emissions_by_epoch.insert(epoch_num, povw_results[i]);
                cache.staking_emissions_by_epoch.insert(epoch_num, staking_results[i]);
            }
        }
    }

    // Batch 2: Fetch epoch start and end times using multicall
    if !epochs_to_process.is_empty() {
        tracing::debug!(
            "Fetching epoch start and end times for {} epochs using multicall",
            epochs_to_process.len()
        );

        const CHUNK_SIZE: usize = 50; // Smaller chunk since we're making 2 calls per epoch
        for chunk in epochs_to_process.chunks(CHUNK_SIZE) {
            // Fetch start times
            let mut start_time_multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IZKC::getEpochStartTimeCall>(
            );

            for &epoch_num in chunk {
                start_time_multicall =
                    start_time_multicall.add_dynamic(zkc.getEpochStartTime(U256::from(epoch_num)));
            }

            let start_times: Vec<U256> = start_time_multicall.aggregate().await?;

            // Fetch end times
            let mut end_time_multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IZKC::getEpochEndTimeCall>(
            );

            for &epoch_num in chunk {
                end_time_multicall =
                    end_time_multicall.add_dynamic(zkc.getEpochEndTime(U256::from(epoch_num)));
            }

            let end_times: Vec<U256> = end_time_multicall.aggregate().await?;

            // Process results
            for (i, &epoch_num) in chunk.iter().enumerate() {
                let start_time = start_times[i];
                let end_time = end_times[i];

                cache.epoch_time_ranges.insert(
                    epoch_num,
                    EpochTimeRange {
                        start_time: start_time.to::<u64>(),
                        end_time: end_time.to::<u64>(),
                    },
                );
            }
        }
    }

    // Batch 3: Fetch current reward caps using dynamic multicall
    // Skip this if we're in historical mode (end_epoch is set)
    if !work_log_ids.is_empty() && end_epoch.is_none() {
        tracing::debug!(
            "Fetching current reward caps for {} work log IDs using multicall",
            work_log_ids.len()
        );

        const CHUNK_SIZE: usize = 50;
        for chunk in work_log_ids.chunks(CHUNK_SIZE) {
            // Use dynamic multicall for same-type calls
            let mut multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IRewards::getPoVWRewardCapCall>(
            );

            for &work_log_id in chunk {
                multicall = multicall.add_dynamic(rewards_contract.getPoVWRewardCap(work_log_id));
            }

            let results: Vec<U256> = multicall.aggregate().await?;

            // Process results - store current caps with current_epoch as key
            for (&work_log_id, cap) in chunk.iter().zip(results.iter()) {
                cache.reward_caps.insert((work_log_id, current_epoch), *cap);
            }
        }
    }

    // Batch 4: Fetch past reward caps using dynamic multicall
    if epochs_to_process.iter().any(|&e| e < current_epoch) {
        tracing::debug!(
            "Fetching past reward caps for {} work log IDs and past epochs using multicall",
            work_log_ids.len()
        );

        // Build list of (work_log_id, epoch_num, epoch_end_time) tuples
        let mut past_cap_requests = Vec::new();
        for work_log_id in &work_log_ids {
            for &epoch_num in epochs_to_process {
                if epoch_num < current_epoch {
                    if let Some(epoch_range) = cache.epoch_time_ranges.get(&epoch_num) {
                        past_cap_requests.push((
                            *work_log_id,
                            epoch_num,
                            U256::from(epoch_range.end_time),
                        ));
                    }
                }
            }
        }

        // Process in chunks using dynamic multicall
        const CHUNK_SIZE: usize = 100;
        for chunk in past_cap_requests.chunks(CHUNK_SIZE) {
            // Use dynamic multicall for same-type calls
            let mut multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IRewards::getPastPoVWRewardCapCall>(
            );

            for &(work_log_id, _, epoch_end_time) in chunk {
                multicall = multicall.add_dynamic(
                    rewards_contract.getPastPoVWRewardCap(work_log_id, epoch_end_time),
                );
            }

            let results: Vec<U256> = multicall.aggregate().await?;

            // Process results - zip with input tuples
            for (&(work_log_id, epoch_num, _), cap) in chunk.iter().zip(results.iter()) {
                cache.reward_caps.insert((work_log_id, epoch_num), *cap);
            }
        }
    }

    // Batch 3: Build block timestamp cache from all event logs
    tracing::debug!("Building block timestamp cache from event logs");
    let mut all_logs: Vec<&Log> = Vec::new();
    all_logs.extend(all_event_logs.work_logs.iter());
    all_logs.extend(all_event_logs.epoch_finalized_logs.iter());
    all_logs.extend(all_event_logs.stake_created_logs.iter());
    all_logs.extend(all_event_logs.stake_added_logs.iter());
    all_logs.extend(all_event_logs.unstake_initiated_logs.iter());
    all_logs.extend(all_event_logs.unstake_completed_logs.iter());
    all_logs.extend(all_event_logs.vote_delegation_change_logs.iter());
    all_logs.extend(all_event_logs.reward_delegation_change_logs.iter());

    // Collect unique block numbers
    let mut block_numbers = HashSet::new();
    for log in &all_logs {
        if let Some(block_num) = log.block_number {
            block_numbers.insert(block_num);
        }
    }

    if !block_numbers.is_empty() {
        tracing::debug!(
            "Fetching timestamps for {} blocks using concurrent requests",
            block_numbers.len()
        );

        // Convert HashSet to Vec for chunking
        let block_numbers: Vec<_> = block_numbers.into_iter().collect();

        // Fetch timestamps for blocks using concurrent futures
        // Process in chunks to avoid overwhelming the RPC
        const CHUNK_SIZE: usize = 100;
        for chunk in block_numbers.chunks(CHUNK_SIZE) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|&block_num| async move {
                    let block =
                        provider.get_block_by_number(BlockNumberOrTag::Number(block_num)).await?;
                    Ok::<_, anyhow::Error>((block_num, block))
                })
                .collect();

            let results = try_join_all(futures).await?;

            // Process results
            for (block_num, block) in results {
                match block {
                    Some(block) => {
                        cache.block_timestamps.insert(block_num, block.header.timestamp);
                    }
                    None => {
                        anyhow::bail!("Block {} not found", block_num);
                    }
                }
            }
        }
    }

    // Batch 8: Process timestamped stake events
    tracing::debug!("Processing timestamped stake events");

    // Create lookup closures for epoch and timestamp
    let get_epoch_for_timestamp = |timestamp: u64| -> anyhow::Result<u64> {
        for (epoch, range) in &cache.epoch_time_ranges {
            if timestamp >= range.start_time && timestamp <= range.end_time {
                return Ok(*epoch);
            }
        }
        anyhow::bail!("No epoch found for timestamp {}", timestamp)
    };

    let get_timestamp_for_block = |block_num: u64| -> anyhow::Result<u64> {
        cache
            .block_timestamps
            .get(&block_num)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Block timestamp not found for block {}", block_num))
    };

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

    // Process StakeCreated events
    process_event_log(
        &all_event_logs.stake_created_logs,
        |log| {
            log.log_decode::<IStaking::StakeCreated>().ok().map(|decoded| StakeEvent::Created {
                owner: decoded.inner.data.owner,
                amount: decoded.inner.data.amount,
            })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut cache.timestamped_stake_events,
    )?;

    // Process StakeAdded events
    process_event_log(
        &all_event_logs.stake_added_logs,
        |log| {
            log.log_decode::<IStaking::StakeAdded>().ok().map(|decoded| StakeEvent::Added {
                owner: decoded.inner.data.owner,
                new_total: decoded.inner.data.newTotal,
            })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut cache.timestamped_stake_events,
    )?;

    // Process UnstakeInitiated events
    process_event_log(
        &all_event_logs.unstake_initiated_logs,
        |log| {
            log.log_decode::<IStaking::UnstakeInitiated>()
                .ok()
                .map(|decoded| StakeEvent::UnstakeInitiated { owner: decoded.inner.data.owner })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut cache.timestamped_stake_events,
    )?;

    // Process UnstakeCompleted events
    process_event_log(
        &all_event_logs.unstake_completed_logs,
        |log| {
            log.log_decode::<IStaking::UnstakeCompleted>()
                .ok()
                .map(|decoded| StakeEvent::UnstakeCompleted { owner: decoded.inner.data.owner })
        },
        &get_timestamp_for_block,
        &get_epoch_for_timestamp,
        &mut cache.timestamped_stake_events,
    )?;

    // Process VoteDelegateChanged events
    process_event_log(
        &all_event_logs.vote_delegation_change_logs,
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
        &mut cache.timestamped_stake_events,
    )?;

    // Process RewardDelegateChanged events
    process_event_log(
        &all_event_logs.reward_delegation_change_logs,
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
        &mut cache.timestamped_stake_events,
    )?;

    // Sort events by block number, then transaction index, then log index
    cache
        .timestamped_stake_events
        .sort_by_key(|e| (e.block_number, e.transaction_index, e.log_index));

    // Batch 9: Process delegation events
    tracing::debug!("Processing delegation events");

    // Process vote delegation change events (DelegateChanged)
    for log in &all_event_logs.vote_delegation_change_logs {
        if log.topics().len() >= 4 {
            let delegator = Address::from_slice(&log.topics()[1][12..]);
            let new_delegate = Address::from_slice(&log.topics()[3][12..]);
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                let timestamp = get_timestamp_for_block(block_num)?;
                let epoch = get_epoch_for_timestamp(timestamp)?;

                cache.timestamped_delegation_events.push(TimestampedDelegationEvent {
                    event: DelegationEvent::VoteDelegationChange { delegator, new_delegate },
                    timestamp,
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    epoch,
                });
            }
        }
    }

    // Process reward delegation change events (RewardDelegateChanged)
    for log in &all_event_logs.reward_delegation_change_logs {
        if let Ok(decoded) = log.log_decode::<IRewards::RewardDelegateChanged>() {
            let delegator = decoded.inner.data.delegator;
            let new_delegate = decoded.inner.data.toDelegate;
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                let timestamp = get_timestamp_for_block(block_num)?;
                let epoch = get_epoch_for_timestamp(timestamp)?;

                cache.timestamped_delegation_events.push(TimestampedDelegationEvent {
                    event: DelegationEvent::RewardDelegationChange { delegator, new_delegate },
                    timestamp,
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    epoch,
                });
            }
        }
    }

    // Process vote power change events (DelegateVotesChanged)
    for log in &all_event_logs.vote_power_logs {
        if log.topics().len() >= 2 {
            let delegate = Address::from_slice(&log.topics()[1][12..]);
            let data_bytes = &log.data().data;
            if data_bytes.len() >= 64 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data_bytes[32..64]);
                let new_votes = U256::from_be_bytes(bytes);
                if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                    (log.block_number, log.transaction_index, log.log_index)
                {
                    let timestamp = get_timestamp_for_block(block_num)?;
                    let epoch = get_epoch_for_timestamp(timestamp)?;

                    cache.timestamped_delegation_events.push(TimestampedDelegationEvent {
                        event: DelegationEvent::VotePowerChange { delegate, new_votes },
                        timestamp,
                        block_number: block_num,
                        transaction_index: tx_idx,
                        log_index: log_idx,
                        epoch,
                    });
                }
            }
        }
    }

    // Process reward power change events (DelegateRewardsChanged)
    for log in &all_event_logs.reward_power_logs {
        if let Ok(decoded) = log.log_decode::<IRewards::DelegateRewardsChanged>() {
            let delegate = decoded.inner.data.delegate;
            let new_rewards = decoded.inner.data.newRewards;
            if let (Some(block_num), Some(tx_idx), Some(log_idx)) =
                (log.block_number, log.transaction_index, log.log_index)
            {
                let timestamp = get_timestamp_for_block(block_num)?;
                let epoch = get_epoch_for_timestamp(timestamp)?;

                cache.timestamped_delegation_events.push(TimestampedDelegationEvent {
                    event: DelegationEvent::RewardPowerChange { delegate, new_rewards },
                    timestamp,
                    block_number: block_num,
                    transaction_index: tx_idx,
                    log_index: log_idx,
                    epoch,
                });
            }
        }
    }

    // Sort delegation events chronologically
    cache
        .timestamped_delegation_events
        .sort_by_key(|e| (e.block_number, e.transaction_index, e.log_index));

    // Batch 10: Process work events
    tracing::debug!("Processing work events");

    // Process WorkLogUpdated events
    for log in &all_event_logs.work_logs {
        if let Ok(decoded) = log.log_decode::<IPovwAccounting::WorkLogUpdated>() {
            let work_log_id = decoded.inner.data.workLogId;
            let epoch = decoded.inner.data.epochNumber.to::<u64>();
            let update_value = decoded.inner.data.updateValue;
            let recipient = decoded.inner.data.valueRecipient;

            // Aggregate work
            *cache.work_by_work_log_by_epoch.entry((work_log_id, epoch)).or_insert(U256::ZERO) +=
                update_value;

            // Store recipient (last one wins if multiple updates)
            cache.work_recipients_by_epoch.insert((work_log_id, epoch), recipient);
        }
    }

    // Process EpochFinalized events for total work
    for log in &all_event_logs.epoch_finalized_logs {
        if let Ok(decoded) = log.log_decode::<IPovwAccounting::EpochFinalized>() {
            let epoch = decoded.inner.data.epoch.to::<u64>();
            let total_work = U256::from(decoded.inner.data.totalWork);
            cache.total_work_by_epoch.insert(epoch, total_work);
        }
    }

    // Batch 11: Fetch staking power for rewards computation
    // Extract unique stakers from stake events
    tracing::debug!("Extracting unique stakers from stake events");
    let mut stakers_by_epoch: HashMap<u64, HashSet<Address>> = HashMap::new();

    // Process StakeCreated events
    // In historical mode, only process up to end_epoch, not current_epoch
    let max_epoch = end_epoch.unwrap_or(current_epoch);

    for event in &cache.timestamped_stake_events {
        if let StakeEvent::Created { owner, .. } = &event.event {
            stakers_by_epoch.entry(event.epoch).or_default().insert(*owner);
            // Add to all future epochs too (up to max_epoch)
            for epoch in (event.epoch + 1)..=max_epoch {
                stakers_by_epoch.entry(epoch).or_default().insert(*owner);
            }
        }
    }

    // Build a list of (staker, epoch) pairs we need to fetch
    let mut staker_epoch_pairs: Vec<(Address, u64)> = Vec::new();
    for (epoch, stakers) in &stakers_by_epoch {
        for staker in stakers {
            staker_epoch_pairs.push((*staker, *epoch));
        }
    }

    if !staker_epoch_pairs.is_empty() {
        tracing::debug!(
            "Fetching staking power for {} staker-epoch pairs using multicall",
            staker_epoch_pairs.len()
        );

        // Process in chunks to avoid hitting multicall limits
        const CHUNK_SIZE: usize = 100;
        for chunk in staker_epoch_pairs.chunks(CHUNK_SIZE) {
            // Separate current and past epochs
            let mut past_pairs = Vec::new();
            let mut current_pairs = Vec::new();

            for &(staker_address, epoch) in chunk {
                if epoch == current_epoch {
                    current_pairs.push((staker_address, epoch));
                } else {
                    past_pairs.push((staker_address, epoch));
                }
            }

            // Fetch past staking rewards using getPastStakingRewards
            if !past_pairs.is_empty() {
                let mut past_power_multicall =
                    provider
                        .multicall()
                        .dynamic::<boundless_zkc::contracts::IRewards::getPastStakingRewardsCall>();

                for &(staker_address, epoch) in &past_pairs {
                    let epoch_end_time = cache
                        .epoch_time_ranges
                        .get(&epoch)
                        .ok_or_else(|| {
                            anyhow::anyhow!("Missing epoch time range for epoch {}", epoch)
                        })?
                        .end_time;

                    past_power_multicall = past_power_multicall.add_dynamic(
                        rewards_contract
                            .getPastStakingRewards(staker_address, U256::from(epoch_end_time)),
                    );
                }

                let past_results: Vec<U256> = past_power_multicall.aggregate().await?;

                // Store past power results
                for ((staker_address, epoch), power) in past_pairs.iter().zip(past_results.iter()) {
                    cache
                        .staking_power_by_address_by_epoch
                        .insert((*staker_address, *epoch), *power);
                }
            }

            // Fetch current staking rewards using getStakingRewards
            if !current_pairs.is_empty() {
                let mut current_power_multicall =
                    provider
                        .multicall()
                        .dynamic::<boundless_zkc::contracts::IRewards::getStakingRewardsCall>();

                for &(staker_address, _epoch) in &current_pairs {
                    current_power_multicall = current_power_multicall
                        .add_dynamic(rewards_contract.getStakingRewards(staker_address));
                }

                let current_results: Vec<U256> = current_power_multicall.aggregate().await?;

                // Store current power results
                for ((staker_address, epoch), power) in
                    current_pairs.iter().zip(current_results.iter())
                {
                    cache
                        .staking_power_by_address_by_epoch
                        .insert((*staker_address, *epoch), *power);
                }
            }
        }

        // Now fetch total staking power for each unique epoch
        let unique_epochs: HashSet<u64> = staker_epoch_pairs.iter().map(|(_, e)| *e).collect();
        if !unique_epochs.is_empty() {
            tracing::debug!(
                "Fetching total staking power for {} epochs using multicall",
                unique_epochs.len()
            );

            const EPOCH_CHUNK_SIZE: usize = 50;
            let epochs_vec: Vec<u64> = unique_epochs.into_iter().collect();

            for epoch_chunk in epochs_vec.chunks(EPOCH_CHUNK_SIZE) {
                // Separate current and past epochs
                let mut current_epochs = Vec::new();
                let mut past_epochs = Vec::new();

                for &epoch in epoch_chunk {
                    if epoch == current_epoch {
                        current_epochs.push(epoch);
                    } else {
                        past_epochs.push(epoch);
                    }
                }

                // Handle past epochs with getPastTotalStakingRewards
                if !past_epochs.is_empty() {
                    let mut total_power_multicall = provider
                        .multicall()
                        .dynamic::<boundless_zkc::contracts::IRewards::getPastTotalStakingRewardsCall>();

                    let mut valid_epochs = Vec::new();
                    for &epoch in &past_epochs {
                        let epoch_end_time = cache
                            .epoch_time_ranges
                            .get(&epoch)
                            .ok_or_else(|| {
                                anyhow::anyhow!("Missing epoch time range for epoch {}", epoch)
                            })?
                            .end_time;

                        total_power_multicall = total_power_multicall.add_dynamic(
                            rewards_contract.getPastTotalStakingRewards(U256::from(epoch_end_time)),
                        );
                        valid_epochs.push(epoch);
                    }

                    let total_results: Vec<U256> = total_power_multicall.aggregate().await?;

                    // Store total power results
                    for (epoch, total_power) in valid_epochs.iter().zip(total_results.iter()) {
                        cache.total_staking_power_by_epoch.insert(*epoch, *total_power);
                    }
                }

                // Handle current epoch with getTotalStakingRewards
                if !current_epochs.is_empty() {
                    let mut current_multicall = provider
                        .multicall()
                        .dynamic::<boundless_zkc::contracts::IRewards::getTotalStakingRewardsCall>();

                    for &_epoch in &current_epochs {
                        current_multicall = current_multicall
                            .add_dynamic(rewards_contract.getTotalStakingRewards());
                    }

                    let current_results: Vec<U256> = current_multicall.aggregate().await?;

                    // Store current epoch results
                    for (epoch, total_power) in current_epochs.iter().zip(current_results.iter()) {
                        cache.total_staking_power_by_epoch.insert(*epoch, *total_power);
                    }
                }
            }
        }
    }

    tracing::info!(
        "Built rewards cache: {} povw emissions, {} staking emissions, {} work logs, {} reward caps, {} epoch time ranges, {} block timestamps, {} stake events, {} delegation events, {} work entries, {} staking power entries",
        cache.povw_emissions_by_epoch.len(),
        cache.staking_emissions_by_epoch.len(),
        work_log_ids.len(),
        cache.reward_caps.len(),
        cache.epoch_time_ranges.len(),
        cache.block_timestamps.len(),
        cache.timestamped_stake_events.len(),
        cache.timestamped_delegation_events.len(),
        cache.work_by_work_log_by_epoch.len(),
        cache.staking_power_by_address_by_epoch.len()
    );

    Ok(cache)
}
