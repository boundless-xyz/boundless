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

use super::market::{
    padded_string_to_u256, u256_to_padded_string, AllTimeProverSummary, DailyProverSummary,
    HourlyProverSummary, IndexerDb, LockPricingData, MonthlyProverSummary, PeriodProverSummary,
    ProverLeaderboardEntry, RequestCursor, RequestSortField, RequestStatus, SortDirection,
    WeeklyProverSummary,
};
use super::DbError;
use alloy::primitives::{Address, U256};
use anyhow;
use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Row};
use std::str::FromStr;

/// Trait for prover-related database operations.
/// Requires IndexerDb for pool() and row_to_request_status() access.
#[async_trait]
pub trait ProversDb: IndexerDb {
    /// List requests where the prover either locked or fulfilled
    async fn list_requests_by_prover(
        &self,
        prover_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let prover_str = format!("{:x}", prover_address);
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE (lock_prover_address = $1 OR fulfill_prover_address = $1)
                   AND ({} < $2 OR ({} = $2 AND request_digest < $3))
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $4",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(self.pool())
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE (lock_prover_address = $1 OR fulfill_prover_address = $1)
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $2",
                sort_field
            );
            sqlx::query(&query_str)
                .bind(&prover_str)
                .bind(limit as i64)
                .fetch_all(self.pool())
                .await?
        };

        let mut results = Vec::new();
        for row in rows {
            results.push(self.row_to_request_status(&row)?);
        }

        let next_cursor = if results.len() == limit as usize {
            results.last().map(|r| {
                let timestamp = match sort_by {
                    RequestSortField::UpdatedAt => r.updated_at,
                    RequestSortField::CreatedAt => r.created_at,
                };
                RequestCursor { timestamp, request_digest: r.request_digest.to_string() }
            })
        } else {
            None
        };

        Ok((results, next_cursor))
    }

    // === Per-Prover Aggregate Methods ===

    async fn get_active_prover_addresses_in_period(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<Address>, DbError> {
        let query_str = "SELECT DISTINCT prover_address 
            FROM (
                -- Lock events
                SELECT DISTINCT rle.prover_address 
                FROM request_locked_events rle
                WHERE rle.block_timestamp >= $1 AND rle.block_timestamp < $2
                
                UNION
                
                -- Fulfillment events
                SELECT DISTINCT rfe.prover_address
                FROM request_fulfilled_events rfe
                WHERE rfe.block_timestamp >= $1 AND rfe.block_timestamp < $2
            ) AS active_provers
            ORDER BY prover_address";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_all(self.pool())
            .await?;

        let addresses = rows
            .iter()
            .map(|row| {
                let addr_str: String = row.try_get("prover_address")?;
                Address::from_str(&addr_str)
                    .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(addresses)
    }

    async fn get_period_prover_requests_locked_count(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_locked_events rle
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND rle.prover_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", prover_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_prover_requests_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_fulfilled_events rfe
            WHERE rfe.block_timestamp >= $1 
            AND rfe.block_timestamp < $2
            AND rfe.prover_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", prover_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_prover_unique_requestors(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT client_address) as count 
            FROM (
                SELECT DISTINCT rs.client_address
                FROM request_locked_events rle
                JOIN request_status rs ON rle.request_digest = rs.request_digest
                WHERE rle.block_timestamp >= $1 
                AND rle.block_timestamp < $2
                AND rle.prover_address = $3
                
                UNION
                
                SELECT DISTINCT rs.client_address
                FROM request_fulfilled_events rfe
                JOIN request_status rs ON rfe.request_digest = rs.request_digest
                WHERE rfe.block_timestamp >= $1 
                AND rfe.block_timestamp < $2
                AND rfe.prover_address = $3
            ) AS unique_requestors";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", prover_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_prover_total_fees_earned(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT lock_price FROM request_status
             WHERE lock_prover_address = $1
             AND locked_at IS NOT NULL
             AND locked_at >= $2 AND locked_at < $3
             AND lock_price IS NOT NULL",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let lock_price_str: String = row.try_get("lock_price")?;
            let lock_price = padded_string_to_u256(&lock_price_str)?;
            total = total.checked_add(lock_price).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing lock_price"))
            })?;
        }
        Ok(total)
    }

    async fn get_period_prover_total_collateral_locked(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT lock_collateral FROM request_status
             WHERE lock_prover_address = $1
             AND locked_at IS NOT NULL
             AND locked_at >= $2 AND locked_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let lock_collateral_str: String = row.try_get("lock_collateral")?;
            let lock_collateral = padded_string_to_u256(&lock_collateral_str)?;
            total = total.checked_add(lock_collateral).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing lock_collateral"))
            })?;
        }
        Ok(total)
    }

    async fn get_period_prover_total_collateral_slashed(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT slash_transferred_amount, slash_burned_amount FROM request_status
             WHERE lock_prover_address = $1
             AND slashed_at IS NOT NULL
             AND slashed_at >= $2 AND slashed_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let transferred_str: Option<String> = row.try_get("slash_transferred_amount").ok();
            let burned_str: Option<String> = row.try_get("slash_burned_amount").ok();

            if let Some(transferred) = transferred_str {
                let transferred_amount = padded_string_to_u256(&transferred)?;
                total = total.checked_add(transferred_amount).ok_or_else(|| {
                    DbError::Error(anyhow::anyhow!(
                        "Overflow when summing slash_transferred_amount"
                    ))
                })?;
            }

            if let Some(burned) = burned_str {
                let burned_amount = padded_string_to_u256(&burned)?;
                total = total.checked_add(burned_amount).ok_or_else(|| {
                    DbError::Error(anyhow::anyhow!("Overflow when summing slash_burned_amount"))
                })?;
            }
        }
        Ok(total)
    }

    async fn get_period_prover_total_collateral_earned(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT lock_collateral FROM request_status
             WHERE fulfill_prover_address = $1
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at > lock_end
             AND fulfilled_at < expires_at
             AND fulfilled_at >= $2 AND fulfilled_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let lock_collateral_str: String = row.try_get("lock_collateral")?;
            let lock_collateral = padded_string_to_u256(&lock_collateral_str)?;
            total = total.checked_add(lock_collateral).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing lock_collateral"))
            })?;
        }
        Ok(total)
    }

    async fn get_period_prover_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status
            WHERE lock_prover_address = $1
            AND request_status = 'expired'
            AND expires_at >= $2 AND expires_at < $3";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", prover_address))
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_prover_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status
            WHERE lock_prover_address = $1
            AND fulfill_prover_address = $1
            AND request_status = 'fulfilled'
            AND fulfilled_at IS NOT NULL
            AND fulfilled_at >= $2 AND fulfilled_at < $3";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", prover_address))
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_prover_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<Vec<LockPricingData>, DbError> {
        let query_str = "SELECT 
                rs.request_digest,
                rs.min_price,
                rs.max_price,
                rs.ramp_up_start,
                rs.ramp_up_period,
                rs.lock_end,
                rs.locked_at as lock_timestamp,
                rs.lock_price,
                rs.lock_price_per_cycle,
                rs.lock_collateral
            FROM request_status rs
            WHERE rs.lock_prover_address = $1
            AND rs.locked_at IS NOT NULL
            AND rs.locked_at >= $2
            AND rs.locked_at < $3";

        let rows = sqlx::query(query_str)
            .bind(format!("{:x}", prover_address))
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_all(self.pool())
            .await?;

        let mut result = Vec::new();
        for row in rows {
            let min_price: String = row.try_get("min_price")?;
            let max_price: String = row.try_get("max_price")?;
            let ramp_up_start: i64 = row.try_get("ramp_up_start")?;
            let ramp_up_period: i64 = row.try_get("ramp_up_period")?;
            let lock_end: i64 = row.try_get("lock_end")?;
            let lock_collateral: String = row.try_get("lock_collateral")?;
            let lock_timestamp: i64 = row.try_get("lock_timestamp")?;
            let lock_price: Option<String> = row.try_get("lock_price").ok();
            let lock_price_per_cycle: Option<String> = row.try_get("lock_price_per_cycle").ok();

            result.push(LockPricingData {
                min_price,
                max_price,
                ramp_up_start: ramp_up_start as u64,
                ramp_up_period: ramp_up_period as u32,
                lock_end: lock_end as u64,
                lock_collateral,
                lock_timestamp: lock_timestamp as u64,
                lock_price,
                lock_price_per_cycle,
            });
        }

        Ok(result)
    }

    async fn get_period_prover_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT program_cycles FROM request_status
             WHERE fulfill_prover_address = $1
             AND request_status = 'fulfilled'
             AND program_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $2 AND fulfilled_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let program_cycles_str: String = row.try_get("program_cycles")?;
            let program_cycles = padded_string_to_u256(&program_cycles_str)?;
            total = total.checked_add(program_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing program_cycles"))
            })?;
        }
        Ok(total)
    }

    async fn get_period_prover_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        prover_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT total_cycles FROM request_status
             WHERE fulfill_prover_address = $1
             AND request_status = 'fulfilled'
             AND total_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $2 AND fulfilled_at < $3",
        )
        .bind(format!("{:x}", prover_address))
        .bind(period_start as i64)
        .bind(period_end as i64)
        .fetch_all(self.pool())
        .await?;

        let mut total = U256::ZERO;
        for row in rows {
            let total_cycles_str: String = row.try_get("total_cycles")?;
            let total_cycles = padded_string_to_u256(&total_cycles_str)?;
            total = total.checked_add(total_cycles).ok_or_else(|| {
                DbError::Error(anyhow::anyhow!("Overflow when summing total_cycles"))
            })?;
        }
        Ok(total)
    }

    async fn get_all_time_prover_unique_requestors(
        &self,
        end_ts: u64,
        prover_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT client_address) as count 
            FROM (
                SELECT DISTINCT rs.client_address
                FROM request_locked_events rle
                JOIN request_status rs ON rle.request_digest = rs.request_digest
                WHERE rle.block_timestamp < $1
                AND rle.prover_address = $2
                
                UNION
                
                SELECT DISTINCT rs.client_address
                FROM request_fulfilled_events rfe
                JOIN request_status rs ON rfe.request_digest = rs.request_digest
                WHERE rfe.block_timestamp < $1
                AND rfe.prover_address = $2
            ) AS unique_requestors";

        let row = sqlx::query(query_str)
            .bind(end_ts as i64)
            .bind(format!("{:x}", prover_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn upsert_hourly_prover_summary(
        &self,
        summary: HourlyProverSummary,
    ) -> Result<(), DbError> {
        upsert_prover_summary_generic(self.pool(), summary, "hourly_prover_summary").await
    }

    async fn upsert_daily_prover_summary(
        &self,
        summary: DailyProverSummary,
    ) -> Result<(), DbError> {
        upsert_prover_summary_generic(self.pool(), summary, "daily_prover_summary").await
    }

    async fn upsert_weekly_prover_summary(
        &self,
        summary: WeeklyProverSummary,
    ) -> Result<(), DbError> {
        upsert_prover_summary_generic(self.pool(), summary, "weekly_prover_summary").await
    }

    async fn upsert_monthly_prover_summary(
        &self,
        summary: MonthlyProverSummary,
    ) -> Result<(), DbError> {
        upsert_prover_summary_generic(self.pool(), summary, "monthly_prover_summary").await
    }

    async fn upsert_all_time_prover_summary(
        &self,
        summary: AllTimeProverSummary,
    ) -> Result<(), DbError> {
        let query_str = "INSERT INTO all_time_prover_summary (
                period_timestamp,
                prover_address,
                total_requests_locked,
                total_requests_fulfilled,
                total_unique_requestors,
                total_fees_earned,
                total_collateral_locked,
                total_collateral_slashed,
                total_collateral_earned,
                total_requests_locked_and_expired,
                total_requests_locked_and_fulfilled,
                locked_orders_fulfillment_rate,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_request_id,
                best_peak_prove_mhz,
                best_effective_prove_mhz,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, CAST($17 AS DOUBLE PRECISION), CAST($18 AS DOUBLE PRECISION), CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp, prover_address) DO UPDATE SET
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_fulfilled = EXCLUDED.total_requests_fulfilled,
                total_unique_requestors = EXCLUDED.total_unique_requestors,
                total_fees_earned = EXCLUDED.total_fees_earned,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_collateral_slashed = EXCLUDED.total_collateral_slashed,
                total_collateral_earned = EXCLUDED.total_collateral_earned,
                total_requests_locked_and_expired = EXCLUDED.total_requests_locked_and_expired,
                total_requests_locked_and_fulfilled = EXCLUDED.total_requests_locked_and_fulfilled,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
                best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
                updated_at = CURRENT_TIMESTAMP";

        sqlx::query(query_str)
            .bind(summary.period_timestamp as i64)
            .bind(format!("{:x}", summary.prover_address))
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_fulfilled as i64)
            .bind(summary.total_unique_requestors as i64)
            .bind(u256_to_padded_string(summary.total_fees_earned))
            .bind(u256_to_padded_string(summary.total_collateral_locked))
            .bind(u256_to_padded_string(summary.total_collateral_slashed))
            .bind(u256_to_padded_string(summary.total_collateral_earned))
            .bind(summary.total_requests_locked_and_expired as i64)
            .bind(summary.total_requests_locked_and_fulfilled as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .bind(u256_to_padded_string(summary.total_program_cycles))
            .bind(u256_to_padded_string(summary.total_cycles))
            .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_peak_prove_mhz.to_string())
            .bind(summary.best_effective_prove_mhz.to_string())
            .execute(self.pool())
            .await?;

        Ok(())
    }

    async fn get_hourly_prover_summaries(
        &self,
        prover_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<PeriodProverSummary>, DbError> {
        get_prover_summaries_generic(
            self.pool(),
            prover_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "hourly_prover_summary",
        )
        .await
    }

    async fn get_daily_prover_summaries(
        &self,
        prover_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<DailyProverSummary>, DbError> {
        get_prover_summaries_generic(
            self.pool(),
            prover_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "daily_prover_summary",
        )
        .await
    }

    async fn get_weekly_prover_summaries(
        &self,
        prover_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<WeeklyProverSummary>, DbError> {
        get_prover_summaries_generic(
            self.pool(),
            prover_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "weekly_prover_summary",
        )
        .await
    }

    async fn get_monthly_prover_summaries(
        &self,
        prover_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<MonthlyProverSummary>, DbError> {
        get_prover_summaries_generic(
            self.pool(),
            prover_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "monthly_prover_summary",
        )
        .await
    }

    async fn get_all_time_prover_summaries(
        &self,
        prover_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<AllTimeProverSummary>, DbError> {
        get_all_time_prover_summaries_generic(
            self.pool(),
            prover_address,
            cursor,
            limit,
            sort,
            before,
            after,
        )
        .await
    }

    async fn get_latest_all_time_prover_summary(
        &self,
        prover_address: Address,
    ) -> Result<Option<AllTimeProverSummary>, DbError> {
        let query_str = "SELECT 
            period_timestamp, prover_address, total_requests_locked, total_requests_fulfilled,
            total_unique_requestors, total_fees_earned, total_collateral_locked, total_collateral_slashed,
            total_collateral_earned, total_requests_locked_and_expired, total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate, total_program_cycles, total_cycles,
            best_peak_prove_mhz_request_id, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz, best_effective_prove_mhz
        FROM all_time_prover_summary 
        WHERE prover_address = $1 
        ORDER BY period_timestamp DESC 
        LIMIT 1";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", prover_address))
            .fetch_optional(self.pool())
            .await?;

        if let Some(row) = row {
            Ok(Some(parse_all_time_prover_summary_row(&row)?))
        } else {
            Ok(None)
        }
    }

    async fn get_hourly_prover_summaries_by_range(
        &self,
        prover_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodProverSummary>, DbError> {
        get_prover_summaries_by_range_generic(
            self.pool(),
            prover_address,
            start_ts,
            end_ts,
            "hourly_prover_summary",
        )
        .await
    }

    async fn get_prover_leaderboard(
        &self,
        start_ts: u64,
        end_ts: u64,
        use_hourly_table: bool,
        cursor_fees: Option<U256>,
        cursor_address: Option<Address>,
        limit: i64,
    ) -> Result<Vec<ProverLeaderboardEntry>, DbError> {
        get_prover_leaderboard_impl(
            self.pool(),
            start_ts,
            end_ts,
            use_hourly_table,
            cursor_fees,
            cursor_address,
            limit,
        )
        .await
    }

    async fn get_prover_median_lock_prices(
        &self,
        prover_addresses: &[Address],
        start_ts: u64,
        end_ts: u64,
    ) -> Result<std::collections::HashMap<Address, U256>, DbError> {
        get_prover_median_lock_prices_impl(self.pool(), prover_addresses, start_ts, end_ts).await
    }

    async fn get_prover_last_activity_times(
        &self,
        prover_addresses: &[Address],
    ) -> Result<std::collections::HashMap<Address, u64>, DbError> {
        get_prover_last_activity_times_impl(self.pool(), prover_addresses).await
    }
}

// Blanket implementation for anything that implements IndexerDb
impl<T: IndexerDb + Send + Sync> ProversDb for T {}

// === Standalone helper functions for generic operations ===

fn parse_period_prover_summary_row(row: &PgRow) -> Result<PeriodProverSummary, DbError> {
    let period_timestamp: i64 = row.try_get("period_timestamp")?;
    let prover_address_str: String = row.try_get("prover_address")?;
    let prover_address = Address::from_str(&prover_address_str)
        .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid prover address: {}", e)))?;
    let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
    let total_requests_fulfilled: i64 = row.try_get("total_requests_fulfilled")?;
    let total_unique_requestors: i64 = row.try_get("total_unique_requestors")?;
    let total_fees_earned_str: String = row.try_get("total_fees_earned")?;
    let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
    let total_collateral_slashed_str: String = row.try_get("total_collateral_slashed")?;
    let total_collateral_earned_str: String = row.try_get("total_collateral_earned")?;
    let p10_str: String = row.try_get("p10_lock_price_per_cycle")?;
    let p25_str: String = row.try_get("p25_lock_price_per_cycle")?;
    let p50_str: String = row.try_get("p50_lock_price_per_cycle")?;
    let p75_str: String = row.try_get("p75_lock_price_per_cycle")?;
    let p90_str: String = row.try_get("p90_lock_price_per_cycle")?;
    let p95_str: String = row.try_get("p95_lock_price_per_cycle")?;
    let p99_str: String = row.try_get("p99_lock_price_per_cycle")?;
    let total_requests_locked_and_expired: i64 =
        row.try_get("total_requests_locked_and_expired")?;
    let total_requests_locked_and_fulfilled: i64 =
        row.try_get("total_requests_locked_and_fulfilled")?;
    let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
    let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
    let total_cycles_str: String = row.try_get("total_cycles")?;
    let best_peak_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_peak_prove_mhz").ok().flatten().unwrap_or(0.0);
    let best_peak_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_peak_prove_mhz_request_id").ok();
    let best_effective_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_effective_prove_mhz").ok().flatten().unwrap_or(0.0);
    let best_effective_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_effective_prove_mhz_request_id").ok();

    Ok(PeriodProverSummary {
        period_timestamp: period_timestamp as u64,
        prover_address,
        total_requests_locked: total_requests_locked as u64,
        total_requests_fulfilled: total_requests_fulfilled as u64,
        total_unique_requestors: total_unique_requestors as u64,
        total_fees_earned: padded_string_to_u256(&total_fees_earned_str)?,
        total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
        total_collateral_slashed: padded_string_to_u256(&total_collateral_slashed_str)?,
        total_collateral_earned: padded_string_to_u256(&total_collateral_earned_str)?,
        total_requests_locked_and_expired: total_requests_locked_and_expired as u64,
        total_requests_locked_and_fulfilled: total_requests_locked_and_fulfilled as u64,
        locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
        p10_lock_price_per_cycle: padded_string_to_u256(&p10_str)?,
        p25_lock_price_per_cycle: padded_string_to_u256(&p25_str)?,
        p50_lock_price_per_cycle: padded_string_to_u256(&p50_str)?,
        p75_lock_price_per_cycle: padded_string_to_u256(&p75_str)?,
        p90_lock_price_per_cycle: padded_string_to_u256(&p90_str)?,
        p95_lock_price_per_cycle: padded_string_to_u256(&p95_str)?,
        p99_lock_price_per_cycle: padded_string_to_u256(&p99_str)?,
        total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
        total_cycles: padded_string_to_u256(&total_cycles_str)?,
        best_peak_prove_mhz,
        best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
        best_effective_prove_mhz,
        best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
    })
}

fn parse_all_time_prover_summary_row(row: &PgRow) -> Result<AllTimeProverSummary, DbError> {
    let period_timestamp: i64 = row.try_get("period_timestamp")?;
    let prover_address_str: String = row.try_get("prover_address")?;
    let prover_address = Address::from_str(&prover_address_str)
        .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid prover address: {}", e)))?;
    let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
    let total_requests_fulfilled: i64 = row.try_get("total_requests_fulfilled")?;
    let total_unique_requestors: i64 = row.try_get("total_unique_requestors")?;
    let total_fees_earned_str: String = row.try_get("total_fees_earned")?;
    let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
    let total_collateral_slashed_str: String = row.try_get("total_collateral_slashed")?;
    let total_collateral_earned_str: String = row.try_get("total_collateral_earned")?;
    let total_requests_locked_and_expired: i64 =
        row.try_get("total_requests_locked_and_expired")?;
    let total_requests_locked_and_fulfilled: i64 =
        row.try_get("total_requests_locked_and_fulfilled")?;
    let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
    let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
    let total_cycles_str: String = row.try_get("total_cycles")?;
    let best_peak_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_peak_prove_mhz").ok().flatten().unwrap_or(0.0);
    let best_peak_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_peak_prove_mhz_request_id").ok();
    let best_effective_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_effective_prove_mhz").ok().flatten().unwrap_or(0.0);
    let best_effective_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_effective_prove_mhz_request_id").ok();

    Ok(AllTimeProverSummary {
        period_timestamp: period_timestamp as u64,
        prover_address,
        total_requests_locked: total_requests_locked as u64,
        total_requests_fulfilled: total_requests_fulfilled as u64,
        total_unique_requestors: total_unique_requestors as u64,
        total_fees_earned: padded_string_to_u256(&total_fees_earned_str)?,
        total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
        total_collateral_slashed: padded_string_to_u256(&total_collateral_slashed_str)?,
        total_collateral_earned: padded_string_to_u256(&total_collateral_earned_str)?,
        total_requests_locked_and_expired: total_requests_locked_and_expired as u64,
        total_requests_locked_and_fulfilled: total_requests_locked_and_fulfilled as u64,
        locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
        total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
        total_cycles: padded_string_to_u256(&total_cycles_str)?,
        best_peak_prove_mhz,
        best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
        best_effective_prove_mhz,
        best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
    })
}

async fn upsert_prover_summary_generic(
    pool: &PgPool,
    summary: PeriodProverSummary,
    table_name: &str,
) -> Result<(), DbError> {
    let query_str = format!(
        "INSERT INTO {} (
            period_timestamp,
            prover_address,
            total_requests_locked,
            total_requests_fulfilled,
            total_unique_requestors,
            total_fees_earned,
            total_collateral_locked,
            total_collateral_slashed,
            total_collateral_earned,
            total_requests_locked_and_expired,
            total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate,
            p10_lock_price_per_cycle,
            p25_lock_price_per_cycle,
            p50_lock_price_per_cycle,
            p75_lock_price_per_cycle,
            p90_lock_price_per_cycle,
            p95_lock_price_per_cycle,
            p99_lock_price_per_cycle,
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_request_id,
            best_peak_prove_mhz,
            best_effective_prove_mhz,
            updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, CAST($24 AS DOUBLE PRECISION), CAST($25 AS DOUBLE PRECISION), CURRENT_TIMESTAMP)
        ON CONFLICT (period_timestamp, prover_address) DO UPDATE SET
            total_requests_locked = EXCLUDED.total_requests_locked,
            total_requests_fulfilled = EXCLUDED.total_requests_fulfilled,
            total_unique_requestors = EXCLUDED.total_unique_requestors,
            total_fees_earned = EXCLUDED.total_fees_earned,
            total_collateral_locked = EXCLUDED.total_collateral_locked,
            total_collateral_slashed = EXCLUDED.total_collateral_slashed,
            total_collateral_earned = EXCLUDED.total_collateral_earned,
            total_requests_locked_and_expired = EXCLUDED.total_requests_locked_and_expired,
            total_requests_locked_and_fulfilled = EXCLUDED.total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
            p10_lock_price_per_cycle = EXCLUDED.p10_lock_price_per_cycle,
            p25_lock_price_per_cycle = EXCLUDED.p25_lock_price_per_cycle,
            p50_lock_price_per_cycle = EXCLUDED.p50_lock_price_per_cycle,
            p75_lock_price_per_cycle = EXCLUDED.p75_lock_price_per_cycle,
            p90_lock_price_per_cycle = EXCLUDED.p90_lock_price_per_cycle,
            p95_lock_price_per_cycle = EXCLUDED.p95_lock_price_per_cycle,
            p99_lock_price_per_cycle = EXCLUDED.p99_lock_price_per_cycle,
            total_program_cycles = EXCLUDED.total_program_cycles,
            total_cycles = EXCLUDED.total_cycles,
            best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
            best_peak_prove_mhz = EXCLUDED.best_peak_prove_mhz,
            best_effective_prove_mhz = EXCLUDED.best_effective_prove_mhz,
            updated_at = CURRENT_TIMESTAMP",
        table_name
    );

    sqlx::query(&query_str)
        .bind(summary.period_timestamp as i64)
        .bind(format!("{:x}", summary.prover_address))
        .bind(summary.total_requests_locked as i64)
        .bind(summary.total_requests_fulfilled as i64)
        .bind(summary.total_unique_requestors as i64)
        .bind(u256_to_padded_string(summary.total_fees_earned))
        .bind(u256_to_padded_string(summary.total_collateral_locked))
        .bind(u256_to_padded_string(summary.total_collateral_slashed))
        .bind(u256_to_padded_string(summary.total_collateral_earned))
        .bind(summary.total_requests_locked_and_expired as i64)
        .bind(summary.total_requests_locked_and_fulfilled as i64)
        .bind(summary.locked_orders_fulfillment_rate)
        .bind(u256_to_padded_string(summary.p10_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p25_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p50_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p75_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p90_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p95_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p99_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.total_program_cycles))
        .bind(u256_to_padded_string(summary.total_cycles))
        .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .bind(summary.best_peak_prove_mhz.to_string())
        .bind(summary.best_effective_prove_mhz.to_string())
        .execute(pool)
        .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn get_prover_summaries_generic(
    pool: &PgPool,
    prover_address: Address,
    cursor: Option<i64>,
    limit: i64,
    sort: SortDirection,
    before: Option<i64>,
    after: Option<i64>,
    table_name: &str,
) -> Result<Vec<PeriodProverSummary>, DbError> {
    let mut conditions = Vec::new();
    let mut bind_count = 0;

    bind_count += 1;
    conditions.push(format!("prover_address = ${}", bind_count));

    let cursor_condition = match (cursor, sort) {
        (Some(_), SortDirection::Asc) => {
            bind_count += 1;
            Some(format!("period_timestamp > ${}", bind_count))
        }
        (Some(_), SortDirection::Desc) => {
            bind_count += 1;
            Some(format!("period_timestamp < ${}", bind_count))
        }
        (None, _) => None,
    };

    if let Some(cond) = cursor_condition {
        conditions.push(cond);
    }

    if after.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp > ${}", bind_count));
    }

    if before.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp < ${}", bind_count));
    }

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let order_clause = match sort {
        SortDirection::Asc => "ORDER BY period_timestamp ASC",
        SortDirection::Desc => "ORDER BY period_timestamp DESC",
    };

    bind_count += 1;
    let query_str = format!(
        "SELECT 
            period_timestamp, prover_address, total_requests_locked, total_requests_fulfilled,
            total_unique_requestors, total_fees_earned, total_collateral_locked, total_collateral_slashed,
            total_collateral_earned, total_requests_locked_and_expired, total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate, p10_lock_price_per_cycle, p25_lock_price_per_cycle,
            p50_lock_price_per_cycle, p75_lock_price_per_cycle, p90_lock_price_per_cycle,
            p95_lock_price_per_cycle, p99_lock_price_per_cycle, total_program_cycles, total_cycles,
            best_peak_prove_mhz_request_id, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz, best_effective_prove_mhz
        FROM {}
        {}
        {}
        LIMIT ${}",
        table_name, where_clause, order_clause, bind_count
    );

    let mut query = sqlx::query(&query_str);

    query = query.bind(format!("{:x}", prover_address));

    if let Some(cursor_ts) = cursor {
        query = query.bind(cursor_ts);
    }

    if let Some(after_ts) = after {
        query = query.bind(after_ts);
    }

    if let Some(before_ts) = before {
        query = query.bind(before_ts);
    }

    query = query.bind(limit);

    let rows = query.fetch_all(pool).await?;

    let summaries = rows
        .into_iter()
        .map(|row| parse_period_prover_summary_row(&row))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(summaries)
}

async fn get_all_time_prover_summaries_generic(
    pool: &PgPool,
    prover_address: Address,
    cursor: Option<i64>,
    limit: i64,
    sort: SortDirection,
    before: Option<i64>,
    after: Option<i64>,
) -> Result<Vec<AllTimeProverSummary>, DbError> {
    let mut conditions = Vec::new();
    let mut bind_count = 0;

    bind_count += 1;
    conditions.push(format!("prover_address = ${}", bind_count));

    let cursor_condition = match (cursor, sort) {
        (Some(_), SortDirection::Asc) => {
            bind_count += 1;
            Some(format!("period_timestamp > ${}", bind_count))
        }
        (Some(_), SortDirection::Desc) => {
            bind_count += 1;
            Some(format!("period_timestamp < ${}", bind_count))
        }
        (None, _) => None,
    };

    if let Some(cond) = cursor_condition {
        conditions.push(cond);
    }

    if after.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp > ${}", bind_count));
    }

    if before.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp < ${}", bind_count));
    }

    let where_clause = format!("WHERE {}", conditions.join(" AND "));

    let order_clause = match sort {
        SortDirection::Asc => "ORDER BY period_timestamp ASC",
        SortDirection::Desc => "ORDER BY period_timestamp DESC",
    };

    bind_count += 1;
    let query_str = format!(
        "SELECT 
            period_timestamp, prover_address, total_requests_locked, total_requests_fulfilled,
            total_unique_requestors, total_fees_earned, total_collateral_locked, total_collateral_slashed,
            total_collateral_earned, total_requests_locked_and_expired, total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate, total_program_cycles, total_cycles,
            best_peak_prove_mhz_request_id, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz, best_effective_prove_mhz
        FROM all_time_prover_summary
        {}
        {}
        LIMIT ${}",
        where_clause, order_clause, bind_count
    );

    let mut query = sqlx::query(&query_str);

    query = query.bind(format!("{:x}", prover_address));

    if let Some(cursor_ts) = cursor {
        query = query.bind(cursor_ts);
    }

    if let Some(after_ts) = after {
        query = query.bind(after_ts);
    }

    if let Some(before_ts) = before {
        query = query.bind(before_ts);
    }

    query = query.bind(limit);

    let rows = query.fetch_all(pool).await?;

    let summaries = rows
        .into_iter()
        .map(|row| parse_all_time_prover_summary_row(&row))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(summaries)
}

