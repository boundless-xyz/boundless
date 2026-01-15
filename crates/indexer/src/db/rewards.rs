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

use std::{str::FromStr, sync::Arc, time::Duration};

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use boundless_rewards::{StakingPosition, WorkLogRewardInfo};
use chrono::Utc;
use log::LevelFilter;
use serde_json;
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    ConnectOptions, PgPool, Row,
};

use super::DbError;

pub type RewardsDbObj = Arc<dyn RewardsIndexerDb + Send + Sync>;

/// Convert a U256 to a zero-padded string for proper database sorting
/// U256 max value has 78 decimal digits (2^256 ≈ 1.15 * 10^77)
fn pad_u256(value: U256) -> String {
    format!("{:0>78}", value)
}

/// Convert a zero-padded string back to U256
fn unpad_u256(s: &str) -> Result<U256, DbError> {
    U256::from_str(s.trim_start_matches('0')).or_else(|_| {
        // If trimming all zeros, the value is 0
        if s.chars().all(|c| c == '0') {
            Ok(U256::ZERO)
        } else {
            Err(DbError::BadTransaction(format!("Invalid U256 string: {}", s)))
        }
    })
}

#[derive(Debug, Clone)]
pub struct PovwRewardByEpoch {
    pub work_log_id: Address,
    pub epoch: u64,
    pub work_submitted: U256,
    pub percentage: f64,
    pub uncapped_rewards: U256,
    pub reward_cap: U256,
    pub actual_rewards: U256,
    pub is_capped: bool,
    pub staked_amount: U256,
}

