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

use crate::EpochTimeRange;

/// Contains all the necessary data for the rewards computations
#[derive(Debug, Clone, Default)]
pub struct RewardsCache {
    /// Emissions by epoch number
    pub emissions_by_epoch: HashMap<u64, U256>,
    /// Epoch end times by epoch number
    pub epoch_end_times: HashMap<u64, U256>,
    /// Reward caps by (work_log_id, epoch) - includes both historical and current
    pub reward_caps: HashMap<(Address, u64), U256>,
    /// Latest staking amounts by work log ID (current blockchain state, not historical)
    /// WARNING: These are the current staking amounts, not the historical amounts at each epoch
    pub latest_staking_amounts: HashMap<Address, U256>,
}

/// For the given epochs, pre-fetches all the necessary data for the rewards computations
/// Uses multicall to batch requests
pub async fn build_rewards_cache<P: Provider>(
    provider: &P,
    deployment: &Deployment,
    zkc_address: Address,
    epochs_to_process: &[u64],
    work_log_ids: &[Address],
    current_epoch: u64,
) -> anyhow::Result<RewardsCache> {
    let mut cache = RewardsCache::default();

    let zkc = IZKC::new(zkc_address, provider);
    let rewards_contract = IRewards::new(deployment.vezkc_address, provider);
    let staking_contract = IStaking::new(deployment.vezkc_address, provider);

    // Batch 1: Fetch all epoch emissions using dynamic multicall
    if !epochs_to_process.is_empty() {
        tracing::debug!(
            "Fetching emissions for {} epochs using multicall",
            epochs_to_process.len()
        );

        // Process in chunks to avoid hitting multicall limits
        const CHUNK_SIZE: usize = 50;
        for chunk in epochs_to_process.chunks(CHUNK_SIZE) {
            // Use dynamic multicall for same-type calls
            let mut multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IZKC::getPoVWEmissionsForEpochCall>(
            );

            for &epoch_num in chunk {
                multicall =
                    multicall.add_dynamic(zkc.getPoVWEmissionsForEpoch(U256::from(epoch_num)));
            }

            let results: Vec<U256> = multicall.aggregate().await?;

            // Process results - zip with input epochs
            for (&epoch_num, emission) in chunk.iter().zip(results.iter()) {
                cache.emissions_by_epoch.insert(epoch_num, *emission);
            }
        }
    }

    // Batch 2: Fetch epoch end times for past epochs using dynamic multicall
    let past_epochs: Vec<u64> =
        epochs_to_process.iter().filter(|&&epoch_num| epoch_num < current_epoch).copied().collect();

    if !past_epochs.is_empty() {
        tracing::debug!(
            "Fetching epoch end times for {} past epochs using multicall",
            past_epochs.len()
        );

        const CHUNK_SIZE: usize = 50;
        for chunk in past_epochs.chunks(CHUNK_SIZE) {
            // Use dynamic multicall for same-type calls
            let mut multicall = provider
                .multicall()
                .dynamic::<boundless_zkc::contracts::IZKC::getEpochEndTimeCall>();

            for &epoch_num in chunk {
                multicall = multicall.add_dynamic(zkc.getEpochEndTime(U256::from(epoch_num)));
            }

            let results: Vec<U256> = multicall.aggregate().await?;

            // Process results - zip with input epochs
            for (&epoch_num, end_time) in chunk.iter().zip(results.iter()) {
                cache.epoch_end_times.insert(epoch_num, *end_time);
            }
        }
    }

    // Batch 3: Fetch current reward caps using dynamic multicall
    if !work_log_ids.is_empty() {
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

    // Batch 4: Fetch latest staking amounts using dynamic multicall
    // DEPRECATED: These are current staking amounts, not historical values
    // This is kept for compatibility but should not be used for historical calculations
    // Use compute_staking_positions_by_address instead for accurate historical data
    if !work_log_ids.is_empty() {
        tracing::debug!(
            "Fetching latest staking amounts for {} work log IDs using multicall",
            work_log_ids.len()
        );

        const CHUNK_SIZE: usize = 50;
        for chunk in work_log_ids.chunks(CHUNK_SIZE) {
            // Use dynamic multicall for same-type calls
            let mut multicall = provider.multicall()
                .dynamic::<boundless_zkc::contracts::IStaking::getStakedAmountAndWithdrawalTimeCall>();

            for &work_log_id in chunk {
                multicall = multicall
                    .add_dynamic(staking_contract.getStakedAmountAndWithdrawalTime(work_log_id));
            }

            let results = multicall.aggregate().await?;

            // Process results - zip with input work log IDs
            for (&work_log_id, result) in chunk.iter().zip(results.iter()) {
                cache.latest_staking_amounts.insert(work_log_id, result.amount);
            }
        }
    }

    // Batch 5: Fetch past reward caps using dynamic multicall
    if epochs_to_process.iter().any(|&e| e < current_epoch) {
        tracing::debug!(
            "Fetching past reward caps for {} work log IDs and past epochs using multicall",
            work_log_ids.len()
        );

        // Build list of (work_log_id, epoch_num, epoch_end_time) tuples
        let mut past_cap_requests = Vec::new();
        for &work_log_id in work_log_ids {
            for &epoch_num in epochs_to_process {
                if epoch_num < current_epoch {
                    if let Some(&epoch_end_time) = cache.epoch_end_times.get(&epoch_num) {
                        past_cap_requests.push((work_log_id, epoch_num, epoch_end_time));
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
                .dynamic::<boundless_zkc::contracts::IRewards::getPastPoVWRewardCapCall>();

            for &(work_log_id, _, epoch_end_time) in chunk {
                multicall = multicall.add_dynamic(
                    rewards_contract.getPastPoVWRewardCap(work_log_id, epoch_end_time)
                );
            }

            let results: Vec<U256> = multicall.aggregate().await?;

            // Process results - zip with input tuples
            for (&(work_log_id, epoch_num, _), cap) in chunk.iter().zip(results.iter()) {
                cache.reward_caps.insert((work_log_id, epoch_num), *cap);
            }
        }
    }

    tracing::info!(
        "Built PoVW rewards cache: {} epochs, {} work logs, {} reward caps, {} latest staking amounts",
        cache.emissions_by_epoch.len(),
        work_log_ids.len(),
        cache.reward_caps.len(),
        cache.latest_staking_amounts.len()
    );

    Ok(cache)
}

/// Create emissions lookup closure from cache
pub fn create_emissions_lookup(
    cache: &RewardsCache,
) -> impl Fn(u64) -> anyhow::Result<U256> + '_ {
    move |epoch: u64| -> anyhow::Result<U256> {
        cache
            .emissions_by_epoch
            .get(&epoch)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Emissions not found for epoch {}", epoch))
    }
}

/// Create reward cap lookup closure from cache
pub fn create_reward_cap_lookup(
    cache: &RewardsCache,
) -> impl Fn(Address, u64, bool) -> anyhow::Result<U256> + '_ {
    move |work_log_id: Address, epoch: u64, _is_current: bool| -> anyhow::Result<U256> {
        // All caps are now stored by (work_log_id, epoch)
        Ok(cache.reward_caps.get(&(work_log_id, epoch)).copied().unwrap_or(U256::ZERO))
    }
}

/// Create latest staking amount lookup closure from cache
/// DEPRECATED: Returns current staking amounts, not historical values
/// Use compute_staking_positions_by_address for accurate historical data
pub fn create_staking_amount_lookup(
    cache: &RewardsCache,
) -> impl Fn(Address) -> anyhow::Result<U256> + '_ {
    move |work_log_id: Address| -> anyhow::Result<U256> {
        Ok(cache.latest_staking_amounts.get(&work_log_id).copied().unwrap_or(U256::ZERO))
    }
}

/// Build cache of epoch start and end times
pub async fn build_epoch_start_end_time_cache<P: Provider>(
    provider: &P,
    zkc_address: Address,
    epochs_to_process: &[u64],
    current_epoch: u64,
) -> anyhow::Result<HashMap<u64, EpochTimeRange>> {
    let mut cache = HashMap::new();
    let zkc = IZKC::new(zkc_address, provider);

    for &epoch in epochs_to_process {
        let start_time = if epoch == 0 {
            U256::ZERO
        } else {
            zkc.getEpochEndTime(U256::from(epoch - 1)).call().await?
        };

        let end_time = if epoch == current_epoch {
            // For the current epoch, use a far future time
            U256::from(u64::MAX)
        } else {
            zkc.getEpochEndTime(U256::from(epoch)).call().await?
        };

        cache.insert(
            epoch,
            EpochTimeRange { start_time: start_time.to::<u64>(), end_time: end_time.to::<u64>() },
        );
    }

    Ok(cache)
}

/// Build cache of block timestamps to avoid repeated RPC calls
pub async fn build_block_timestamp_cache<P: Provider>(
    provider: &P,
    logs: &[&Log],
    cache: &mut HashMap<u64, u64>,
) -> anyhow::Result<()> {
    // Collect unique block numbers using HashSet
    let mut block_numbers = HashSet::new();
    for log in logs {
        if let Some(block_num) = log.block_number {
            if !cache.contains_key(&block_num) {
                block_numbers.insert(block_num);
            }
        }
    }

    if block_numbers.is_empty() {
        return Ok(());
    }

    tracing::debug!(
        "Fetching timestamps for {} blocks using concurrent requests",
        block_numbers.len()
    );

    // Convert HashSet to Vec for chunking
    let block_numbers: Vec<_> = block_numbers.into_iter().collect();

    // Fetch timestamps for blocks not in cache using concurrent futures
    // Process in chunks to avoid overwhelming the RPC
    const CHUNK_SIZE: usize = 50;
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
                    cache.insert(block_num, block.header.timestamp);
                }
                None => {
                    anyhow::bail!("Block {} not found", block_num);
                }
            }
        }
    }

    Ok(())
}

/// Create epoch lookup closure from cache
pub fn create_epoch_lookup(
    cache: &HashMap<u64, EpochTimeRange>,
) -> impl Fn(u64) -> anyhow::Result<u64> + '_ {
    move |timestamp: u64| -> anyhow::Result<u64> {
        for (epoch, range) in cache {
            if timestamp >= range.start_time && timestamp <= range.end_time {
                return Ok(*epoch);
            }
        }
        anyhow::bail!("No epoch found for timestamp {}", timestamp)
    }
}

/// Create block lookup closure from cache
pub fn create_block_lookup(cache: &HashMap<u64, u64>) -> impl Fn(u64) -> anyhow::Result<u64> + '_ {
    move |block_num: u64| -> anyhow::Result<u64> {
        cache
            .get(&block_num)
            .copied()
            .ok_or_else(|| anyhow::anyhow!("Block timestamp not found for block {}", block_num))
    }
}
