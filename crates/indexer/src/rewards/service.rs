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

use std::collections::HashMap;
use std::sync::Arc;

use alloy::{
    primitives::{Address, U256}, providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    }, rpc::client::RpcClient, transports::layers::RetryBackoffLayer
};
use anyhow::{Context, Result};
use boundless_rewards::{
    build_epoch_start_end_time_cache, build_block_timestamp_cache,
    build_povw_rewards_cache,
    compute_povw_rewards_by_work_log_id,
    compute_staking_positions_by_address, compute_delegation_powers_by_address,
    create_epoch_lookup, create_block_lookup, create_emissions_lookup,
    create_reward_cap_lookup, create_staking_amount_lookup,
    fetch_all_event_logs, EpochTimeRange,
    MAINNET_FROM_BLOCK, SEPOLIA_FROM_BLOCK,
};
use boundless_povw::deployments::Deployment as PovwDeployment;
use boundless_zkc::{contracts::IZKC, deployments::Deployment as ZkcDeployment};
use tokio::time::Duration;
use url::Url;

use crate::db::{
    rewards::{
        PovwRewardAggregate, PovwRewardByEpoch, RewardsDb, RewardsDbObj,
        StakingPositionByEpoch, StakingPositionAggregate,
        VoteDelegationPowerByEpoch, RewardDelegationPowerByEpoch,
        VoteDelegationPowerAggregate, RewardDelegationPowerAggregate,
    },
};
use std::collections::HashSet;

const EPOCHS_TO_PROCESS: u64 = 10;

#[derive(Clone)]
pub struct RewardsIndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
    pub start_block: Option<u64>,
}

type ProviderType = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

pub struct RewardsIndexerService {
    provider: ProviderType,
    db: RewardsDbObj,
    zkc_address: Address,
    #[allow(dead_code)]
    vezkc_address: Address,
    #[allow(dead_code)]
    povw_accounting_address: Address,
    config: RewardsIndexerServiceConfig,
    chain_id: u64,
    epoch_cache: HashMap<u64, EpochTimeRange>,
    block_timestamp_cache: HashMap<u64, u64>,
}