async fn get_prover_summaries_by_range_generic(
    pool: &PgPool,
    prover_address: Address,
    start_ts: u64,
    end_ts: u64,
    table_name: &str,
) -> Result<Vec<PeriodProverSummary>, DbError> {
    let query_str = format!(
        "SELECT 
            period_timestamp, prover_address, total_requests_locked, total_requests_fulfilled,
            total_unique_requestors, total_fees_earned, total_collateral_locked, total_collateral_slashed,
            total_collateral_earned, total_requests_locked_and_expired, total_requests_locked_and_fulfilled,
            locked_orders_fulfillment_rate, p10_lock_price_per_cycle, p25_lock_price_per_cycle,
            p50_lock_price_per_cycle, p75_lock_price_per_cycle, p90_lock_price_per_cycle,
            p95_lock_price_per_cycle, p99_lock_price_per_cycle, total_program_cycles, total_cycles,
            best_peak_prove_mhz_request_id, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz, best_effective_prove_mhz
        FROM {} WHERE prover_address = $1 AND period_timestamp >= $2 AND period_timestamp < $3 ORDER BY period_timestamp ASC",
        table_name
    );

    let rows = sqlx::query(&query_str)
        .bind(format!("{:x}", prover_address))
        .bind(start_ts as i64)
        .bind(end_ts as i64)
        .fetch_all(pool)
        .await?;

    let mut results = Vec::new();
    for row in rows {
        results.push(parse_period_prover_summary_row(&row)?);
    }

    Ok(results)
}

