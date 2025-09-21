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

use std::{str::FromStr, sync::Arc};

use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use boundless_rewards::WorkLogRewardInfo;
use sqlx::{any::AnyPoolOptions, AnyPool, Row};

use super::DbError;

pub type RewardsDbObj = Arc<dyn RewardsIndexerDb + Send + Sync>;

/// Convert a U256 to a zero-padded string for proper database sorting
/// U256 max value has 78 decimal digits (2^256 ≈ 1.15 * 10^77)
fn pad_u256(value: U256) -> String {
    format!("{:0>78}", value)
}

/// Convert a zero-padded string back to U256
fn unpad_u256(s: &str) -> Result<U256, DbError> {
    U256::from_str(s.trim_start_matches('0'))
        .or_else(|_| {
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
        Self {
            work_log_id: info.work_log_id,
            epoch: 0, // Will be set by caller
            work_submitted: info.work_submitted,
            percentage: info.percentage,
            uncapped_rewards: info.uncapped_rewards,
            reward_cap: info.reward_cap,
            actual_rewards: info.actual_rewards,
            is_capped: info.is_capped,
            staked_amount: info.staked_amount,
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

    /// Get the current epoch from indexer state
    async fn get_current_epoch(&self) -> Result<Option<u64>, DbError>;

    /// Set the current epoch in indexer state
    async fn set_current_epoch(&self, epoch: u64) -> Result<(), DbError>;

    /// Get the last processed block for rewards indexer
    async fn get_last_rewards_block(&self) -> Result<Option<u64>, DbError>;

    /// Set the last processed block for rewards indexer
    async fn set_last_rewards_block(&self, block: u64) -> Result<(), DbError>;
}

pub struct RewardsDb {
    pool: AnyPool,
}

impl RewardsDb {
    pub async fn new(database_url: &str) -> Result<Self, DbError> {
        sqlx::any::install_default_drivers();
        let pool = AnyPoolOptions::new()
            .max_connections(5)
            .connect(database_url)
            .await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

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
        let mut tx = self.pool.begin().await?;

        for reward in rewards {
            let query = r#"
                INSERT INTO povw_rewards_by_epoch
                (work_log_id, epoch, work_submitted, percentage, uncapped_rewards, reward_cap, actual_rewards, is_capped, staked_amount, updated_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, CURRENT_TIMESTAMP)
                ON CONFLICT (work_log_id, epoch)
                DO UPDATE SET
                    work_submitted = $3,
                    percentage = $4,
                    uncapped_rewards = $5,
                    reward_cap = $6,
                    actual_rewards = $7,
                    is_capped = $8,
                    staked_amount = $9,
                    updated_at = CURRENT_TIMESTAMP
            "#;

            sqlx::query(query)
                .bind(format!("{:#x}", reward.work_log_id))
                .bind(epoch as i64)
                .bind(pad_u256(reward.work_submitted))
                .bind(reward.percentage)
                .bind(pad_u256(reward.uncapped_rewards))
                .bind(pad_u256(reward.reward_cap))
                .bind(pad_u256(reward.actual_rewards))
                .bind(if reward.is_capped { 1i32 } else { 0i32 })
                .bind(pad_u256(reward.staked_amount))
                .execute(&mut *tx)
                .await?;
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

        let rows = sqlx::query(query)
            .bind(format!("{:#x}", work_log_id))
            .fetch_all(&self.pool)
            .await?;

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
        let mut tx = self.pool.begin().await?;

        for agg in aggregates {
            let query = r#"
                INSERT INTO povw_rewards_aggregate
                (work_log_id, total_work_submitted, total_actual_rewards, total_uncapped_rewards, epochs_participated, updated_at)
                VALUES ($1, $2, $3, $4, $5, CURRENT_TIMESTAMP)
                ON CONFLICT (work_log_id)
                DO UPDATE SET
                    total_work_submitted = $2,
                    total_actual_rewards = $3,
                    total_uncapped_rewards = $4,
                    epochs_participated = $5,
                    updated_at = CURRENT_TIMESTAMP
            "#;

            sqlx::query(query)
                .bind(format!("{:#x}", agg.work_log_id))
                .bind(pad_u256(agg.total_work_submitted))
                .bind(pad_u256(agg.total_actual_rewards))
                .bind(pad_u256(agg.total_uncapped_rewards))
                .bind(agg.epochs_participated as i64)
                .execute(&mut *tx)
                .await?;
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

        let rows = sqlx::query(query)
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let mut results = Vec::new();
        for row in rows {
            results.push(PovwRewardAggregate {
                work_log_id: Address::from_str(&row.get::<String, _>("work_log_id"))
                    .map_err(|e| DbError::BadTransaction(e.to_string()))?,
                total_work_submitted: unpad_u256(&row.get::<String, _>("total_work_submitted"))?,
                total_actual_rewards: unpad_u256(&row.get::<String, _>("total_actual_rewards"))?,
                total_uncapped_rewards: unpad_u256(&row.get::<String, _>("total_uncapped_rewards"))?,
                epochs_participated: row.get::<i64, _>("epochs_participated") as u64,
            });
        }

        Ok(results)
    }

    async fn get_current_epoch(&self) -> Result<Option<u64>, DbError> {
        let query = "SELECT value FROM indexer_state WHERE key = 'current_epoch'";
        let result = sqlx::query(query).fetch_optional(&self.pool).await?;

        match result {
            Some(row) => {
                let value: String = row.get("value");
                Ok(Some(value.parse().map_err(|_| DbError::BadBlockNumb(value))?))
            },
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

        sqlx::query(query)
            .bind(epoch.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    async fn get_last_rewards_block(&self) -> Result<Option<u64>, DbError> {
        let query = "SELECT value FROM indexer_state WHERE key = 'last_rewards_block'";
        let result = sqlx::query(query).fetch_optional(&self.pool).await?;

        match result {
            Some(row) => {
                let value: String = row.get("value");
                Ok(Some(value.parse().map_err(|_| DbError::BadBlockNumb(value))?))
            },
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

        sqlx::query(query)
            .bind(block.to_string())
            .execute(&self.pool)
            .await?;

        Ok(())
    }
}