impl RewardsIndexerService {
    pub async fn new(
        rpc_url: Url,
        vezkc_address: Address,
        zkc_address: Address,
        povw_accounting_address: Address,
        db_conn: &str,
        config: RewardsIndexerServiceConfig,
    ) -> Result<Self> {
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .filler(ChainIdFiller::default())
            .connect_client(RpcClient::builder().layer(RetryBackoffLayer::new(3, 1000, 200)).http(rpc_url));
        let chain_id = provider.get_chain_id().await?;
        let db: RewardsDbObj = Arc::new(RewardsDb::new(db_conn).await?);

        Ok(Self {
            provider,
            db,
            vezkc_address,
            zkc_address,
            povw_accounting_address,
            config,
            chain_id,
            epoch_cache: HashMap::new(),
            block_timestamp_cache: HashMap::new(),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let start_time = std::time::Instant::now();
        tracing::info!("🚀 Starting rewards indexer run");

        // Get deployments based on chain ID
        let povw_deployment = PovwDeployment::from_chain_id(self.chain_id)
            .context("Could not determine PoVW deployment from chain ID")?;
        let zkc_deployment = ZkcDeployment::from_chain_id(self.chain_id)
            .context("Could not determine ZKC deployment from chain ID")?;

        // Determine starting block
        let start_block = if let Some(block) = self.config.start_block {
            block
        } else {
            match self.chain_id {
                1 => MAINNET_FROM_BLOCK,
                11155111 => SEPOLIA_FROM_BLOCK,
                _ => 0,
            }
        };

        // Get current block
        let current_block = self.provider.get_block_number().await?;

        tracing::info!(
            "📊 Fetching events from block {} to {} ({} blocks)",
            start_block,
            current_block,
            current_block - start_block
        );

        // Fetch all event logs
        let fetch_start = std::time::Instant::now();
        let all_logs = fetch_all_event_logs(
            &self.provider,
            &povw_deployment,
            &zkc_deployment,
            start_block,
            current_block,
        )
        .await?;
        tracing::info!("✅ Event fetching completed in {:.2}s", fetch_start.elapsed().as_secs_f64());

        // Get current epoch from ZKC contract
        let zkc = IZKC::new(self.zkc_address, &self.provider);
        let current_epoch = zkc.getCurrentEpoch().call().await?;
        let current_epoch_u64 = current_epoch.to::<u64>();

        tracing::info!("Current epoch: {}", current_epoch_u64);

        // Store current epoch
        self.db.set_current_epoch(current_epoch_u64).await?;

        // Process the last EPOCHS_TO_PROCESS epochs
        let epochs_to_process = if current_epoch_u64 >= EPOCHS_TO_PROCESS {
            (current_epoch_u64 - EPOCHS_TO_PROCESS + 1..=current_epoch_u64).collect::<Vec<_>>()
        } else {
            (0..=current_epoch_u64).collect::<Vec<_>>()
        };

        // Extract unique work log IDs from the work logs
        let mut unique_work_log_ids = HashSet::new();
        for log in &all_logs.work_logs {
            if let Ok(decoded) = log.log_decode::<boundless_povw::log_updater::IPovwAccounting::WorkLogUpdated>() {
                unique_work_log_ids.insert(decoded.inner.data.workLogId);
            }
        }
        let work_log_ids: Vec<Address> = unique_work_log_ids.into_iter().collect();

        // Build PoVW rewards cache with all necessary data
        tracing::info!("🔨 Building PoVW rewards cache for {} epochs and {} work log IDs",
            epochs_to_process.len(), work_log_ids.len());
        let cache_build_start = std::time::Instant::now();
        let povw_cache = build_povw_rewards_cache(
            &self.provider,
            &povw_deployment,
            self.zkc_address,
            &epochs_to_process,
            &work_log_ids,
            current_epoch_u64,
        ).await?;
        tracing::info!("✅ PoVW rewards cache built in {:.2}s", cache_build_start.elapsed().as_secs_f64());

        // Create lookup closures from cache
        let get_emissions = create_emissions_lookup(&povw_cache);
        let get_reward_cap = create_reward_cap_lookup(&povw_cache);
        let get_staking_amount = create_staking_amount_lookup(&povw_cache);

        for &epoch in &epochs_to_process {
            tracing::info!("Processing rewards for epoch {}", epoch);

            let epoch_rewards = compute_povw_rewards_by_work_log_id(
                &self.provider,
                &povw_deployment,
                U256::from(epoch),
                current_epoch,
                &all_logs.work_logs,
                &all_logs.epoch_finalized_logs,
                &get_emissions,
                &get_reward_cap,
                &get_staking_amount,
            )
            .await?;

            // Convert to database format
            let mut db_rewards = Vec::new();
            let num_rewards = epoch_rewards.rewards_by_work_log_id.len();
            let total_work = epoch_rewards.total_work;

            for (_, info) in epoch_rewards.rewards_by_work_log_id {
                // Calculate percentage before conversion
                let percentage = if total_work > U256::ZERO {
                    (info.work * U256::from(10000) / total_work).to::<u64>() as f64 / 100.0
                } else {
                    0.0
                };

                let mut reward: PovwRewardByEpoch = info.into();
                reward.epoch = epoch;
                reward.percentage = percentage;
                db_rewards.push(reward);
            }

            // Upsert epoch rewards
            self.db.upsert_povw_rewards_by_epoch(epoch, db_rewards).await?;
            tracing::info!(
                "Updated {} rewards for epoch {}",
                num_rewards,
                epoch
            );
        }

        // Compute and update aggregate table from all epochs
        tracing::info!("Computing aggregate rewards from all epochs");

        // Build cache for all epochs if needed (reuse existing cache for overlapping epochs)
        let all_epochs: Vec<u64> = (0..=current_epoch_u64).collect();
        let aggregate_cache = if all_epochs.len() > epochs_to_process.len() {
            // Need to fetch data for more epochs
            tracing::info!("🔨 Building extended PoVW cache for aggregates ({} epochs)", all_epochs.len());
            let cache_build_start = std::time::Instant::now();
            let cache = build_povw_rewards_cache(
                &self.provider,
                &povw_deployment,
                self.zkc_address,
                &all_epochs,
                &work_log_ids,
                current_epoch_u64,
            ).await?;
            tracing::info!("✅ Extended PoVW cache built in {:.2}s", cache_build_start.elapsed().as_secs_f64());
            cache
        } else {
            // Reuse existing cache
            povw_cache.clone()
        };

        // Create lookup closures from aggregate cache
        let get_emissions_agg = create_emissions_lookup(&aggregate_cache);
        let get_reward_cap_agg = create_reward_cap_lookup(&aggregate_cache);
        let get_staking_amount_agg = create_staking_amount_lookup(&aggregate_cache);

        // Process all epochs from 0 to current
        let mut aggregate_map: HashMap<Address, PovwRewardAggregate> = HashMap::new();

        for epoch_num in 0..=current_epoch_u64 {
            let epoch_rewards = compute_povw_rewards_by_work_log_id(
                &self.provider,
                &povw_deployment,
                U256::from(epoch_num),
                current_epoch,
                &all_logs.work_logs,
                &all_logs.epoch_finalized_logs,
                &get_emissions_agg,
                &get_reward_cap_agg,
                &get_staking_amount_agg,
            )
            .await?;

            for (work_log_id, info) in epoch_rewards.rewards_by_work_log_id {
                let entry = aggregate_map.entry(work_log_id).or_insert_with(|| {
                    PovwRewardAggregate {
                        work_log_id,
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
        }

        // Convert to vector and upsert
        let aggregates: Vec<PovwRewardAggregate> = aggregate_map.into_values().collect();
        self.db.upsert_povw_rewards_aggregate(aggregates.clone()).await?;

        tracing::info!(
            "Updated aggregate rewards for {} work logs",
            aggregates.len()
        );

        // Process staking positions
        tracing::info!("Computing staking positions");

        // Build/update epoch cache (only fetches missing epochs)
        tracing::info!("🕐 Building epoch time cache...");
        let cache_start = std::time::Instant::now();
        // Collect epochs to process from the logs
        let mut epochs_to_cache = HashSet::new();
        for i in 0..=current_epoch_u64 {
            epochs_to_cache.insert(i);
        }
        let epochs_vec: Vec<u64> = epochs_to_cache.into_iter().collect();
        self.epoch_cache = build_epoch_start_end_time_cache(&self.provider, self.zkc_address, &epochs_vec, current_epoch_u64).await?;
        tracing::info!("✅ Epoch cache built in {:.2}s", cache_start.elapsed().as_secs_f64());

        // Collect all staking-related logs for block timestamp cache
        let all_staking_logs: Vec<&alloy::rpc::types::Log> = all_logs.stake_created_logs.iter()
            .chain(all_logs.stake_added_logs.iter())
            .chain(all_logs.unstake_initiated_logs.iter())
            .chain(all_logs.unstake_completed_logs.iter())
            .chain(all_logs.vote_delegation_change_logs.iter())
            .chain(all_logs.reward_delegation_change_logs.iter())
            .collect();

        // Build/update block timestamp cache (only fetches missing blocks)
        tracing::info!("🕐 Building block timestamp cache for {} unique blocks...", all_staking_logs.len());
        let block_cache_start = std::time::Instant::now();
        build_block_timestamp_cache(&self.provider, &all_staking_logs, &mut self.block_timestamp_cache).await?;
        tracing::info!("✅ Block timestamp cache built in {:.2}s", block_cache_start.elapsed().as_secs_f64());

        // Collect all delegation-related logs for block timestamp cache
        let all_delegation_logs: Vec<&alloy::rpc::types::Log> = all_logs.vote_delegation_change_logs.iter()
            .chain(all_logs.reward_delegation_change_logs.iter())
            .chain(all_logs.vote_power_logs.iter())
            .chain(all_logs.reward_power_logs.iter())
            .collect();

        // Build/update block timestamp cache for delegation logs (only fetches missing blocks)
        if !all_delegation_logs.is_empty() {
            tracing::info!("🕐 Building block timestamp cache for {} delegation logs...", all_delegation_logs.len());
            let delegation_cache_start = std::time::Instant::now();
            build_block_timestamp_cache(&self.provider, &all_delegation_logs, &mut self.block_timestamp_cache).await?;
            tracing::info!("✅ Delegation timestamp cache built in {:.2}s", delegation_cache_start.elapsed().as_secs_f64());
        }

        // Create lookup closures
        let get_epoch = create_epoch_lookup(&self.epoch_cache);
        let get_timestamp = create_block_lookup(&self.block_timestamp_cache);

        // Compute staking positions (synchronous call)
        tracing::info!("🧮 Computing staking positions from events...");
        let staking_start = std::time::Instant::now();
        let epoch_positions = compute_staking_positions_by_address(
            &all_logs.stake_created_logs,
            &all_logs.stake_added_logs,
            &all_logs.unstake_initiated_logs,
            &all_logs.unstake_completed_logs,
            &all_logs.vote_delegation_change_logs,
            &all_logs.reward_delegation_change_logs,
            &get_epoch,
            &get_timestamp,
            current_epoch_u64,
        )?;
        tracing::info!("✅ Staking positions computed in {:.2}s", staking_start.elapsed().as_secs_f64());

        // Store staking positions by epoch
        tracing::info!("💾 Storing staking positions for {} epochs...", epoch_positions.len());
        let db_start = std::time::Instant::now();
        for (i, epoch_data) in epoch_positions.iter().enumerate() {
            let positions: Vec<StakingPositionByEpoch> = epoch_data.positions.iter()
                .map(|(address, position)| StakingPositionByEpoch {
                    staker_address: *address,
                    epoch: epoch_data.epoch,
                    staked_amount: position.staked_amount,
                    is_withdrawing: position.is_withdrawing,
                    rewards_delegated_to: position.rewards_delegated_to,
                    votes_delegated_to: position.votes_delegated_to,
                })
                .collect();

            if !positions.is_empty() {
                self.db.upsert_staking_positions_by_epoch(epoch_data.epoch, positions).await?;
                tracing::debug!("[{}/{}] Updated {} staking positions for epoch {}",
                    i + 1, epoch_positions.len(), epoch_data.positions.len(), epoch_data.epoch);
            }
        }
        tracing::info!("✅ Staking positions stored in {:.2}s", db_start.elapsed().as_secs_f64());

        // Compute and store aggregates (latest epoch is the current state)
        if let Some(latest) = epoch_positions.last() {
            let mut epochs_per_address: HashMap<Address, u64> = HashMap::new();

            // Count epochs participated for each address
            for epoch_data in &epoch_positions {
                for address in epoch_data.positions.keys() {
                    *epochs_per_address.entry(*address).or_insert(0) += 1;
                }
            }

            let aggregates: Vec<StakingPositionAggregate> = latest.positions.iter()
                .map(|(address, position)| StakingPositionAggregate {
                    staker_address: *address,
                    total_staked: position.staked_amount,
                    is_withdrawing: position.is_withdrawing,
                    rewards_delegated_to: position.rewards_delegated_to,
                    votes_delegated_to: position.votes_delegated_to,
                    epochs_participated: epochs_per_address.get(address).copied().unwrap_or(0),
                })
                .collect();

            if !aggregates.is_empty() {
                self.db.upsert_staking_positions_aggregate(aggregates.clone()).await?;
                tracing::info!("Updated aggregate staking positions for {} addresses", aggregates.len());
            }
        }

        // Compute delegation powers
        tracing::info!("🧮 Computing delegation powers from events...");
        let delegation_start = std::time::Instant::now();
        let epoch_delegation_powers = compute_delegation_powers_by_address(
            &all_logs.vote_delegation_change_logs,
            &all_logs.reward_delegation_change_logs,
            &all_logs.vote_power_logs,
            &all_logs.reward_power_logs,
            &get_epoch,
            &get_timestamp,
            current_epoch_u64,
        )?;
        tracing::info!("✅ Delegation powers computed in {:.2}s", delegation_start.elapsed().as_secs_f64());

        // Store delegation powers by epoch
        tracing::info!("💾 Storing delegation powers for {} epochs...", epoch_delegation_powers.len());
        let delegation_db_start = std::time::Instant::now();

        for epoch_data in &epoch_delegation_powers {
            // Prepare vote delegation powers
            let vote_powers: Vec<VoteDelegationPowerByEpoch> = epoch_data.powers.iter()
                .filter(|(_, powers)| powers.vote_power > U256::ZERO)
                .map(|(address, powers)| VoteDelegationPowerByEpoch {
                    delegate_address: *address,
                    epoch: epoch_data.epoch,
                    vote_power: powers.vote_power,
                    delegator_count: powers.vote_delegators.len() as u64,
                    delegators: powers.vote_delegators.clone(),
                })
                .collect();

            // Prepare reward delegation powers
            let reward_powers: Vec<RewardDelegationPowerByEpoch> = epoch_data.powers.iter()
                .filter(|(_, powers)| powers.reward_power > U256::ZERO)
                .map(|(address, powers)| RewardDelegationPowerByEpoch {
                    delegate_address: *address,
                    epoch: epoch_data.epoch,
                    reward_power: powers.reward_power,
                    delegator_count: powers.reward_delegators.len() as u64,
                    delegators: powers.reward_delegators.clone(),
                })
                .collect();

            // Store both vote and reward powers
            if !vote_powers.is_empty() {
                self.db.upsert_vote_delegation_powers_by_epoch(epoch_data.epoch, vote_powers).await?;
            }
            if !reward_powers.is_empty() {
                self.db.upsert_reward_delegation_powers_by_epoch(epoch_data.epoch, reward_powers).await?;
            }

            tracing::debug!("Updated delegation powers for epoch {}", epoch_data.epoch);
        }
        tracing::info!("✅ Delegation powers stored in {:.2}s", delegation_db_start.elapsed().as_secs_f64());

        // Compute and store delegation aggregates (latest epoch is the current state)
        if let Some(latest) = epoch_delegation_powers.last() {
            let mut vote_epochs_per_address: HashMap<Address, u64> = HashMap::new();
            let mut reward_epochs_per_address: HashMap<Address, u64> = HashMap::new();

            // Count epochs participated for each address
            for epoch_data in &epoch_delegation_powers {
                for (address, powers) in &epoch_data.powers {
                    if powers.vote_power > U256::ZERO {
                        *vote_epochs_per_address.entry(*address).or_insert(0) += 1;
                    }
                    if powers.reward_power > U256::ZERO {
                        *reward_epochs_per_address.entry(*address).or_insert(0) += 1;
                    }
                }
            }

            // Create vote aggregates
            let vote_aggregates: Vec<VoteDelegationPowerAggregate> = latest.powers.iter()
                .filter(|(_, powers)| powers.vote_power > U256::ZERO)
                .map(|(address, powers)| VoteDelegationPowerAggregate {
                    delegate_address: *address,
                    total_vote_power: powers.vote_power,
                    delegator_count: powers.vote_delegators.len() as u64,
                    delegators: powers.vote_delegators.clone(),
                    epochs_participated: vote_epochs_per_address.get(address).copied().unwrap_or(0),
                })
                .collect();

            // Create reward aggregates
            let reward_aggregates: Vec<RewardDelegationPowerAggregate> = latest.powers.iter()
                .filter(|(_, powers)| powers.reward_power > U256::ZERO)
                .map(|(address, powers)| RewardDelegationPowerAggregate {
                    delegate_address: *address,
                    total_reward_power: powers.reward_power,
                    delegator_count: powers.reward_delegators.len() as u64,
                    delegators: powers.reward_delegators.clone(),
                    epochs_participated: reward_epochs_per_address.get(address).copied().unwrap_or(0),
                })
                .collect();

            // Store aggregates
            if !vote_aggregates.is_empty() {
                self.db.upsert_vote_delegation_powers_aggregate(vote_aggregates.clone()).await?;
                tracing::info!("Updated aggregate vote delegation powers for {} delegates", vote_aggregates.len());
            }
            if !reward_aggregates.is_empty() {
                self.db.upsert_reward_delegation_powers_aggregate(reward_aggregates.clone()).await?;
                tracing::info!("Updated aggregate reward delegation powers for {} delegates", reward_aggregates.len());
            }
        }

        // Save last processed block
        self.db.set_last_rewards_block(current_block).await?;

        tracing::info!("🎉 Rewards indexer run completed successfully in {:.2}s", start_time.elapsed().as_secs_f64());
        Ok(())
    }
}