// Prover leaderboard query: aggregates stats for all provers in a time period
// Sorted by fees_earned DESC
async fn get_prover_leaderboard_impl(
    pool: &PgPool,
    start_ts: u64,
    end_ts: u64,
    use_hourly_table: bool,
    cursor_fees: Option<U256>,
    cursor_address: Option<Address>,
    limit: i64,
) -> Result<Vec<ProverLeaderboardEntry>, DbError> {
    let table_name =
        if use_hourly_table { "hourly_prover_summary" } else { "daily_prover_summary" };

    // Build the query with cursor-based pagination on (fees_earned, address)
    // Sorted by fees_earned DESC, address DESC
    let query_str = if cursor_fees.is_some() && cursor_address.is_some() {
        format!(
            "SELECT 
                prover_address,
                SUM(total_requests_locked)::BIGINT as orders_locked,
                SUM(total_requests_fulfilled)::BIGINT as orders_fulfilled,
                SUM(total_requests_locked_and_fulfilled)::BIGINT as fulfilled,
                SUM(total_requests_locked_and_expired)::BIGINT as expired,
                LPAD(SUM(CAST(total_cycles AS NUMERIC))::TEXT, 78, '0') as cycles,
                LPAD(SUM(CAST(total_fees_earned AS NUMERIC))::TEXT, 78, '0') as fees_earned,
                LPAD(SUM(CAST(total_collateral_earned AS NUMERIC))::TEXT, 78, '0') as collateral_earned,
                MAX(best_effective_prove_mhz) as best_effective_prove_mhz
            FROM {}
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            GROUP BY prover_address
            HAVING (
                SUM(CAST(total_fees_earned AS NUMERIC)) < CAST($3 AS NUMERIC)
                OR (SUM(CAST(total_fees_earned AS NUMERIC)) = CAST($3 AS NUMERIC) AND prover_address < $4)
            )
            ORDER BY fees_earned DESC, prover_address DESC
            LIMIT $5",
            table_name
        )
    } else {
        format!(
            "SELECT 
                prover_address,
                SUM(total_requests_locked)::BIGINT as orders_locked,
                SUM(total_requests_fulfilled)::BIGINT as orders_fulfilled,
                SUM(total_requests_locked_and_fulfilled)::BIGINT as fulfilled,
                SUM(total_requests_locked_and_expired)::BIGINT as expired,
                LPAD(SUM(CAST(total_cycles AS NUMERIC))::TEXT, 78, '0') as cycles,
                LPAD(SUM(CAST(total_fees_earned AS NUMERIC))::TEXT, 78, '0') as fees_earned,
                LPAD(SUM(CAST(total_collateral_earned AS NUMERIC))::TEXT, 78, '0') as collateral_earned,
                MAX(best_effective_prove_mhz) as best_effective_prove_mhz
            FROM {}
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            GROUP BY prover_address
            ORDER BY fees_earned DESC, prover_address DESC
            LIMIT $3",
            table_name
        )
    };

    let rows = if let (Some(cursor_f), Some(cursor_a)) = (cursor_fees, cursor_address) {
        let cursor_fees_str = u256_to_padded_string(cursor_f);
        sqlx::query(&query_str)
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .bind(&cursor_fees_str)
            .bind(format!("{:x}", cursor_a))
            .bind(limit)
            .fetch_all(pool)
            .await?
    } else {
        sqlx::query(&query_str)
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .bind(limit)
            .fetch_all(pool)
            .await?
    };

    let mut results = Vec::new();
    for row in rows {
        let prover_address_str: String = row.try_get("prover_address")?;
        let prover_address = Address::from_str(&prover_address_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))?;
        let orders_locked: i64 = row.try_get("orders_locked")?;
        let orders_fulfilled: i64 = row.try_get("orders_fulfilled")?;
        let fulfilled: i64 = row.try_get("fulfilled")?;
        let expired: i64 = row.try_get("expired")?;
        let cycles_str: String = row.try_get("cycles")?;
        let fees_str: String = row.try_get("fees_earned")?;
        let collateral_str: String = row.try_get("collateral_earned")?;
        let best_effective_prove_mhz: f64 = row.try_get("best_effective_prove_mhz")?;

        let cycles = padded_string_to_u256(&cycles_str)?;
        let fees_earned = padded_string_to_u256(&fees_str)?;
        let collateral_earned = padded_string_to_u256(&collateral_str)?;

        let total_outcomes = fulfilled + expired;
        let locked_order_fulfillment_rate = if total_outcomes > 0 {
            (fulfilled as f32 / total_outcomes as f32) * 100.0
        } else {
            0.0
        };

        results.push(ProverLeaderboardEntry {
            prover_address,
            orders_locked: orders_locked as u64,
            orders_fulfilled: orders_fulfilled as u64,
            cycles,
            fees_earned,
            collateral_earned,
            median_lock_price_per_cycle: None,
            best_effective_prove_mhz,
            locked_order_fulfillment_rate,
            last_activity_time: 0,
        });
    }

    Ok(results)
}