impl From<WorkLogRewardInfo> for PovwRewardByEpoch {
    fn from(info: WorkLogRewardInfo) -> Self {
        // Note: percentage needs to be calculated by the caller since we don't have total_work here
        Self {
            work_log_id: info.work_log_id,
            epoch: 0, // Will be set by caller
            work_submitted: info.work,
            percentage: 0.0, // Will be set by caller
            uncapped_rewards: info.proportional_rewards,
            reward_cap: info.reward_cap,
            actual_rewards: info.capped_rewards,
            is_capped: info.is_capped,
            staked_amount: info.staking_amount,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PovwRewardAggregate {
    pub work_log_id: Address,
    pub total_work_submitted: U256,
    pub total_actual_rewards: U256,
    pub total_uncapped_rewards: U256,
    pub epochs_participated: u64,
}

#[derive(Debug, Clone)]
pub struct StakingPositionByEpoch {
    pub staker_address: Address,
    pub epoch: u64,
    pub staked_amount: U256,
    pub is_withdrawing: bool,
    pub rewards_delegated_to: Option<Address>,
    pub votes_delegated_to: Option<Address>,
    pub rewards_generated: U256,
}

impl From<(Address, u64, &StakingPosition)> for StakingPositionByEpoch {
    fn from(value: (Address, u64, &StakingPosition)) -> Self {
        Self {
            staker_address: value.0,
            epoch: value.1,
            staked_amount: value.2.staked_amount,
            is_withdrawing: value.2.is_withdrawing,
            rewards_delegated_to: value.2.rewards_delegated_to,
            votes_delegated_to: value.2.votes_delegated_to,
            rewards_generated: value.2.rewards_generated,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StakingPositionAggregate {
    pub staker_address: Address,
    pub total_staked: U256,
    pub is_withdrawing: bool,
    pub rewards_delegated_to: Option<Address>,
    pub votes_delegated_to: Option<Address>,
    pub epochs_participated: u64,
    pub total_rewards_generated: U256,
    pub total_rewards_earned: U256,
}

#[derive(Debug, Clone)]
pub struct StakingRewardByEpoch {
    pub staker_address: Address,
    pub epoch: u64,
    pub staking_power: U256,
    pub percentage: f64,
    pub rewards_earned: U256,
}

#[derive(Debug, Clone)]
pub struct VoteDelegationPowerByEpoch {
    pub delegate_address: Address,
    pub epoch: u64,
    pub vote_power: U256,
    pub delegator_count: u64,
    pub delegators: Vec<Address>,
}

#[derive(Debug, Clone)]
pub struct RewardDelegationPowerByEpoch {
    pub delegate_address: Address,
    pub epoch: u64,
    pub reward_power: U256,
    pub delegator_count: u64,
    pub delegators: Vec<Address>,
}

#[derive(Debug, Clone)]
pub struct VoteDelegationPowerAggregate {
    pub delegate_address: Address,
    pub total_vote_power: U256,
    pub delegator_count: u64,
    pub delegators: Vec<Address>,
    pub epochs_participated: u64,
}

#[derive(Debug, Clone)]
pub struct RewardDelegationPowerAggregate {
    pub delegate_address: Address,
    pub total_reward_power: U256,
    pub delegator_count: u64,
    pub delegators: Vec<Address>,
    pub epochs_participated: u64,
}

/// Global PoVW summary statistics across all epochs
#[derive(Debug, Clone)]
pub struct PoVWSummaryStats {
    pub total_epochs_with_work: u64,
    pub total_unique_work_log_ids: u64,
    pub total_work_all_time: U256,
    pub total_emissions_all_time: U256,
    pub total_capped_rewards_all_time: U256,
    pub total_uncapped_rewards_all_time: U256,
    pub updated_at: Option<String>,
}

/// Per-epoch PoVW summary
#[derive(Debug, Clone)]
pub struct EpochPoVWSummary {
    pub epoch: u64,
    pub total_work: U256,
    pub total_emissions: U256,
    pub total_capped_rewards: U256,
    pub total_uncapped_rewards: U256,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    pub num_participants: u64,
    pub updated_at: Option<String>,
}

/// Global staking summary statistics
#[derive(Debug, Clone)]
pub struct StakingSummaryStats {
    pub current_total_staked: U256,
    pub total_unique_stakers: u64,
    pub current_active_stakers: u64,
    pub current_withdrawing: u64,
    pub total_staking_emissions_all_time: Option<U256>,
    pub updated_at: Option<String>,
}

/// Per-epoch staking summary
#[derive(Debug, Clone)]
pub struct EpochStakingSummary {
    pub epoch: u64,
    pub total_staked: U256,
    pub num_stakers: u64,
    pub num_withdrawing: u64,
    pub total_staking_emissions: U256,
    pub total_staking_power: U256,
    pub num_reward_recipients: u64,
    pub epoch_start_time: u64,
    pub epoch_end_time: u64,
    pub updated_at: Option<String>,
}

#[async_trait]
pub trait RewardsIndexerDb {
    /// Upsert rewards data for a specific epoch
    async fn upsert_povw_rewards_by_epoch(
        &self,
        epoch: u64,
        rewards: Vec<PovwRewardByEpoch>,
    ) -> Result<(), DbError>;

    /// Get rewards for a specific epoch with pagination
    async fn get_povw_rewards_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError>;

    /// Get all rewards for a specific work log ID
    async fn get_povw_rewards_by_work_log(
        &self,
        work_log_id: Address,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError>;

    /// Upsert aggregate rewards data
    async fn upsert_povw_rewards_aggregate(
        &self,
        aggregates: Vec<PovwRewardAggregate>,
    ) -> Result<(), DbError>;

    /// Get aggregate rewards with pagination, sorted by total rewards
    async fn get_povw_rewards_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PovwRewardAggregate>, DbError>;

    /// Get PoVW rewards aggregate for a specific address
    async fn get_povw_rewards_aggregate_by_address(
        &self,
        address: Address,
    ) -> Result<Option<PovwRewardAggregate>, DbError>;

    /// Get the current epoch from indexer state
    async fn get_current_epoch(&self) -> Result<Option<u64>, DbError>;

    /// Set the current epoch in indexer state
    async fn set_current_epoch(&self, epoch: u64) -> Result<(), DbError>;

    /// Get the last processed block for rewards indexer
    async fn get_last_rewards_block(&self) -> Result<Option<u64>, DbError>;

    /// Set the last processed block for rewards indexer
    async fn set_last_rewards_block(&self, block: u64) -> Result<(), DbError>;

    /// Upsert staking positions for a specific epoch
    async fn upsert_staking_positions_by_epoch(
        &self,
        epoch: u64,
        positions: Vec<StakingPositionByEpoch>,
    ) -> Result<(), DbError>;

    /// Get staking positions for a specific epoch with pagination
    async fn get_staking_positions_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingPositionByEpoch>, DbError>;

    /// Upsert aggregate staking positions
    async fn upsert_staking_positions_aggregate(
        &self,
        aggregates: Vec<StakingPositionAggregate>,
    ) -> Result<(), DbError>;

    /// Get aggregate staking positions with pagination, sorted by total staked
    async fn get_staking_positions_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingPositionAggregate>, DbError>;

    /// Get staking position aggregate for a specific address
    async fn get_staking_position_aggregate_by_address(
        &self,
        address: Address,
    ) -> Result<Option<StakingPositionAggregate>, DbError>;

    /// Upsert vote delegation powers for a specific epoch
    async fn upsert_vote_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        powers: Vec<VoteDelegationPowerByEpoch>,
    ) -> Result<(), DbError>;

    /// Get vote delegation powers for a specific epoch with pagination
    async fn get_vote_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<VoteDelegationPowerByEpoch>, DbError>;

    /// Upsert aggregate vote delegation powers
    async fn upsert_vote_delegation_powers_aggregate(
        &self,
        aggregates: Vec<VoteDelegationPowerAggregate>,
    ) -> Result<(), DbError>;

    /// Get aggregate vote delegation powers with pagination, sorted by total power
    async fn get_vote_delegation_powers_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<VoteDelegationPowerAggregate>, DbError>;

    /// Upsert reward delegation powers for a specific epoch
    async fn upsert_reward_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        powers: Vec<RewardDelegationPowerByEpoch>,
    ) -> Result<(), DbError>;

    /// Get reward delegation powers for a specific epoch with pagination
    async fn get_reward_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<RewardDelegationPowerByEpoch>, DbError>;

    /// Upsert aggregate reward delegation powers
    async fn upsert_reward_delegation_powers_aggregate(
        &self,
        aggregates: Vec<RewardDelegationPowerAggregate>,
    ) -> Result<(), DbError>;

    /// Get aggregate reward delegation powers with pagination, sorted by total power
    async fn get_reward_delegation_powers_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<RewardDelegationPowerAggregate>, DbError>;

    /// Get staking history for a specific address across epochs
    async fn get_staking_history_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<StakingPositionByEpoch>, DbError>;

    /// Get PoVW rewards history for a specific address across epochs
    async fn get_povw_rewards_history_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError>;

    /// Get vote delegations received history for a specific address across epochs
    async fn get_vote_delegations_received_history(
        &self,
        delegate_address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<VoteDelegationPowerByEpoch>, DbError>;

    /// Get reward delegations received history for a specific address across epochs
    async fn get_reward_delegations_received_history(
        &self,
        delegate_address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<RewardDelegationPowerByEpoch>, DbError>;

    /// Upsert global PoVW summary statistics
    async fn upsert_povw_summary_stats(&self, stats: PoVWSummaryStats) -> Result<(), DbError>;

    /// Get global PoVW summary statistics
    async fn get_povw_summary_stats(&self) -> Result<Option<PoVWSummaryStats>, DbError>;

    /// Upsert per-epoch PoVW summary
    async fn upsert_epoch_povw_summary(
        &self,
        epoch: u64,
        summary: EpochPoVWSummary,
    ) -> Result<(), DbError>;

    /// Get per-epoch PoVW summary
    async fn get_epoch_povw_summary(&self, epoch: u64)
        -> Result<Option<EpochPoVWSummary>, DbError>;

    /// Upsert global staking summary statistics
    async fn upsert_staking_summary_stats(&self, stats: StakingSummaryStats)
        -> Result<(), DbError>;

    /// Get global staking summary statistics
    async fn get_staking_summary_stats(&self) -> Result<Option<StakingSummaryStats>, DbError>;

    /// Upsert per-epoch staking summary
    async fn upsert_epoch_staking_summary(
        &self,
        epoch: u64,
        summary: EpochStakingSummary,
    ) -> Result<(), DbError>;

    /// Get per-epoch staking summary
    async fn get_epoch_staking_summary(
        &self,
        epoch: u64,
    ) -> Result<Option<EpochStakingSummary>, DbError>;

    /// Upsert staking rewards for a specific epoch
    async fn upsert_staking_rewards_by_epoch(
        &self,
        epoch: u64,
        rewards: Vec<StakingRewardByEpoch>,
    ) -> Result<(), DbError>;

    /// Get staking rewards for a specific epoch with pagination
    async fn get_staking_rewards_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingRewardByEpoch>, DbError>;

    /// Get staking rewards for a specific address across epochs
    async fn get_staking_rewards_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<StakingRewardByEpoch>, DbError>;

    /// Get all epoch PoVW summaries
    async fn get_all_epoch_povw_summaries(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<EpochPoVWSummary>, DbError>;

    /// Get all epoch staking summaries
    async fn get_all_epoch_staking_summaries(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<EpochStakingSummary>, DbError>;
}

// Batch insert chunk size to avoid parameter limits
// PostgreSQL: 65535 max params
// Using a conservative chunk size that avoids large statements
const BATCH_INSERT_CHUNK_SIZE: usize = 75;

pub struct RewardsDb {
    pool: PgPool,
}

impl RewardsDb {
    /// Create a new RewardsDb instance.
    ///
    /// # Arguments
    /// * `database_url` - Database connection string
    /// * `pool_options` - Optional pool configuration.
    /// * `skip_migrations` - If `true`, skips running migrations. Useful for read-only connections
    pub async fn new(
        database_url: &str,
        pool_options: Option<PgPoolOptions>,
        skip_migrations: bool,
    ) -> Result<Self, DbError> {
        // Parse connection options to configure slow query logging
        let mut connect_opts = PgConnectOptions::from_str(database_url)?;

        // Configure slow query logging: only log queries that take over 2 seconds
        connect_opts = connect_opts.log_slow_statements(LevelFilter::Warn, Duration::from_secs(2)); // Only warn for queries > 2s

        let pool = if let Some(opts) = pool_options {
            opts.connect_with(connect_opts).await?
        } else {
            // Lambda-optimized defaults
            PgPoolOptions::new()
                .max_connections(3) // Lambda: 25 lambdas × 3 = 75 max connections
                .acquire_timeout(Duration::from_secs(5)) // Lambda: fail fast for users
                .idle_timeout(Some(Duration::from_secs(300))) // Lambda: match container warm time
                .max_lifetime(Some(Duration::from_secs(300))) // Lambda: 5 min max
                .connect_with(connect_opts)
                .await?
        };

        if !skip_migrations {
            // Run migrations
            sqlx::migrate!("./migrations").run(&pool).await?;
        }

        Ok(Self { pool })
    }
}

#[async_trait]
impl RewardsIndexerDb for RewardsDb {
    async fn upsert_povw_rewards_by_epoch(
        &self,
        epoch: u64,
        rewards: Vec<PovwRewardByEpoch>,
    ) -> Result<(), DbError> {
        if rewards.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in rewards.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4,
                    param_idx + 5,
                    param_idx + 6,
                    param_idx + 7,
                    param_idx + 8
                ));
                param_idx += 9;
            }

            let query = format!(
                r#"INSERT INTO povw_rewards_by_epoch
                (work_log_id, epoch, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount, updated_at)
                VALUES {}
                ON CONFLICT (work_log_id, epoch)
                DO UPDATE SET
                    work_submitted = EXCLUDED.work_submitted,
                    percentage = EXCLUDED.percentage,
                    uncapped_rewards = EXCLUDED.uncapped_rewards,
                    reward_cap = EXCLUDED.reward_cap,
                    actual_rewards = EXCLUDED.actual_rewards,
                    is_capped = EXCLUDED.is_capped,
                    staked_amount = EXCLUDED.staked_amount,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for reward in chunk {
                q = q
                    .bind(format!("{:#x}", reward.work_log_id))
                    .bind(epoch as i64)
                    .bind(pad_u256(reward.work_submitted))
                    .bind(reward.percentage)
                    .bind(pad_u256(reward.uncapped_rewards))
                    .bind(pad_u256(reward.reward_cap))
                    .bind(pad_u256(reward.actual_rewards))
                    .bind(if reward.is_capped { 1i32 } else { 0i32 })
                    .bind(pad_u256(reward.staked_amount));
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_povw_rewards_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError> {
        let query = r#"
            SELECT work_log_id, epoch, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount
            FROM povw_rewards_by_epoch
            WHERE epoch = $1
            ORDER BY work_submitted DESC
            LIMIT $2 OFFSET $3
        "#;

        let rows = sqlx::query(query)
            .bind(epoch as i64)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(PovwRewardByEpoch {
                work_log_id: Address::from_str(&row.get::<String, _>("work_log_id"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                work_submitted: unpad_u256(&row.get::<String, _>("work_submitted"))?,
                percentage: row.get("percentage"),
                uncapped_rewards: unpad_u256(&row.get::<String, _>("uncapped_rewards"))?,
                reward_cap: unpad_u256(&row.get::<String, _>("reward_cap"))?,
                actual_rewards: unpad_u256(&row.get::<String, _>("actual_rewards"))?,
                is_capped: row.get::<i32, _>("is_capped") != 0,
                staked_amount: unpad_u256(&row.get::<String, _>("staked_amount"))?,
            });
        }

        Ok(results)
    }

    async fn get_povw_rewards_by_work_log(
        &self,
        work_log_id: Address,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError> {
        let query = r#"
            SELECT work_log_id, epoch, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount
            FROM povw_rewards_by_epoch
            WHERE work_log_id = $1
            ORDER BY epoch DESC
        "#;

        let rows =
            sqlx::query(query).bind(format!("{:#x}", work_log_id)).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(PovwRewardByEpoch {
                work_log_id,
                epoch: row.get::<i64, _>("epoch") as u64,
                work_submitted: unpad_u256(&row.get::<String, _>("work_submitted"))?,
                percentage: row.get("percentage"),
                uncapped_rewards: unpad_u256(&row.get::<String, _>("uncapped_rewards"))?,
                reward_cap: unpad_u256(&row.get::<String, _>("reward_cap"))?,
                actual_rewards: unpad_u256(&row.get::<String, _>("actual_rewards"))?,
                is_capped: row.get::<i32, _>("is_capped") != 0,
                staked_amount: unpad_u256(&row.get::<String, _>("staked_amount"))?,
            });
        }

        Ok(results)
    }

    async fn upsert_povw_rewards_aggregate(
        &self,
        aggregates: Vec<PovwRewardAggregate>,
    ) -> Result<(), DbError> {
        if aggregates.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in aggregates.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO povw_rewards_aggregate
                (work_log_id, total_work_submitted, total_actual_rewards, total_uncapped_rewards, epochs_participated, updated_at)
                VALUES {}
                ON CONFLICT (work_log_id)
                DO UPDATE SET
                    total_work_submitted = EXCLUDED.total_work_submitted,
                    total_actual_rewards = EXCLUDED.total_actual_rewards,
                    total_uncapped_rewards = EXCLUDED.total_uncapped_rewards,
                    epochs_participated = EXCLUDED.epochs_participated,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for agg in chunk {
                q = q
                    .bind(format!("{:#x}", agg.work_log_id))
                    .bind(pad_u256(agg.total_work_submitted))
                    .bind(pad_u256(agg.total_actual_rewards))
                    .bind(pad_u256(agg.total_uncapped_rewards))
                    .bind(agg.epochs_participated as i64);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_povw_rewards_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<PovwRewardAggregate>, DbError> {
        let query = r#"
            SELECT work_log_id, total_work_submitted, total_actual_rewards, total_uncapped_rewards, epochs_participated
            FROM povw_rewards_aggregate
            ORDER BY total_work_submitted DESC
            LIMIT $1 OFFSET $2
        "#;

        let rows =
            sqlx::query(query).bind(limit as i64).bind(offset as i64).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(PovwRewardAggregate {
                work_log_id: Address::from_str(&row.get::<String, _>("work_log_id"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_work_submitted: unpad_u256(&row.get::<String, _>("total_work_submitted"))?,
                total_actual_rewards: unpad_u256(&row.get::<String, _>("total_actual_rewards"))?,
                total_uncapped_rewards: unpad_u256(
                    &row.get::<String, _>("total_uncapped_rewards"),
                )?,
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
            });
        }

        Ok(results)
    }

    async fn get_povw_rewards_aggregate_by_address(
        &self,
        address: Address,
    ) -> Result<Option<PovwRewardAggregate>, DbError> {
        let query = r#"
            SELECT work_log_id, total_work_submitted, total_actual_rewards, total_uncapped_rewards, epochs_participated
            FROM povw_rewards_aggregate
            WHERE work_log_id = $1
        "#;

        let row =
            sqlx::query(query).bind(format!("{:#x}", address)).fetch_optional(&self.pool).await?;

        if let Some(row) = row {
            Ok(Some(PovwRewardAggregate {
                work_log_id: Address::from_str(&row.get::<String, _>("work_log_id"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_work_submitted: unpad_u256(&row.get::<String, _>("total_work_submitted"))?,
                total_actual_rewards: unpad_u256(&row.get::<String, _>("total_actual_rewards"))?,
                total_uncapped_rewards: unpad_u256(
                    &row.get::<String, _>("total_uncapped_rewards"),
                )?,
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
            }))
        } else {
            Ok(None)
        }
    }

    async fn get_current_epoch(&self) -> Result<Option<u64>, DbError> {
        let query = "SELECT value FROM indexer_state WHERE key = 'current_epoch'";
        let result = sqlx::query(query).fetch_optional(&self.pool).await?;

        match result {
            Some(row) => {
                let value: String = row.get("value");
                Ok(Some(value.parse().map_err(|_| DbError::BadBlockNumb(value))?))
            }
            None => Ok(None),
        }
    }

    async fn set_current_epoch(&self, epoch: u64) -> Result<(), DbError> {
        let query = r#"
            INSERT INTO indexer_state (key, value, updated_at)
            VALUES ('current_epoch', $1, CURRENT_TIMESTAMP)
            ON CONFLICT (key)
            DO UPDATE SET value = $1, updated_at = CURRENT_TIMESTAMP
        "#;

        sqlx::query(query).bind(epoch.to_string()).execute(&self.pool).await?;

        Ok(())
    }

    async fn get_last_rewards_block(&self) -> Result<Option<u64>, DbError> {
        let query = "SELECT value FROM indexer_state WHERE key = 'last_rewards_block'";
        let result = sqlx::query(query).fetch_optional(&self.pool).await?;

        match result {
            Some(row) => {
                let value: String = row.get("value");
                Ok(Some(value.parse().map_err(|_| DbError::BadBlockNumb(value))?))
            }
            None => Ok(None),
        }
    }

    async fn set_last_rewards_block(&self, block: u64) -> Result<(), DbError> {
        let query = r#"
            INSERT INTO indexer_state (key, value, updated_at)
            VALUES ('last_rewards_block', $1, CURRENT_TIMESTAMP)
            ON CONFLICT (key)
            DO UPDATE SET value = $1, updated_at = CURRENT_TIMESTAMP
        "#;

        sqlx::query(query).bind(block.to_string()).execute(&self.pool).await?;

        Ok(())
    }

    async fn upsert_staking_positions_by_epoch(
        &self,
        epoch: u64,
        positions: Vec<StakingPositionByEpoch>,
    ) -> Result<(), DbError> {
        if positions.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in positions.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4,
                    param_idx + 5,
                    param_idx + 6
                ));
                param_idx += 7;
            }

            let query = format!(
                r#"INSERT INTO staking_positions_by_epoch
                (staker_address, epoch, staked_amount, is_withdrawing, rewards_delegated_to, votes_delegated_to, rewards_generated, updated_at)
                VALUES {}
                ON CONFLICT (staker_address, epoch)
                DO UPDATE SET
                    staked_amount = EXCLUDED.staked_amount,
                    is_withdrawing = EXCLUDED.is_withdrawing,
                    rewards_delegated_to = EXCLUDED.rewards_delegated_to,
                    votes_delegated_to = EXCLUDED.votes_delegated_to,
                    rewards_generated = EXCLUDED.rewards_generated,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for position in chunk {
                q = q
                    .bind(format!("{:#x}", position.staker_address))
                    .bind(epoch as i64)
                    .bind(pad_u256(position.staked_amount))
                    .bind(if position.is_withdrawing { 1i32 } else { 0i32 })
                    .bind(position.rewards_delegated_to.map(|a| format!("{:#x}", a)))
                    .bind(position.votes_delegated_to.map(|a| format!("{:#x}", a)))
                    .bind(pad_u256(position.rewards_generated));
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_staking_positions_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingPositionByEpoch>, DbError> {
        let query = r#"
            SELECT staker_address, epoch, staked_amount, is_withdrawing, rewards_delegated_to, votes_delegated_to, rewards_generated
            FROM staking_positions_by_epoch
            WHERE epoch = $1
            ORDER BY staked_amount DESC
            LIMIT $2 OFFSET $3
        "#;

        let rows = sqlx::query(query)
            .bind(epoch as i64)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            let rewards_delegated_to: Option<String> = row.get("rewards_delegated_to");
            let votes_delegated_to: Option<String> = row.get("votes_delegated_to");

            results.push(StakingPositionByEpoch {
                staker_address: Address::from_str(&row.get::<String, _>("staker_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                staked_amount: unpad_u256(&row.get::<String, _>("staked_amount"))?,
                is_withdrawing: row.get::<i32, _>("is_withdrawing") != 0,
                rewards_delegated_to: rewards_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                votes_delegated_to: votes_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                rewards_generated: unpad_u256(&row.get::<String, _>("rewards_generated"))?,
            });
        }

        Ok(results)
    }

    async fn upsert_staking_positions_aggregate(
        &self,
        aggregates: Vec<StakingPositionAggregate>,
    ) -> Result<(), DbError> {
        if aggregates.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in aggregates.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4,
                    param_idx + 5,
                    param_idx + 6,
                    param_idx + 7
                ));
                param_idx += 8;
            }

            let query = format!(
                r#"INSERT INTO staking_positions_aggregate
                (staker_address, total_staked, is_withdrawing, rewards_delegated_to, votes_delegated_to, epochs_participated, total_rewards_generated, total_rewards_earned, updated_at)
                VALUES {}
                ON CONFLICT (staker_address)
                DO UPDATE SET
                    total_staked = EXCLUDED.total_staked,
                    is_withdrawing = EXCLUDED.is_withdrawing,
                    rewards_delegated_to = EXCLUDED.rewards_delegated_to,
                    votes_delegated_to = EXCLUDED.votes_delegated_to,
                    epochs_participated = EXCLUDED.epochs_participated,
                    total_rewards_generated = EXCLUDED.total_rewards_generated,
                    total_rewards_earned = EXCLUDED.total_rewards_earned,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for aggregate in chunk {
                q = q
                    .bind(format!("{:#x}", aggregate.staker_address))
                    .bind(pad_u256(aggregate.total_staked))
                    .bind(if aggregate.is_withdrawing { 1i32 } else { 0i32 })
                    .bind(aggregate.rewards_delegated_to.map(|a| format!("{:#x}", a)))
                    .bind(aggregate.votes_delegated_to.map(|a| format!("{:#x}", a)))
                    .bind(aggregate.epochs_participated as i64)
                    .bind(pad_u256(aggregate.total_rewards_generated))
                    .bind(pad_u256(aggregate.total_rewards_earned));
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_staking_positions_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingPositionAggregate>, DbError> {
        let query = r#"
            SELECT staker_address, total_staked, is_withdrawing, rewards_delegated_to, votes_delegated_to, epochs_participated, total_rewards_generated, total_rewards_earned
            FROM staking_positions_aggregate
            ORDER BY total_staked DESC
            LIMIT $1 OFFSET $2
        "#;

        let rows =
            sqlx::query(query).bind(limit as i64).bind(offset as i64).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let rewards_delegated_to: Option<String> = row.get("rewards_delegated_to");
            let votes_delegated_to: Option<String> = row.get("votes_delegated_to");

            results.push(StakingPositionAggregate {
                staker_address: Address::from_str(&row.get::<String, _>("staker_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_staked: unpad_u256(&row.get::<String, _>("total_staked"))?,
                is_withdrawing: row.get::<i32, _>("is_withdrawing") != 0,
                rewards_delegated_to: rewards_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                votes_delegated_to: votes_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
                total_rewards_generated: unpad_u256(
                    &row.get::<String, _>("total_rewards_generated"),
                )?,
                total_rewards_earned: unpad_u256(&row.get::<String, _>("total_rewards_earned"))?,
            });
        }

        Ok(results)
    }

    async fn get_staking_position_aggregate_by_address(
        &self,
        address: Address,
    ) -> Result<Option<StakingPositionAggregate>, DbError> {
        let query = r#"
            SELECT staker_address, total_staked, is_withdrawing, rewards_delegated_to, votes_delegated_to, epochs_participated, total_rewards_generated, total_rewards_earned
            FROM staking_positions_aggregate
            WHERE staker_address = $1
        "#;

        let row =
            sqlx::query(query).bind(format!("{:#x}", address)).fetch_optional(&self.pool).await?;

        if let Some(row) = row {
            let rewards_delegated_to: Option<String> = row.get("rewards_delegated_to");
            let votes_delegated_to: Option<String> = row.get("votes_delegated_to");

            Ok(Some(StakingPositionAggregate {
                staker_address: Address::from_str(&row.get::<String, _>("staker_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_staked: unpad_u256(&row.get::<String, _>("total_staked"))?,
                is_withdrawing: row.get::<i32, _>("is_withdrawing") != 0,
                rewards_delegated_to: rewards_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                votes_delegated_to: votes_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
                total_rewards_generated: unpad_u256(
                    &row.get::<String, _>("total_rewards_generated"),
                )?,
                total_rewards_earned: unpad_u256(&row.get::<String, _>("total_rewards_earned"))?,
            }))
        } else {
            Ok(None)
        }
    }

    async fn upsert_vote_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        powers: Vec<VoteDelegationPowerByEpoch>,
    ) -> Result<(), DbError> {
        if powers.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in powers.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO vote_delegation_powers_by_epoch
                (delegate_address, epoch, vote_power, delegator_count, delegators, updated_at)
                VALUES {}
                ON CONFLICT (delegate_address, epoch)
                DO UPDATE SET
                    vote_power = EXCLUDED.vote_power,
                    delegator_count = EXCLUDED.delegator_count,
                    delegators = EXCLUDED.delegators,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for power in chunk {
                let delegators_json = serde_json::to_string(
                    &power.delegators.iter().map(|a| format!("{:#x}", a)).collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());

                q = q
                    .bind(format!("{:#x}", power.delegate_address))
                    .bind(epoch as i64)
                    .bind(pad_u256(power.vote_power))
                    .bind(power.delegator_count as i32)
                    .bind(delegators_json);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_vote_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<VoteDelegationPowerByEpoch>, DbError> {
        let query = r#"
            SELECT delegate_address, epoch, vote_power, delegator_count, delegators
            FROM vote_delegation_powers_by_epoch
            WHERE epoch = $1
            ORDER BY vote_power DESC
            LIMIT $2 OFFSET $3
        "#;

        let rows = sqlx::query(query)
            .bind(epoch as i64)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(VoteDelegationPowerByEpoch {
                delegate_address: Address::from_str(&row.get::<String, _>("delegate_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                vote_power: unpad_u256(&row.get::<String, _>("vote_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
            });
        }

        Ok(results)
    }

    async fn upsert_vote_delegation_powers_aggregate(
        &self,
        aggregates: Vec<VoteDelegationPowerAggregate>,
    ) -> Result<(), DbError> {
        if aggregates.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in aggregates.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO vote_delegation_powers_aggregate
                (delegate_address, total_vote_power, delegator_count, delegators, epochs_participated, updated_at)
                VALUES {}
                ON CONFLICT (delegate_address)
                DO UPDATE SET
                    total_vote_power = EXCLUDED.total_vote_power,
                    delegator_count = EXCLUDED.delegator_count,
                    delegators = EXCLUDED.delegators,
                    epochs_participated = EXCLUDED.epochs_participated,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for aggregate in chunk {
                let delegators_json = serde_json::to_string(
                    &aggregate.delegators.iter().map(|a| format!("{:#x}", a)).collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());

                q = q
                    .bind(format!("{:#x}", aggregate.delegate_address))
                    .bind(pad_u256(aggregate.total_vote_power))
                    .bind(aggregate.delegator_count as i32)
                    .bind(delegators_json)
                    .bind(aggregate.epochs_participated as i64);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_vote_delegation_powers_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<VoteDelegationPowerAggregate>, DbError> {
        let query = r#"
            SELECT delegate_address, total_vote_power, delegator_count, delegators, epochs_participated
            FROM vote_delegation_powers_aggregate
            ORDER BY total_vote_power DESC
            LIMIT $1 OFFSET $2
        "#;

        let rows =
            sqlx::query(query).bind(limit as i64).bind(offset as i64).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(VoteDelegationPowerAggregate {
                delegate_address: Address::from_str(&row.get::<String, _>("delegate_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_vote_power: unpad_u256(&row.get::<String, _>("total_vote_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
            });
        }

        Ok(results)
    }

    async fn upsert_reward_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        powers: Vec<RewardDelegationPowerByEpoch>,
    ) -> Result<(), DbError> {
        if powers.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in powers.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO reward_delegation_powers_by_epoch
                (delegate_address, epoch, reward_power, delegator_count, delegators, updated_at)
                VALUES {}
                ON CONFLICT (delegate_address, epoch)
                DO UPDATE SET
                    reward_power = EXCLUDED.reward_power,
                    delegator_count = EXCLUDED.delegator_count,
                    delegators = EXCLUDED.delegators,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for power in chunk {
                let delegators_json = serde_json::to_string(
                    &power.delegators.iter().map(|a| format!("{:#x}", a)).collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());

                q = q
                    .bind(format!("{:#x}", power.delegate_address))
                    .bind(epoch as i64)
                    .bind(pad_u256(power.reward_power))
                    .bind(power.delegator_count as i32)
                    .bind(delegators_json);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_reward_delegation_powers_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<RewardDelegationPowerByEpoch>, DbError> {
        let query = r#"
            SELECT delegate_address, epoch, reward_power, delegator_count, delegators
            FROM reward_delegation_powers_by_epoch
            WHERE epoch = $1
            ORDER BY reward_power DESC
            LIMIT $2 OFFSET $3
        "#;

        let rows = sqlx::query(query)
            .bind(epoch as i64)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(RewardDelegationPowerByEpoch {
                delegate_address: Address::from_str(&row.get::<String, _>("delegate_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                reward_power: unpad_u256(&row.get::<String, _>("reward_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
            });
        }

        Ok(results)
    }

    async fn upsert_reward_delegation_powers_aggregate(
        &self,
        aggregates: Vec<RewardDelegationPowerAggregate>,
    ) -> Result<(), DbError> {
        if aggregates.is_empty() {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;

        // Process in chunks to avoid parameter limits
        for chunk in aggregates.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${},CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO reward_delegation_powers_aggregate
                (delegate_address, total_reward_power, delegator_count, delegators, epochs_participated, updated_at)
                VALUES {}
                ON CONFLICT (delegate_address)
                DO UPDATE SET
                    total_reward_power = EXCLUDED.total_reward_power,
                    delegator_count = EXCLUDED.delegator_count,
                    delegators = EXCLUDED.delegators,
                    epochs_participated = EXCLUDED.epochs_participated,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for aggregate in chunk {
                let delegators_json = serde_json::to_string(
                    &aggregate.delegators.iter().map(|a| format!("{:#x}", a)).collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());

                q = q
                    .bind(format!("{:#x}", aggregate.delegate_address))
                    .bind(pad_u256(aggregate.total_reward_power))
                    .bind(aggregate.delegator_count as i32)
                    .bind(delegators_json)
                    .bind(aggregate.epochs_participated as i64);
            }
            q.execute(&mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn get_reward_delegation_powers_aggregate(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<RewardDelegationPowerAggregate>, DbError> {
        let query = r#"
            SELECT delegate_address, total_reward_power, delegator_count, delegators, epochs_participated
            FROM reward_delegation_powers_aggregate
            ORDER BY total_reward_power DESC
            LIMIT $1 OFFSET $2
        "#;

        let rows =
            sqlx::query(query).bind(limit as i64).bind(offset as i64).fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(RewardDelegationPowerAggregate {
                delegate_address: Address::from_str(&row.get::<String, _>("delegate_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_reward_power: unpad_u256(&row.get::<String, _>("total_reward_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
            });
        }

        Ok(results)
    }

    async fn get_staking_history_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<StakingPositionByEpoch>, DbError> {
        let mut query = String::from(
            "SELECT staker_address, epoch, staked_amount, is_withdrawing,
                    rewards_delegated_to, votes_delegated_to, rewards_generated
             FROM staking_positions_by_epoch
             WHERE staker_address = $1",
        );

        let mut bind_count = 1;
        if start_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch >= ${}", bind_count));
        }
        if end_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch <= ${}", bind_count));
        }
        query.push_str(" ORDER BY epoch DESC");

        let mut q = sqlx::query(&query).bind(format!("{:#x}", address));
        if let Some(start) = start_epoch {
            q = q.bind(start as i64);
        }
        if let Some(end) = end_epoch {
            q = q.bind(end as i64);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let rewards_delegated_to: Option<String> = row.get("rewards_delegated_to");
            let votes_delegated_to: Option<String> = row.get("votes_delegated_to");

            results.push(StakingPositionByEpoch {
                staker_address: address,
                epoch: row.get::<i64, _>("epoch") as u64,
                staked_amount: unpad_u256(&row.get::<String, _>("staked_amount"))?,
                is_withdrawing: row.get::<i32, _>("is_withdrawing") != 0,
                rewards_delegated_to: rewards_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                votes_delegated_to: votes_delegated_to.and_then(|s| Address::from_str(&s).ok()),
                rewards_generated: unpad_u256(&row.get::<String, _>("rewards_generated"))?,
            });
        }

        Ok(results)
    }

    async fn get_povw_rewards_history_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<PovwRewardByEpoch>, DbError> {
        let mut query = String::from(
            "SELECT work_log_id, epoch, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount
             FROM povw_rewards_by_epoch
             WHERE work_log_id = $1"
        );

        let mut bind_count = 1;
        if start_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch >= ${}", bind_count));
        }
        if end_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch <= ${}", bind_count));
        }
        query.push_str(" ORDER BY epoch DESC");

        let mut q = sqlx::query(&query).bind(format!("{:#x}", address));
        if let Some(start) = start_epoch {
            q = q.bind(start as i64);
        }
        if let Some(end) = end_epoch {
            q = q.bind(end as i64);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(PovwRewardByEpoch {
                work_log_id: address,
                epoch: row.get::<i64, _>("epoch") as u64,
                work_submitted: unpad_u256(&row.get::<String, _>("work_submitted"))?,
                percentage: row.get("percentage"),
                uncapped_rewards: unpad_u256(&row.get::<String, _>("uncapped_rewards"))?,
                reward_cap: unpad_u256(&row.get::<String, _>("reward_cap"))?,
                actual_rewards: unpad_u256(&row.get::<String, _>("actual_rewards"))?,
                is_capped: row.get::<i32, _>("is_capped") != 0,
                staked_amount: unpad_u256(&row.get::<String, _>("staked_amount"))?,
            });
        }

        Ok(results)
    }

    async fn get_vote_delegations_received_history(
        &self,
        delegate_address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<VoteDelegationPowerByEpoch>, DbError> {
        let mut query = String::from(
            "SELECT delegate_address, epoch, vote_power, delegator_count, delegators
             FROM vote_delegation_powers_by_epoch
             WHERE delegate_address = $1",
        );

        let mut bind_count = 1;
        if start_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch >= ${}", bind_count));
        }
        if end_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch <= ${}", bind_count));
        }
        query.push_str(" ORDER BY epoch DESC");

        let mut q = sqlx::query(&query).bind(format!("{:#x}", delegate_address));
        if let Some(start) = start_epoch {
            q = q.bind(start as i64);
        }
        if let Some(end) = end_epoch {
            q = q.bind(end as i64);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(VoteDelegationPowerByEpoch {
                delegate_address,
                epoch: row.get::<i64, _>("epoch") as u64,
                vote_power: unpad_u256(&row.get::<String, _>("vote_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
            });
        }

        Ok(results)
    }

    async fn get_reward_delegations_received_history(
        &self,
        delegate_address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<RewardDelegationPowerByEpoch>, DbError> {
        let mut query = String::from(
            "SELECT delegate_address, epoch, reward_power, delegator_count, delegators
             FROM reward_delegation_powers_by_epoch
             WHERE delegate_address = $1",
        );

        let mut bind_count = 1;
        if start_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch >= ${}", bind_count));
        }
        if end_epoch.is_some() {
            bind_count += 1;
            query.push_str(&format!(" AND epoch <= ${}", bind_count));
        }
        query.push_str(" ORDER BY epoch DESC");

        let mut q = sqlx::query(&query).bind(format!("{:#x}", delegate_address));
        if let Some(start) = start_epoch {
            q = q.bind(start as i64);
        }
        if let Some(end) = end_epoch {
            q = q.bind(end as i64);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::new();
        for row in rows {
            let delegators_json: String = row.get("delegators");
            let delegator_addrs: Vec<String> = match serde_json::from_str(&delegators_json) {
                Ok(addrs) => addrs,
                Err(e) => {
                    tracing::warn!("Failed to parse delegators JSON: {}, using empty vec", e);
                    Vec::new()
                }
            };
            let delegators: Vec<Address> =
                delegator_addrs.iter().filter_map(|s| Address::from_str(s).ok()).collect();

            results.push(RewardDelegationPowerByEpoch {
                delegate_address,
                epoch: row.get::<i64, _>("epoch") as u64,
                reward_power: unpad_u256(&row.get::<String, _>("reward_power"))?,
                delegator_count: row.get::<i32, _>("delegator_count") as u64,
                delegators,
            });
        }

        Ok(results)
    }

    async fn upsert_povw_summary_stats(&self, stats: PoVWSummaryStats) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO povw_summary_stats
             (id, total_epochs_with_work, total_unique_work_log_ids,
              total_work_all_time, total_emissions_all_time,
              total_capped_rewards_all_time, total_uncapped_rewards_all_time, updated_at)
             VALUES (1, $1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (id) DO UPDATE SET
                total_epochs_with_work = $1,
                total_unique_work_log_ids = $2,
                total_work_all_time = $3,
                total_emissions_all_time = $4,
                total_capped_rewards_all_time = $5,
                total_uncapped_rewards_all_time = $6,
                updated_at = $7",
        )
        .bind(stats.total_epochs_with_work as i64)
        .bind(stats.total_unique_work_log_ids as i64)
        .bind(pad_u256(stats.total_work_all_time))
        .bind(pad_u256(stats.total_emissions_all_time))
        .bind(pad_u256(stats.total_capped_rewards_all_time))
        .bind(pad_u256(stats.total_uncapped_rewards_all_time))
        .bind(stats.updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(DbError::from)?;
        Ok(())
    }

    async fn get_povw_summary_stats(&self) -> Result<Option<PoVWSummaryStats>, DbError> {
        let row = sqlx::query(
            "SELECT total_epochs_with_work, total_unique_work_log_ids,
                    total_work_all_time, total_emissions_all_time,
                    total_capped_rewards_all_time, total_uncapped_rewards_all_time, updated_at
             FROM povw_summary_stats
             WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)?;

        if let Some(row) = row {
            Ok(Some(PoVWSummaryStats {
                total_epochs_with_work: row.get::<i64, _>("total_epochs_with_work") as u64,
                total_unique_work_log_ids: row.get::<i64, _>("total_unique_work_log_ids") as u64,
                total_work_all_time: unpad_u256(&row.get::<String, _>("total_work_all_time"))?,
                total_emissions_all_time: unpad_u256(
                    &row.get::<String, _>("total_emissions_all_time"),
                )?,
                total_capped_rewards_all_time: unpad_u256(
                    &row.get::<String, _>("total_capped_rewards_all_time"),
                )?,
                total_uncapped_rewards_all_time: unpad_u256(
                    &row.get::<String, _>("total_uncapped_rewards_all_time"),
                )?,
                updated_at: row.get::<Option<String>, _>("updated_at"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn upsert_epoch_povw_summary(
        &self,
        epoch: u64,
        summary: EpochPoVWSummary,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO epoch_povw_summary
             (epoch, total_work, total_emissions, total_capped_rewards,
              total_uncapped_rewards, epoch_start_time, epoch_end_time, num_participants, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             ON CONFLICT (epoch) DO UPDATE SET
                total_work = $2,
                total_emissions = $3,
                total_capped_rewards = $4,
                total_uncapped_rewards = $5,
                epoch_start_time = $6,
                epoch_end_time = $7,
                num_participants = $8,
                updated_at = $9",
        )
        .bind(epoch as i64)
        .bind(pad_u256(summary.total_work))
        .bind(pad_u256(summary.total_emissions))
        .bind(pad_u256(summary.total_capped_rewards))
        .bind(pad_u256(summary.total_uncapped_rewards))
        .bind(summary.epoch_start_time as i64)
        .bind(summary.epoch_end_time as i64)
        .bind(summary.num_participants as i64)
        .bind(summary.updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(DbError::from)?;
        Ok(())
    }

    async fn get_epoch_povw_summary(
        &self,
        epoch: u64,
    ) -> Result<Option<EpochPoVWSummary>, DbError> {
        let row = sqlx::query(
            "SELECT epoch, total_work, total_emissions, total_capped_rewards,
                    total_uncapped_rewards, epoch_start_time, epoch_end_time, num_participants, updated_at
             FROM epoch_povw_summary
             WHERE epoch = $1",
        )
        .bind(epoch as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)?;

        if let Some(row) = row {
            Ok(Some(EpochPoVWSummary {
                epoch: row.get::<i64, _>("epoch") as u64,
                total_work: unpad_u256(&row.get::<String, _>("total_work"))?,
                total_emissions: unpad_u256(&row.get::<String, _>("total_emissions"))?,
                total_capped_rewards: unpad_u256(&row.get::<String, _>("total_capped_rewards"))?,
                total_uncapped_rewards: unpad_u256(
                    &row.get::<String, _>("total_uncapped_rewards"),
                )?,
                epoch_start_time: row.get::<i64, _>("epoch_start_time") as u64,
                epoch_end_time: row.get::<i64, _>("epoch_end_time") as u64,
                num_participants: row.get::<i64, _>("num_participants") as u64,
                updated_at: row.get::<Option<String>, _>("updated_at"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn upsert_staking_summary_stats(
        &self,
        stats: StakingSummaryStats,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO staking_summary_stats
             (id, current_total_staked, total_unique_stakers,
              current_active_stakers, current_withdrawing,
              total_staking_emissions_all_time, updated_at)
             VALUES (1, $1, $2, $3, $4, $5, $6)
             ON CONFLICT (id) DO UPDATE SET
                current_total_staked = $1,
                total_unique_stakers = $2,
                current_active_stakers = $3,
                current_withdrawing = $4,
                total_staking_emissions_all_time = $5,
                updated_at = $6",
        )
        .bind(pad_u256(stats.current_total_staked))
        .bind(stats.total_unique_stakers as i64)
        .bind(stats.current_active_stakers as i64)
        .bind(stats.current_withdrawing as i64)
        .bind(stats.total_staking_emissions_all_time.map(pad_u256))
        .bind(stats.updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(DbError::from)?;
        Ok(())
    }

    async fn get_staking_summary_stats(&self) -> Result<Option<StakingSummaryStats>, DbError> {
        let row = sqlx::query(
            "SELECT current_total_staked, total_unique_stakers,
                    current_active_stakers, current_withdrawing,
                    total_staking_emissions_all_time, updated_at
             FROM staking_summary_stats
             WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)?;

        if let Some(row) = row {
            Ok(Some(StakingSummaryStats {
                current_total_staked: unpad_u256(&row.get::<String, _>("current_total_staked"))?,
                total_unique_stakers: row.get::<i64, _>("total_unique_stakers") as u64,
                current_active_stakers: row.get::<i64, _>("current_active_stakers") as u64,
                current_withdrawing: row.get::<i64, _>("current_withdrawing") as u64,
                total_staking_emissions_all_time: row
                    .get::<Option<String>, _>("total_staking_emissions_all_time")
                    .and_then(|s| unpad_u256(&s).ok()),
                updated_at: row.get::<Option<String>, _>("updated_at"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn upsert_epoch_staking_summary(
        &self,
        epoch: u64,
        summary: EpochStakingSummary,
    ) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO epoch_staking_summary
             (epoch, total_staked, num_stakers, num_withdrawing,
              total_staking_emissions, total_staking_power,
              num_reward_recipients, epoch_start_time, epoch_end_time, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (epoch) DO UPDATE SET
                total_staked = $2,
                num_stakers = $3,
                num_withdrawing = $4,
                total_staking_emissions = $5,
                total_staking_power = $6,
                num_reward_recipients = $7,
                epoch_start_time = $8,
                epoch_end_time = $9,
                updated_at = $10",
        )
        .bind(epoch as i64)
        .bind(pad_u256(summary.total_staked))
        .bind(summary.num_stakers as i64)
        .bind(summary.num_withdrawing as i64)
        .bind(pad_u256(summary.total_staking_emissions))
        .bind(pad_u256(summary.total_staking_power))
        .bind(summary.num_reward_recipients as i64)
        .bind(summary.epoch_start_time as i64)
        .bind(summary.epoch_end_time as i64)
        .bind(summary.updated_at.unwrap_or_else(|| Utc::now().to_rfc3339()))
        .execute(&self.pool)
        .await
        .map_err(DbError::from)?;
        Ok(())
    }

    async fn get_epoch_staking_summary(
        &self,
        epoch: u64,
    ) -> Result<Option<EpochStakingSummary>, DbError> {
        let row = sqlx::query(
            "SELECT epoch, total_staked, num_stakers, num_withdrawing,
                    total_staking_emissions, total_staking_power,
                    num_reward_recipients, epoch_start_time, epoch_end_time, updated_at
             FROM epoch_staking_summary
             WHERE epoch = $1",
        )
        .bind(epoch as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(DbError::from)?;

        if let Some(row) = row {
            Ok(Some(EpochStakingSummary {
                epoch: row.get::<i64, _>("epoch") as u64,
                total_staked: unpad_u256(&row.get::<String, _>("total_staked"))?,
                num_stakers: row.get::<i64, _>("num_stakers") as u64,
                num_withdrawing: row.get::<i64, _>("num_withdrawing") as u64,
                total_staking_emissions: unpad_u256(
                    &row.get::<String, _>("total_staking_emissions"),
                )?,
                total_staking_power: unpad_u256(&row.get::<String, _>("total_staking_power"))?,
                num_reward_recipients: row.get::<i64, _>("num_reward_recipients") as u64,
                epoch_start_time: row.get::<i64, _>("epoch_start_time") as u64,
                epoch_end_time: row.get::<i64, _>("epoch_end_time") as u64,
                updated_at: row.get::<Option<String>, _>("updated_at"),
            }))
        } else {
            Ok(None)
        }
    }

    async fn upsert_staking_rewards_by_epoch(
        &self,
        epoch: u64,
        rewards: Vec<StakingRewardByEpoch>,
    ) -> Result<(), DbError> {
        if rewards.is_empty() {
            return Ok(());
        }

        // Process in chunks to avoid parameter limits
        for chunk in rewards.chunks(BATCH_INSERT_CHUNK_SIZE) {
            let mut values_clauses = Vec::new();
            let mut param_idx = 1;

            for _ in chunk {
                values_clauses.push(format!(
                    "(${},${},${},${},${})",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4
                ));
                param_idx += 5;
            }

            let query = format!(
                r#"INSERT INTO staking_rewards_by_epoch
                (staker_address, epoch, staking_power, percentage, rewards_earned)
                VALUES {}
                ON CONFLICT (staker_address, epoch) DO UPDATE SET
                    staking_power = EXCLUDED.staking_power,
                    percentage = EXCLUDED.percentage,
                    rewards_earned = EXCLUDED.rewards_earned,
                    updated_at = CURRENT_TIMESTAMP"#,
                values_clauses.join(",")
            );

            let mut q = sqlx::query(&query);
            for reward in chunk {
                q = q
                    .bind(format!("{:#x}", reward.staker_address))
                    .bind(epoch as i64)
                    .bind(pad_u256(reward.staking_power))
                    .bind(reward.percentage)
                    .bind(pad_u256(reward.rewards_earned));
            }
            q.execute(&self.pool).await.map_err(DbError::from)?;
        }
        Ok(())
    }

    async fn get_staking_rewards_by_epoch(
        &self,
        epoch: u64,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<StakingRewardByEpoch>, DbError> {
        let rows = sqlx::query(
            "SELECT staker_address, epoch, staking_power, percentage, rewards_earned
             FROM staking_rewards_by_epoch
             WHERE epoch = $1
             ORDER BY rewards_earned DESC
             LIMIT $2 OFFSET $3",
        )
        .bind(epoch as i64)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::from)?;

        let mut rewards = Vec::new();
        for row in rows {
            rewards.push(StakingRewardByEpoch {
                staker_address: Address::from_str(&row.get::<String, _>("staker_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                staking_power: unpad_u256(&row.get::<String, _>("staking_power"))?,
                percentage: row.get::<f64, _>("percentage"),
                rewards_earned: unpad_u256(&row.get::<String, _>("rewards_earned"))?,
            });
        }
        Ok(rewards)
    }

    async fn get_staking_rewards_by_address(
        &self,
        address: Address,
        start_epoch: Option<u64>,
        end_epoch: Option<u64>,
    ) -> Result<Vec<StakingRewardByEpoch>, DbError> {
        let mut query = String::from(
            "SELECT staker_address, epoch, staking_power, percentage, rewards_earned
             FROM staking_rewards_by_epoch
             WHERE staker_address = $1",
        );

        if let Some(start) = start_epoch {
            query.push_str(&format!(" AND epoch >= {}", start));
        }
        if let Some(end) = end_epoch {
            query.push_str(&format!(" AND epoch <= {}", end));
        }
        query.push_str(" ORDER BY epoch DESC");

        let rows = sqlx::query(&query)
            .bind(format!("{:#x}", address))
            .fetch_all(&self.pool)
            .await
            .map_err(DbError::from)?;

        let mut rewards = Vec::new();
        for row in rows {
            rewards.push(StakingRewardByEpoch {
                staker_address: Address::from_str(&row.get::<String, _>("staker_address"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                epoch: row.get::<i64, _>("epoch") as u64,
                staking_power: unpad_u256(&row.get::<String, _>("staking_power"))?,
                percentage: row.get::<f64, _>("percentage"),
                rewards_earned: unpad_u256(&row.get::<String, _>("rewards_earned"))?,
            });
        }
        Ok(rewards)
    }

    async fn get_all_epoch_povw_summaries(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<EpochPoVWSummary>, DbError> {
        let rows = sqlx::query(
            "SELECT epoch, total_work, total_emissions, total_capped_rewards,
                    total_uncapped_rewards, epoch_start_time, epoch_end_time, num_participants, updated_at
             FROM epoch_povw_summary
             ORDER BY epoch DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::from)?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(EpochPoVWSummary {
                epoch: row.get::<i64, _>("epoch") as u64,
                total_work: unpad_u256(&row.get::<String, _>("total_work"))?,
                total_emissions: unpad_u256(&row.get::<String, _>("total_emissions"))?,
                total_capped_rewards: unpad_u256(&row.get::<String, _>("total_capped_rewards"))?,
                total_uncapped_rewards: unpad_u256(
                    &row.get::<String, _>("total_uncapped_rewards"),
                )?,
                epoch_start_time: row.get::<i64, _>("epoch_start_time") as u64,
                epoch_end_time: row.get::<i64, _>("epoch_end_time") as u64,
                num_participants: row.get::<i64, _>("num_participants") as u64,
                updated_at: row.get::<Option<String>, _>("updated_at"),
            });
        }
        Ok(summaries)
    }

    async fn get_all_epoch_staking_summaries(
        &self,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<EpochStakingSummary>, DbError> {
        let rows = sqlx::query(
            "SELECT epoch, total_staked, num_stakers, num_withdrawing,
                    total_staking_emissions, total_staking_power, num_reward_recipients,
                    epoch_start_time, epoch_end_time, updated_at
             FROM epoch_staking_summary
             ORDER BY epoch DESC
             LIMIT $1 OFFSET $2",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(DbError::from)?;

        let mut summaries = Vec::new();
        for row in rows {
            summaries.push(EpochStakingSummary {
                epoch: row.get::<i64, _>("epoch") as u64,
                total_staked: unpad_u256(&row.get::<String, _>("total_staked"))?,
                num_stakers: row.get::<i64, _>("num_stakers") as u64,
                num_withdrawing: row.get::<i64, _>("num_withdrawing") as u64,
                total_staking_emissions: unpad_u256(
                    &row.get::<String, _>("total_staking_emissions"),
                )?,
                total_staking_power: unpad_u256(&row.get::<String, _>("total_staking_power"))?,
                num_reward_recipients: row.get::<i64, _>("num_reward_recipients") as u64,
                epoch_start_time: row.get::<i64, _>("epoch_start_time") as u64,
                epoch_end_time: row.get::<i64, _>("epoch_end_time") as u64,
                updated_at: row.get::<Option<String>, _>("updated_at"),
            });
        }
        Ok(summaries)
    }
}
