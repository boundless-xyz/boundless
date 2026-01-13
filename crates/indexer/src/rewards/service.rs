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

use std::collections::HashMap;
use std::sync::Arc;

use alloy::{
    primitives::{Address, U256},
    providers::{
        fillers::{ChainIdFiller, FillProvider, JoinFill},
        Identity, Provider, ProviderBuilder, RootProvider,
    },
    rpc::client::RpcClient,
    transports::layers::RetryBackoffLayer,
};
use anyhow::{Context, Result};
use boundless_povw::{deployments::Deployment as PovwDeployment, log_updater::IPovwAccounting};
use boundless_rewards::{
    build_rewards_cache, compute_delegation_powers, compute_povw_rewards, compute_staking_data,
    fetch_all_event_logs, AllEventLogs, EpochTimeRange, RewardsCache, MAINNET_FROM_BLOCK,
    SEPOLIA_FROM_BLOCK,
};
use boundless_zkc::{contracts::IZKC, deployments::Deployment as ZkcDeployment};
use tokio::time::Duration;
use url::Url;

use crate::db::rewards::{
    EpochPoVWSummary, EpochStakingSummary, PoVWSummaryStats, PovwRewardAggregate,
    PovwRewardByEpoch, RewardDelegationPowerAggregate, RewardDelegationPowerByEpoch, RewardsDb,
    RewardsDbObj, StakingPositionAggregate, StakingPositionByEpoch, StakingRewardByEpoch,
    StakingSummaryStats, VoteDelegationPowerAggregate, VoteDelegationPowerByEpoch,
};

#[derive(Clone)]
pub struct RewardsIndexerServiceConfig {
    pub interval: Duration,
    pub retries: u32,
    pub batch_size: u64,
    pub block_chunk_size: u64,
    pub start_block: Option<u64>,
    pub end_block: Option<u64>,
    pub end_epoch: Option<u64>,
    pub epochs_to_process: Option<u64>,
}

type ProviderType = FillProvider<JoinFill<Identity, ChainIdFiller>, RootProvider>;