// Get median lock price per cycle for a list of provers in a time period
async fn get_prover_median_lock_prices_impl(
    pool: &PgPool,
    prover_addresses: &[Address],
    start_ts: u64,
    end_ts: u64,
) -> Result<std::collections::HashMap<Address, U256>, DbError> {
    if prover_addresses.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> =
        (3..=prover_addresses.len() + 2).map(|i| format!("${}", i)).collect();
    let placeholders_str = placeholders.join(", ");

    let query_str = format!(
        "SELECT 
            lock_prover_address,
            LPAD(
                ROUND(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY CAST(lock_price_per_cycle AS NUMERIC)))::TEXT,
                78, '0'
            ) as median_price
        FROM request_status
        WHERE locked_at >= $1 AND locked_at < $2
          AND lock_prover_address IN ({})
          AND lock_price_per_cycle IS NOT NULL
        GROUP BY lock_prover_address",
        placeholders_str
    );

    let mut query = sqlx::query(&query_str).bind(start_ts as i64).bind(end_ts as i64);

    for addr in prover_addresses {
        query = query.bind(format!("{:x}", addr));
    }

    let rows = query.fetch_all(pool).await?;

    let mut result = std::collections::HashMap::new();
    for row in rows {
        let addr_str: String = row.try_get("lock_prover_address")?;
        let median_str: String = row.try_get("median_price")?;
        let addr = Address::from_str(&addr_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))?;
        let median = padded_string_to_u256(&median_str)?;
        result.insert(addr, median);
    }

    Ok(result)
}