struct EventsAndPreparedData {
    all_logs: AllEventLogs,
    actual_current_epoch_u64: u64,
    processing_end_epoch: u64,
    epochs_to_process: Vec<u64>,
    end_block: u64,
    povw_deployment: PovwDeployment,
    #[allow(dead_code)]
    zkc_deployment: ZkcDeployment,
}

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
            .connect_client(
                RpcClient::builder().layer(RetryBackoffLayer::new(3, 1000, 200)).http(rpc_url),
            );
        let chain_id = provider.get_chain_id().await?;
        let db: RewardsDbObj = Arc::new(RewardsDb::new(db_conn, None, false).await?);

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
        tracing::info!("Starting rewards indexer run");

        // Fetch events and prepare data
        let prepared_data = self.fetch_events_and_prepare_data().await?;

        // Store current epoch in database
        self.db.set_current_epoch(prepared_data.actual_current_epoch_u64).await?;

        // Build cache
        let povw_cache = self
            .build_cache(
                &prepared_data.povw_deployment,
                &prepared_data.epochs_to_process,
                prepared_data.actual_current_epoch_u64,
                &prepared_data.all_logs,
            )
            .await?;

        // Compute and store staking rewards
        let staking_amounts_by_epoch = self
            .compute_and_store_staking_rewards(
                prepared_data.actual_current_epoch_u64,
                prepared_data.processing_end_epoch,
                &povw_cache,
            )
            .await?;

        // Compute and store PoVW rewards
        self.compute_and_store_povw_rewards(
            prepared_data.actual_current_epoch_u64,
            prepared_data.processing_end_epoch,
            &prepared_data.epochs_to_process,
            &povw_cache,
            &prepared_data.povw_deployment,
            &staking_amounts_by_epoch,
        )
        .await?;

        // Compute and store delegation powers
        self.compute_and_store_delegation_powers(
            prepared_data.actual_current_epoch_u64,
            prepared_data.processing_end_epoch,
            &povw_cache,
        )
        .await?;

        // Save last processed block
        self.db.set_last_rewards_block(prepared_data.end_block).await?;

        tracing::info!(
            "Rewards indexer run completed successfully in {:.2}s",
            start_time.elapsed().as_secs_f64()
        );
        Ok(())
    }

    async fn fetch_events_and_prepare_data(&self) -> Result<EventsAndPreparedData> {
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

        // Determine ending block (use end_block if provided, otherwise current block)
        let actual_current_block = self.provider.get_block_number().await?;
        let end_block = self.config.end_block.unwrap_or(actual_current_block);

        // Validate end_block is not greater than current block
        if end_block > actual_current_block {
            anyhow::bail!(
                "End block {} is greater than current block {}",
                end_block,
                actual_current_block
            );
        }

        tracing::info!(
            "Fetching events from block {} to {} ({} blocks)",
            start_block,
            end_block,
            end_block - start_block
        );

        // Fetch all event logs up to end_block
        let fetch_start = std::time::Instant::now();
        let all_logs = fetch_all_event_logs(
            &self.provider,
            &povw_deployment,
            &zkc_deployment,
            start_block,
            end_block,
            self.config.block_chunk_size,
        )
        .await?;
        tracing::info!("Event fetching completed in {:.2}s", fetch_start.elapsed().as_secs_f64());

        // Get current epoch from ZKC contract
        let zkc = IZKC::new(self.zkc_address, &self.provider);
        let actual_current_epoch = zkc.getCurrentEpoch().call().await?;
        let actual_current_epoch_u64 = actual_current_epoch.to::<u64>();

        tracing::info!("Current blockchain epoch: {}", actual_current_epoch_u64);

        // Determine the end epoch for processing
        let processing_end_epoch = if let Some(end_epoch) = self.config.end_epoch {
            if end_epoch > actual_current_epoch_u64 {
                anyhow::bail!(
                    "End epoch {} is greater than current epoch {}",
                    end_epoch,
                    actual_current_epoch_u64
                );
            }
            tracing::info!(
                "Historical mode: processing up to epoch {} (current epoch: {})",
                end_epoch,
                actual_current_epoch_u64
            );
            end_epoch
        } else {
            tracing::info!(
                "Live mode: processing up to current epoch {}",
                actual_current_epoch_u64
            );
            actual_current_epoch_u64
        };

        tracing::info!("Processing up to epoch: {}", processing_end_epoch);

        // Process the last epochs_to_process epochs, if the arg is provided in config.
        let epochs_to_process = if let Some(epochs_to_process_count) = self.config.epochs_to_process
        {
            if epochs_to_process_count > processing_end_epoch {
                (0..=processing_end_epoch).collect::<Vec<_>>()
            } else {
                (processing_end_epoch - epochs_to_process_count + 1..=processing_end_epoch)
                    .collect::<Vec<_>>()
            }
        } else {
            (0..=processing_end_epoch).collect::<Vec<_>>()
        };

        Ok(EventsAndPreparedData {
            all_logs,
            actual_current_epoch_u64,
            processing_end_epoch,
            epochs_to_process,
            end_block,
            povw_deployment,
            zkc_deployment,
        })
    }

    async fn build_cache(
        &self,
        povw_deployment: &PovwDeployment,
        epochs_to_process: &[u64],
        actual_current_epoch_u64: u64,
        all_logs: &AllEventLogs,
    ) -> Result<RewardsCache> {
        // Build PoVW rewards cache with all necessary data (includes epoch times, block timestamps, and stake events)
        tracing::info!("Building rewards cache for {} epochs", epochs_to_process.len());
        let cache_build_start = std::time::Instant::now();
        let povw_cache = build_rewards_cache(
            &self.provider,
            povw_deployment,
            self.zkc_address,
            epochs_to_process,
            actual_current_epoch_u64, // Pass real current epoch
            self.config.end_epoch,    // Pass end_epoch for historical mode detection
            all_logs,
        )
        .await?;
        tracing::info!(
            "PoVW rewards cache built in {:.2}s",
            cache_build_start.elapsed().as_secs_f64()
        );

        Ok(povw_cache)
    }

    async fn compute_and_store_staking_rewards(
        &mut self,
        actual_current_epoch_u64: u64,
        processing_end_epoch: u64,
        povw_cache: &RewardsCache,
    ) -> Result<HashMap<(Address, u64), U256>> {
        // Compute all staking data (positions and rewards) using the unified function
        tracing::info!("Computing staking data (positions + rewards)...");
        let staking_start = std::time::Instant::now();

        let staking_data = compute_staking_data(
            actual_current_epoch_u64, // Real current epoch for comparison
            processing_end_epoch,     // Process up to this epoch
            &povw_cache.timestamped_stake_events,
            &povw_cache.staking_emissions_by_epoch,
            &povw_cache.staking_power_by_address_by_epoch,
            &povw_cache.total_staking_power_by_epoch,
        )?;

        tracing::info!(
            "Staking data computed in {:.2}s (current total: {} ZKC, {} stakers, total emissions: {} ZKC)",
            staking_start.elapsed().as_secs_f64(),
            staking_data.summary.current_total_staked / U256::from(10).pow(U256::from(18)),
            staking_data.summary.current_active_stakers,
            staking_data.summary.total_staking_emissions_all_time / U256::from(10).pow(U256::from(18))
        );

        // Store epoch and block caches from the povw_cache for later use
        self.epoch_cache = povw_cache.epoch_time_ranges.clone();
        self.block_timestamp_cache = povw_cache.block_timestamps.clone();

        // Build staking lookup for PoVW rewards computation
        let mut staking_amounts_by_epoch: HashMap<(Address, u64), U256> = HashMap::new();
        for epoch_data in &staking_data.epochs {
            for (address, position) in &epoch_data.positions_by_staker {
                staking_amounts_by_epoch
                    .insert((*address, epoch_data.epoch), position.staked_amount);
            }
        }

        // Store staking data
        tracing::info!("Storing staking data for {} epochs...", staking_data.epochs.len());
        let staking_db_start = std::time::Instant::now();

        // Store staking positions and rewards by epoch
        for epoch_data in &staking_data.epochs {
            // Store positions
            let positions: Vec<StakingPositionByEpoch> = epoch_data
                .positions_by_staker
                .iter()
                .map(|(address, position)| StakingPositionByEpoch {
                    staker_address: *address,
                    epoch: epoch_data.epoch,
                    staked_amount: position.staked_amount,
                    is_withdrawing: position.is_withdrawing,
                    rewards_delegated_to: position.rewards_delegated_to,
                    votes_delegated_to: position.votes_delegated_to,
                    rewards_generated: position.rewards_generated,
                })
                .collect();

            if !positions.is_empty() {
                self.db.upsert_staking_positions_by_epoch(epoch_data.epoch, positions).await?;
            }

            // Store rewards
            let rewards: Vec<StakingRewardByEpoch> = epoch_data
                .rewards_by_address
                .iter()
                .map(|(address, info)| StakingRewardByEpoch {
                    staker_address: *address,
                    epoch: epoch_data.epoch,
                    staking_power: info.staking_power,
                    percentage: info.percentage,
                    rewards_earned: info.rewards_earned,
                })
                .collect();

            if !rewards.is_empty() {
                self.db.upsert_staking_rewards_by_epoch(epoch_data.epoch, rewards).await?;
            }

            // Get epoch time range from cache
            let epoch_time_range = self
                .epoch_cache
                .get(&epoch_data.epoch)
                .cloned()
                .unwrap_or(boundless_rewards::EpochTimeRange { start_time: 0, end_time: 0 });

            // Store epoch summary
            let epoch_summary = EpochStakingSummary {
                epoch: epoch_data.epoch,
                total_staked: epoch_data.total_staked,
                num_stakers: epoch_data.num_stakers as u64,
                num_withdrawing: epoch_data.num_withdrawing as u64,
                total_staking_emissions: epoch_data.total_staking_emissions,
                total_staking_power: epoch_data.total_staking_power,
                num_reward_recipients: epoch_data.num_reward_recipients as u64,
                epoch_start_time: epoch_time_range.start_time,
                epoch_end_time: epoch_time_range.end_time,
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            };
            self.db.upsert_epoch_staking_summary(epoch_data.epoch, epoch_summary).await?;
        }

        // Store staking position aggregates (with rewards)
        let aggregates: Vec<StakingPositionAggregate> = staking_data
            .staker_aggregates
            .values()
            .map(|agg| StakingPositionAggregate {
                staker_address: agg.staker_address,
                total_staked: agg.current_staked,
                is_withdrawing: agg.is_withdrawing,
                rewards_delegated_to: agg.rewards_delegated_to,
                votes_delegated_to: agg.votes_delegated_to,
                epochs_participated: agg.epochs_participated,
                total_rewards_earned: agg.total_rewards_earned,
                total_rewards_generated: agg.total_rewards_generated,
            })
            .collect();

        if !aggregates.is_empty() {
            self.db.upsert_staking_positions_aggregate(aggregates).await?;
            tracing::info!(
                "Updated staking position aggregates for {} addresses",
                staking_data.staker_aggregates.len()
            );
        }

        // Store global staking summary statistics
        let staking_summary_stats = StakingSummaryStats {
            current_total_staked: staking_data.summary.current_total_staked,
            total_unique_stakers: staking_data.summary.total_unique_stakers as u64,
            current_active_stakers: staking_data.summary.current_active_stakers as u64,
            current_withdrawing: staking_data.summary.current_withdrawing as u64,
            total_staking_emissions_all_time: Some(
                staking_data.summary.total_staking_emissions_all_time,
            ),
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        self.db.upsert_staking_summary_stats(staking_summary_stats).await?;

        tracing::info!("Staking data stored in {:.2}s", staking_db_start.elapsed().as_secs_f64());

        Ok(staking_amounts_by_epoch)
    }

    async fn compute_and_store_povw_rewards(
        &self,
        actual_current_epoch_u64: u64,
        processing_end_epoch: u64,
        epochs_to_process: &[u64],
        povw_cache: &RewardsCache,
        povw_deployment: &PovwDeployment,
        staking_amounts_by_epoch: &HashMap<(Address, u64), U256>,
    ) -> Result<()> {
        // Get pending epoch total work
        // For historical indexing (when end_epoch is specified), we don't fetch pending work from blockchain state
        let pending_epoch_total_work = if self.config.end_epoch.is_some() {
            tracing::info!("Historical indexing mode - using finalized epoch data only");
            U256::ZERO
        } else {
            let povw_accounting =
                IPovwAccounting::new(povw_deployment.povw_accounting_address, &self.provider);
            let pending_epoch = povw_accounting.pendingEpoch().call().await?;
            U256::from(pending_epoch.totalWork)
        };

        // Compute rewards for all epochs at once
        tracing::info!("Computing PoVW rewards for all epochs (0 to {})...", processing_end_epoch);
        let povw_result = compute_povw_rewards(
            actual_current_epoch_u64, // Real current epoch for comparison logic
            processing_end_epoch,     // Process up to this epoch
            &povw_cache.work_by_work_log_by_epoch,
            &povw_cache.work_recipients_by_epoch,
            &povw_cache.total_work_by_epoch,
            pending_epoch_total_work,
            &povw_cache.povw_emissions_by_epoch,
            &povw_cache.reward_caps,
            staking_amounts_by_epoch,
            &povw_cache.epoch_time_ranges,
        )?;

        tracing::info!(
            "Computed rewards for {} epochs with {} unique work logs. Total work: {}, Total emissions: {}",
            povw_result.summary.total_epochs_with_work,
            povw_result.summary.total_unique_work_log_ids,
            povw_result.summary.total_work_all_time,
            povw_result.summary.total_emissions_all_time
        );

        // Store rewards for epochs we're processing
        for &epoch in epochs_to_process {
            let epoch_rewards = povw_result
                .epoch_rewards
                .iter()
                .find(|e| e.epoch == U256::from(epoch))
                .cloned()
                .unwrap_or_else(|| {
                    // Create empty epoch if not found
                    // For empty epochs, use reasonable defaults for times
                    boundless_rewards::EpochPoVWRewards {
                        epoch: U256::from(epoch),
                        total_work: U256::ZERO,
                        total_emissions: U256::ZERO,
                        total_capped_rewards: U256::ZERO,
                        total_proportional_rewards: U256::ZERO,
                        epoch_start_time: 0,
                        epoch_end_time: 0,
                        rewards_by_work_log_id: HashMap::new(),
                    }
                });

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
            tracing::debug!("Updated {} rewards for epoch {}", num_rewards, epoch);
        }

        // Convert aggregates to database format and upsert
        let aggregates: Vec<PovwRewardAggregate> = povw_result
            .summary_by_work_log_id
            .into_values()
            .map(|aggregate| PovwRewardAggregate {
                work_log_id: aggregate.work_log_id,
                total_work_submitted: aggregate.total_work_submitted,
                total_actual_rewards: aggregate.total_actual_rewards,
                total_uncapped_rewards: aggregate.total_uncapped_rewards,
                epochs_participated: aggregate.epochs_participated,
            })
            .collect();
        self.db.upsert_povw_rewards_aggregate(aggregates.clone()).await?;

        tracing::info!("Updated aggregate rewards for {} work logs", aggregates.len());

        // Store PoVW global summary statistics
        tracing::info!("Storing PoVW global summary statistics...");
        let povw_summary_stats = PoVWSummaryStats {
            total_epochs_with_work: povw_result.summary.total_epochs_with_work as u64,
            total_unique_work_log_ids: povw_result.summary.total_unique_work_log_ids as u64,
            total_work_all_time: povw_result.summary.total_work_all_time,
            total_emissions_all_time: povw_result.summary.total_emissions_all_time,
            total_capped_rewards_all_time: povw_result.summary.total_capped_rewards_all_time,
            total_uncapped_rewards_all_time: povw_result.summary.total_uncapped_rewards_all_time,
            updated_at: Some(chrono::Utc::now().to_rfc3339()),
        };
        self.db.upsert_povw_summary_stats(povw_summary_stats).await?;
        tracing::info!("Updated PoVW global summary statistics");

        // Store per-epoch PoVW summaries
        tracing::info!(
            "Storing per-epoch PoVW summaries for {} epochs...",
            povw_result.epoch_rewards.len()
        );
        for epoch_data in &povw_result.epoch_rewards {
            let num_participants = epoch_data.rewards_by_work_log_id.len() as u64;
            let epoch_summary = EpochPoVWSummary {
                epoch: epoch_data.epoch.to::<u64>(),
                total_work: epoch_data.total_work,
                total_emissions: epoch_data.total_emissions,
                total_capped_rewards: epoch_data.total_capped_rewards,
                total_uncapped_rewards: epoch_data.total_proportional_rewards,
                epoch_start_time: epoch_data.epoch_start_time,
                epoch_end_time: epoch_data.epoch_end_time,
                num_participants,
                updated_at: Some(chrono::Utc::now().to_rfc3339()),
            };
            self.db.upsert_epoch_povw_summary(epoch_data.epoch.to::<u64>(), epoch_summary).await?;
        }
        tracing::info!("Updated per-epoch PoVW summaries");

        Ok(())
    }

    async fn compute_and_store_delegation_powers(
        &self,
        actual_current_epoch_u64: u64,
        processing_end_epoch: u64,
        povw_cache: &RewardsCache,
    ) -> Result<()> {
        // Compute delegation powers
        tracing::info!("Computing delegation powers from events...");
        let delegation_start = std::time::Instant::now();

        // Compute delegation powers from pre-processed events
        let epoch_delegation_powers = compute_delegation_powers(
            &povw_cache.timestamped_delegation_events,
            actual_current_epoch_u64, // Real current epoch for comparison
            processing_end_epoch,     // Process up to this epoch
        )?;
        tracing::info!(
            "Delegation powers computed in {:.2}s",
            delegation_start.elapsed().as_secs_f64()
        );

        // Store delegation powers by epoch
        tracing::info!("Storing delegation powers for {} epochs...", epoch_delegation_powers.len());
        let delegation_db_start = std::time::Instant::now();

        for epoch_data in &epoch_delegation_powers {
            // Prepare vote delegation powers
            let vote_powers: Vec<VoteDelegationPowerByEpoch> = epoch_data
                .powers
                .iter()
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
            let reward_powers: Vec<RewardDelegationPowerByEpoch> = epoch_data
                .powers
                .iter()
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
                self.db
                    .upsert_vote_delegation_powers_by_epoch(epoch_data.epoch, vote_powers)
                    .await?;
            }
            if !reward_powers.is_empty() {
                self.db
                    .upsert_reward_delegation_powers_by_epoch(epoch_data.epoch, reward_powers)
                    .await?;
            }

            tracing::debug!("Updated delegation powers for epoch {}", epoch_data.epoch);
        }
        tracing::info!(
            "Delegation powers stored in {:.2}s",
            delegation_db_start.elapsed().as_secs_f64()
        );

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
            let vote_aggregates: Vec<VoteDelegationPowerAggregate> = latest
                .powers
                .iter()
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
            let reward_aggregates: Vec<RewardDelegationPowerAggregate> = latest
                .powers
                .iter()
                .filter(|(_, powers)| powers.reward_power > U256::ZERO)
                .map(|(address, powers)| RewardDelegationPowerAggregate {
                    delegate_address: *address,
                    total_reward_power: powers.reward_power,
                    delegator_count: powers.reward_delegators.len() as u64,
                    delegators: powers.reward_delegators.clone(),
                    epochs_participated: reward_epochs_per_address
                        .get(address)
                        .copied()
                        .unwrap_or(0),
                })
                .collect();

            // Store aggregates
            if !vote_aggregates.is_empty() {
                self.db.upsert_vote_delegation_powers_aggregate(vote_aggregates.clone()).await?;
                tracing::info!(
                    "Updated aggregate vote delegation powers for {} delegates",
                    vote_aggregates.len()
                );
            }
            if !reward_aggregates.is_empty() {
                self.db
                    .upsert_reward_delegation_powers_aggregate(reward_aggregates.clone())
                    .await?;
                tracing::info!(
                    "Updated aggregate reward delegation powers for {} delegates",
                    reward_aggregates.len()
                );
            }
        }

        Ok(())
    }
}