// Get last activity time for a list of provers
async fn get_prover_last_activity_times_impl(
    pool: &PgPool,
    prover_addresses: &[Address],
) -> Result<std::collections::HashMap<Address, u64>, DbError> {
    if prover_addresses.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> =
        (1..=prover_addresses.len()).map(|i| format!("${}", i)).collect();
    let placeholders_str = placeholders.join(", ");

    // Query for lock activity
    let lock_query = format!(
        "SELECT lock_prover_address as prover_address, MAX(updated_at) as last_activity
        FROM request_status
        WHERE lock_prover_address IN ({})
        GROUP BY lock_prover_address",
        placeholders_str
    );

    // Query for fulfill activity
    let fulfill_query = format!(
        "SELECT fulfill_prover_address as prover_address, MAX(updated_at) as last_activity
        FROM request_status
        WHERE fulfill_prover_address IN ({})
        GROUP BY fulfill_prover_address",
        placeholders_str
    );

    let mut lock_query_builder = sqlx::query(&lock_query);
    for addr in prover_addresses {
        lock_query_builder = lock_query_builder.bind(format!("{:x}", addr));
    }
    let lock_rows = lock_query_builder.fetch_all(pool).await?;

    let mut fulfill_query_builder = sqlx::query(&fulfill_query);
    for addr in prover_addresses {
        fulfill_query_builder = fulfill_query_builder.bind(format!("{:x}", addr));
    }
    let fulfill_rows = fulfill_query_builder.fetch_all(pool).await?;

    let mut result: std::collections::HashMap<Address, u64> = std::collections::HashMap::new();

    for row in lock_rows {
        let addr_str: String = row.try_get("prover_address")?;
        let last_activity: i64 = row.try_get("last_activity")?;
        let addr = Address::from_str(&addr_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))?;
        result
            .entry(addr)
            .and_modify(|e| *e = (*e).max(last_activity as u64))
            .or_insert(last_activity as u64);
    }

    for row in fulfill_rows {
        let addr_str: String = row.try_get("prover_address")?;
        let last_activity: i64 = row.try_get("last_activity")?;
        let addr = Address::from_str(&addr_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))?;
        result
            .entry(addr)
            .and_modify(|e| *e = (*e).max(last_activity as u64))
            .or_insert(last_activity as u64);
    }

    Ok(result)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::db::market::{RequestStatusType, SlashedStatus};
    use crate::test_utils::TestDb;
    use alloy::primitives::{B256, U256};

    async fn get_db_url_from_pool(pool: &sqlx::PgPool) -> String {
        let base_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
        let db_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .expect("failed to query current_database()");

        if let Some(last_slash) = base_url.rfind('/') {
            format!("{}/{}", &base_url[..last_slash], db_name)
        } else {
            format!("{}/{}", base_url, db_name)
        }
    }

    async fn test_db(pool: sqlx::PgPool) -> TestDb {
        let db_url = get_db_url_from_pool(&pool).await;
        TestDb::from_pool(db_url, pool).await.unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_list_requests_by_prover(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let prover1 = Address::from([0xAA; 20]);
        let prover2 = Address::from([0xBB; 20]);
        let prover3 = Address::from([0xCC; 20]);
        let client_addr = Address::from([0x11; 20]);
        let base_ts = 1700000000u64;

        // Create requests where prover1 only locked (not fulfilled)
        for i in 0..3 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Locked,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: client_addr,
                lock_prover_address: Some(prover1),
                fulfill_prover_address: None,
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100),
                locked_at: Some(base_ts + (i as u64 * 100)),
                fulfilled_at: None,
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: None,
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "1000".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 10000,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: None,
                total_cycles: None,
                peak_prove_mhz: None,
                effective_prove_mhz: None,
                prover_effective_prove_mhz: None,
                cycle_status: None,
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("100".to_string()),
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: Some(B256::from([0x01; 32])),
                fulfill_tx_hash: None,
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Create requests where prover1 both locked and fulfilled
        for i in 3..5 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Fulfilled,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: client_addr,
                lock_prover_address: Some(prover1),
                fulfill_prover_address: Some(prover1),
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100) + 50,
                locked_at: Some(base_ts + (i as u64 * 100)),
                fulfilled_at: Some(base_ts + (i as u64 * 100) + 50),
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: Some(102),
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "1000".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 10000,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: Some(U256::from(1000000)),
                total_cycles: Some(U256::from(1015800)),
                peak_prove_mhz: Some(1000.0),
                effective_prove_mhz: Some(950.0),
                prover_effective_prove_mhz: Some(950.0),
                cycle_status: Some("COMPLETED".to_string()),
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("100".to_string()),
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: Some(B256::from([0x01; 32])),
                fulfill_tx_hash: Some(B256::from([0x02; 32])),
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: None,
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: "0x00".to_string(),
                fulfill_journal: Some("0x1234".to_string()),
                fulfill_seal: Some("0x5678".to_string()),
            };
            db.upsert_request_statuses(&[status]).await.unwrap();
        }

        // Create request where prover2 locked but prover1 fulfilled
        let status_mixed = RequestStatus {
            request_digest: B256::from([0x10; 32]),
            request_id: U256::from(10),
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client_addr,
            lock_prover_address: Some(prover2),
            fulfill_prover_address: Some(prover1),
            created_at: base_ts + 1000,
            updated_at: base_ts + 1050,
            locked_at: Some(base_ts + 1000),
            fulfilled_at: Some(base_ts + 1050),
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: Some(101),
            fulfill_block: Some(102),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "1000".to_string(),
            ramp_up_start: base_ts,
            ramp_up_period: 10,
            expires_at: base_ts + 10000,
            lock_end: base_ts + 10000,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(2000000)),
            total_cycles: Some(U256::from(2031600)),
            peak_prove_mhz: Some(2000.0),
            effective_prove_mhz: Some(1900.0),
            prover_effective_prove_mhz: Some(1900.0),
            cycle_status: Some("COMPLETED".to_string()),
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some("100".to_string()),
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: Some(B256::from([0x03; 32])),
            fulfill_tx_hash: Some(B256::from([0x04; 32])),
            slash_tx_hash: None,
            image_id: "test".to_string(),
            image_url: None,
            selector: "test".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: Some("0xabcd".to_string()),
            fulfill_seal: Some("0xef01".to_string()),
        };
        db.upsert_request_statuses(&[status_mixed]).await.unwrap();

        // Create request where prover3 only fulfilled (didn't lock)
        let status_fulfill_only = RequestStatus {
            request_digest: B256::from([0x20; 32]),
            request_id: U256::from(20),
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client_addr,
            lock_prover_address: None,
            fulfill_prover_address: Some(prover3),
            created_at: base_ts + 2000,
            updated_at: base_ts + 2050,
            locked_at: None,
            fulfilled_at: Some(base_ts + 2050),
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: None,
            fulfill_block: Some(102),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "0".to_string(),
            ramp_up_start: base_ts,
            ramp_up_period: 10,
            expires_at: base_ts + 10000,
            lock_end: base_ts + 10000,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(3000000)),
            total_cycles: Some(U256::from(3047400)),
            peak_prove_mhz: Some(3000.0),
            effective_prove_mhz: Some(2800.0),
            prover_effective_prove_mhz: Some(2800.0),
            cycle_status: Some("COMPLETED".to_string()),
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
            fulfill_tx_hash: Some(B256::from([0x05; 32])),
            slash_tx_hash: None,
            image_id: "test".to_string(),
            image_url: None,
            selector: "test".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: Some("0xfedc".to_string()),
            fulfill_seal: Some("0xba98".to_string()),
        };
        db.upsert_request_statuses(&[status_fulfill_only]).await.unwrap();

        // List requests for prover1 - should get all requests where prover1 locked OR fulfilled
        // This includes: 3 locked-only, 2 locked+fulfilled, 1 fulfilled-only (mixed)
        // Total: 6 requests
        let (results, _cursor) = db
            .list_requests_by_prover(prover1, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(
            results.len(),
            6,
            "prover1 should have 6 requests (3 locked-only + 2 locked+fulfilled + 1 fulfilled-only)"
        );
        assert!(
            results.iter().all(|r| {
                r.lock_prover_address == Some(prover1) || r.fulfill_prover_address == Some(prover1)
            }),
            "All results should have prover1 as lock_prover or fulfill_prover"
        );

        // List with limit
        let (results, cursor) = db
            .list_requests_by_prover(prover1, None, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(cursor.is_some());

        // Use cursor for pagination
        let (results2, _) = db
            .list_requests_by_prover(prover1, cursor, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        // Results should be different from first page
        assert_ne!(results[0].request_id, results2[0].request_id);

        // Test sorting by updated_at
        let (results_updated, _) = db
            .list_requests_by_prover(prover1, None, 10, RequestSortField::UpdatedAt)
            .await
            .unwrap();
        assert_eq!(results_updated.len(), 6);
        // Verify descending order
        if results_updated.len() >= 2 {
            assert!(
                results_updated[0].updated_at >= results_updated[1].updated_at,
                "Should be sorted by updated_at descending"
            );
        }

        // List for prover2 - should only get the one request where prover2 locked
        let (results, _) = db
            .list_requests_by_prover(prover2, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].lock_prover_address, Some(prover2));
        assert_eq!(results[0].fulfill_prover_address, Some(prover1)); // prover1 fulfilled it

        // List for prover3 - should only get the one request where prover3 fulfilled
        let (results, _) = db
            .list_requests_by_prover(prover3, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].lock_prover_address, None);
        assert_eq!(results[0].fulfill_prover_address, Some(prover3));

        // List for a prover with no requests - should return empty
        let prover_no_requests = Address::from([0xFF; 20]);
        let (results, _) = db
            .list_requests_by_prover(prover_no_requests, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_prover_leaderboard(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let prover1 = Address::from([0x01; 20]);
        let prover2 = Address::from([0x02; 20]);
        let prover3 = Address::from([0x03; 20]);
        let period_ts = 1700000000u64;

        // Create hourly summaries for 3 provers with different fees earned
        let summary1 = HourlyProverSummary {
            period_timestamp: period_ts,
            prover_address: prover1,
            total_requests_locked: 50,
            total_requests_fulfilled: 45,
            total_unique_requestors: 10,
            total_fees_earned: U256::from(10000), // Most fees
            total_collateral_locked: U256::from(5000),
            total_collateral_slashed: U256::ZERO,
            total_collateral_earned: U256::from(100),
            total_requests_locked_and_expired: 2,
            total_requests_locked_and_fulfilled: 40,
            locked_orders_fulfillment_rate: 95.2,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_program_cycles: U256::from(1_000_000_000u64),
            total_cycles: U256::from(1_100_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_request_id: None,
        };

        let mut summary2 = summary1.clone();
        summary2.prover_address = prover2;
        summary2.total_fees_earned = U256::from(5000); // Second most

        let mut summary3 = summary1.clone();
        summary3.prover_address = prover3;
        summary3.total_fees_earned = U256::from(1000); // Least fees

        db.upsert_hourly_prover_summary(summary1).await.unwrap();
        db.upsert_hourly_prover_summary(summary2).await.unwrap();
        db.upsert_hourly_prover_summary(summary3).await.unwrap();

        // Test leaderboard query - should be sorted by fees_earned DESC
        let results = db
            .get_prover_leaderboard(
                period_ts,
                period_ts + 3600,
                true, // use hourly table
                None,
                None,
                10,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].prover_address, prover1);
        assert_eq!(results[0].fees_earned, U256::from(10000));
        assert_eq!(results[1].prover_address, prover2);
        assert_eq!(results[1].fees_earned, U256::from(5000));
        assert_eq!(results[2].prover_address, prover3);
        assert_eq!(results[2].fees_earned, U256::from(1000));

        // Test pagination with cursor
        let results_page1 = db
            .get_prover_leaderboard(period_ts, period_ts + 3600, true, None, None, 2)
            .await
            .unwrap();
        assert_eq!(results_page1.len(), 2);
        assert_eq!(results_page1[0].fees_earned, U256::from(10000));
        assert_eq!(results_page1[1].fees_earned, U256::from(5000));

        // Get page 2 using cursor from last item of page 1
        let last = &results_page1[1];
        let results_page2 = db
            .get_prover_leaderboard(
                period_ts,
                period_ts + 3600,
                true,
                Some(last.fees_earned),
                Some(last.prover_address),
                2,
            )
            .await
            .unwrap();
        assert_eq!(results_page2.len(), 1);
        assert_eq!(results_page2[0].fees_earned, U256::from(1000));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_prover_leaderboard_aggregation(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let prover = Address::from([0x01; 20]);
        let hour1 = 1700000000u64;
        let hour2 = hour1 + 3600;

        // Create summaries for two hours
        let summary1 = HourlyProverSummary {
            period_timestamp: hour1,
            prover_address: prover,
            total_requests_locked: 10,
            total_requests_fulfilled: 8,
            total_unique_requestors: 5,
            total_fees_earned: U256::from(1000),
            total_collateral_locked: U256::from(500),
            total_collateral_slashed: U256::ZERO,
            total_collateral_earned: U256::from(50),
            total_requests_locked_and_expired: 1,
            total_requests_locked_and_fulfilled: 7,
            locked_orders_fulfillment_rate: 87.5,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_program_cycles: U256::from(100_000_000u64),
            total_cycles: U256::from(110_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_request_id: None,
        };

        let mut summary2 = summary1.clone();
        summary2.period_timestamp = hour2;
        summary2.total_fees_earned = U256::from(2000);
        summary2.total_collateral_earned = U256::from(100);
        summary2.total_requests_locked = 20;
        summary2.total_requests_fulfilled = 18;
        summary2.total_cycles = U256::from(220_000_000u64);

        db.upsert_hourly_prover_summary(summary1).await.unwrap();
        db.upsert_hourly_prover_summary(summary2).await.unwrap();

        // Query across both hours - values should be aggregated
        let results = db
            .get_prover_leaderboard(
                hour1,
                hour2 + 3600, // Include both hours
                true,
                None,
                None,
                10,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        let entry = &results[0];
        assert_eq!(entry.prover_address, prover);
        assert_eq!(entry.fees_earned, U256::from(3000)); // 1000 + 2000
        assert_eq!(entry.collateral_earned, U256::from(150)); // 50 + 100
        assert_eq!(entry.orders_locked, 30); // 10 + 20
        assert_eq!(entry.orders_fulfilled, 26); // 8 + 18
        assert_eq!(entry.cycles, U256::from(330_000_000u64)); // 110M + 220M
    }

    fn create_locked_request_status_for_prover(
        digest: B256,
        client: Address,
        prover: Address,
        locked_at: u64,
        lock_price_per_cycle: U256,
    ) -> RequestStatus {
        RequestStatus {
            request_digest: digest,
            request_id: U256::from(12345),
            request_status: RequestStatusType::Locked,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client,
            lock_prover_address: Some(prover),
            fulfill_prover_address: None,
            created_at: locked_at - 100,
            updated_at: locked_at,
            locked_at: Some(locked_at),
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: Some(110),
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: 0,
            ramp_up_period: 10,
            expires_at: 9999999999,
            lock_end: 9999999999,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(1000000)),
            total_cycles: Some(U256::from(1100000)),
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: Some(u256_to_padded_string(lock_price_per_cycle)),
            lock_price_per_cycle: Some(u256_to_padded_string(lock_price_per_cycle)),
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: Some(B256::ZERO),
            fulfill_tx_hash: None,
            slash_tx_hash: None,
            image_id: "test_image".to_string(),
            image_url: Some("https://test.com".to_string()),
            selector: "test_selector".to_string(),
            predicate_type: "digest_match".to_string(),
            predicate_data: "0x00".to_string(),
            input_type: "inline".to_string(),
            input_data: "0x00".to_string(),
            fulfill_journal: None,
            fulfill_seal: None,
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_prover_median_lock_prices(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let client = Address::from([0x01; 20]);
        let prover1 = Address::from([0xAA; 20]);
        let prover2 = Address::from([0xBB; 20]);
        let base_ts = 1700000000u64;

        // Insert request_status entries with lock prices for prover1
        // Prices: 100, 200 -> median = 150.0 (interpolated, will round to 150)
        let mut statuses = Vec::new();
        for i in 0..2u8 {
            let digest = B256::from([i + 1; 32]);
            let lock_price = U256::from(100 * (i as u64 + 1));
            statuses.push(create_locked_request_status_for_prover(
                digest,
                client,
                prover1,
                base_ts + 100,
                lock_price,
            ));
        }

        // Add requests for prover2 with different prices
        // Prices: 100, 200, 300, 400 -> median = 250.0 (interpolated, will round to 250)
        for i in 0..4u8 {
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = 0x10 + i;
            let digest = B256::from(digest_bytes);
            let lock_price = U256::from(100 * (i as u64 + 1));
            statuses.push(create_locked_request_status_for_prover(
                digest,
                client,
                prover2,
                base_ts + 100,
                lock_price,
            ));
        }

        db.upsert_request_statuses(&statuses).await.unwrap();

        // Query median prices
        let medians = db
            .get_prover_median_lock_prices(&[prover1, prover2], base_ts, base_ts + 200)
            .await
            .unwrap();

        assert_eq!(medians.len(), 2);

        // Prover1 has prices [100, 200], median = 150.0 (interpolated, rounded to 150)
        assert_eq!(medians.get(&prover1), Some(&U256::from(150)));

        // Prover2 has prices [100, 200, 300, 400], median = 250.0 (interpolated, rounded to 250)
        assert_eq!(medians.get(&prover2), Some(&U256::from(250)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_prover_last_activity_times(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let client = Address::from([0x01; 20]);
        let prover1 = Address::from([0xAA; 20]);
        let prover2 = Address::from([0xBB; 20]);
        let base_ts = 1700000000u64;

        let mut statuses = Vec::new();

        // Create requests for prover1 with different timestamps (as lock_prover)
        for i in 0..3u8 {
            let digest = B256::from([i + 1; 32]);
            let ts = base_ts + (i as u64 * 1000);
            statuses.push(create_locked_request_status_for_prover(
                digest,
                client,
                prover1,
                ts,
                U256::from(100),
            ));
        }

        // Create request for prover2
        let digest = B256::from([0x10; 32]);
        let ts2 = base_ts + 5000;
        statuses.push(create_locked_request_status_for_prover(
            digest,
            client,
            prover2,
            ts2,
            U256::from(100),
        ));

        db.upsert_request_statuses(&statuses).await.unwrap();

        let activities = db.get_prover_last_activity_times(&[prover1, prover2]).await.unwrap();

        assert_eq!(activities.len(), 2);
        // Prover1's last activity is at base_ts + 2000 (the third request)
        assert_eq!(activities.get(&prover1), Some(&(base_ts + 2000)));
        // Prover2's last activity is at base_ts + 5000
        assert_eq!(activities.get(&prover2), Some(&(base_ts + 5000)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_prover_leaderboard_empty(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        // Query leaderboard with no data
        let results =
            db.get_prover_leaderboard(1700000000, 1700003600, true, None, None, 10).await.unwrap();

        assert!(results.is_empty());
    }
}
