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
    padded_string_to_u256, u256_to_padded_string, AllTimeRequestorSummary, DailyRequestorSummary,
    IndexerDb, LockPricingData, MonthlyRequestorSummary, PeriodRequestorSummary, RequestCursor,
    RequestSortField, RequestStatus, RequestorLeaderboardEntry, SortDirection,
    WeeklyRequestorSummary,
};
use super::DbError;
use alloy::primitives::{Address, U256};
use async_trait::async_trait;
use sqlx::{postgres::PgRow, PgPool, Row};
use std::str::FromStr;

// Standalone parsing function for PeriodRequestorSummary
fn parse_period_requestor_summary_row(row: &PgRow) -> Result<PeriodRequestorSummary, DbError> {
    let period_timestamp: i64 = row.try_get("period_timestamp")?;
    let requestor_address_str: String = row.try_get("requestor_address")?;
    let requestor_address = Address::from_str(&requestor_address_str)
        .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid requestor address: {}", e)))?;
    let total_fulfilled: i64 = row.try_get("total_fulfilled")?;
    let unique_provers: i64 = row.try_get("unique_provers_locking_requests")?;
    let total_fees_locked_str: String = row.try_get("total_fees_locked")?;
    let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
    let total_locked_and_expired_collateral_str: String =
        row.try_get("total_locked_and_expired_collateral")?;
    let p10_str: String = row.try_get("p10_lock_price_per_cycle")?;
    let p25_str: String = row.try_get("p25_lock_price_per_cycle")?;
    let p50_str: String = row.try_get("p50_lock_price_per_cycle")?;
    let p75_str: String = row.try_get("p75_lock_price_per_cycle")?;
    let p90_str: String = row.try_get("p90_lock_price_per_cycle")?;
    let p95_str: String = row.try_get("p95_lock_price_per_cycle")?;
    let p99_str: String = row.try_get("p99_lock_price_per_cycle")?;
    let total_requests_submitted: i64 = row.try_get("total_requests_submitted")?;
    let total_requests_submitted_onchain: i64 = row.try_get("total_requests_submitted_onchain")?;
    let total_requests_submitted_offchain: i64 =
        row.try_get("total_requests_submitted_offchain")?;
    let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
    let total_requests_slashed: i64 = row.try_get("total_requests_slashed")?;
    let total_expired: i64 = row.try_get("total_expired")?;
    let total_locked_and_expired: i64 = row.try_get("total_locked_and_expired")?;
    let total_locked_and_fulfilled: i64 = row.try_get("total_locked_and_fulfilled")?;
    let total_secondary_fulfillments: i64 = row.try_get("total_secondary_fulfillments")?;
    let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
    let locked_orders_fulfillment_rate_adjusted: f64 = row
        .try_get::<Option<f64>, _>("locked_orders_fulfillment_rate_adjusted")
        .ok()
        .flatten()
        .unwrap_or(0.0);
    let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
    let total_cycles_str: String = row.try_get("total_cycles")?;
    let best_peak_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_peak_prove_mhz_v2").ok().flatten().unwrap_or(0.0);
    let best_peak_prove_mhz_prover: Option<String> = row.try_get("best_peak_prove_mhz_prover").ok();
    let best_peak_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_peak_prove_mhz_request_id").ok();
    let best_effective_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_effective_prove_mhz_v2").ok().flatten().unwrap_or(0.0);
    let best_effective_prove_mhz_prover: Option<String> =
        row.try_get("best_effective_prove_mhz_prover").ok();
    let best_effective_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_effective_prove_mhz_request_id").ok();

    Ok(PeriodRequestorSummary {
        period_timestamp: period_timestamp as u64,
        requestor_address,
        total_fulfilled: total_fulfilled as u64,
        unique_provers_locking_requests: unique_provers as u64,
        total_fees_locked: padded_string_to_u256(&total_fees_locked_str)?,
        total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
        total_locked_and_expired_collateral: padded_string_to_u256(
            &total_locked_and_expired_collateral_str,
        )?,
        p10_lock_price_per_cycle: padded_string_to_u256(&p10_str)?,
        p25_lock_price_per_cycle: padded_string_to_u256(&p25_str)?,
        p50_lock_price_per_cycle: padded_string_to_u256(&p50_str)?,
        p75_lock_price_per_cycle: padded_string_to_u256(&p75_str)?,
        p90_lock_price_per_cycle: padded_string_to_u256(&p90_str)?,
        p95_lock_price_per_cycle: padded_string_to_u256(&p95_str)?,
        p99_lock_price_per_cycle: padded_string_to_u256(&p99_str)?,
        total_requests_submitted: total_requests_submitted as u64,
        total_requests_submitted_onchain: total_requests_submitted_onchain as u64,
        total_requests_submitted_offchain: total_requests_submitted_offchain as u64,
        total_requests_locked: total_requests_locked as u64,
        total_requests_slashed: total_requests_slashed as u64,
        total_expired: total_expired as u64,
        total_locked_and_expired: total_locked_and_expired as u64,
        total_locked_and_fulfilled: total_locked_and_fulfilled as u64,
        total_secondary_fulfillments: total_secondary_fulfillments as u64,
        locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
        locked_orders_fulfillment_rate_adjusted: locked_orders_fulfillment_rate_adjusted as f32,
        total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
        total_cycles: padded_string_to_u256(&total_cycles_str)?,
        best_peak_prove_mhz,
        best_peak_prove_mhz_prover,
        best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
        best_effective_prove_mhz,
        best_effective_prove_mhz_prover,
        best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
    })
}

// Standalone parsing function for AllTimeRequestorSummary
fn parse_all_time_requestor_summary_row(row: &PgRow) -> Result<AllTimeRequestorSummary, DbError> {
    let period_timestamp: i64 = row.try_get("period_timestamp")?;
    let requestor_address_str: String = row.try_get("requestor_address")?;
    let requestor_address = Address::from_str(&requestor_address_str)
        .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid requestor address: {}", e)))?;
    let total_fulfilled: i64 = row.try_get("total_fulfilled")?;
    let unique_provers: i64 = row.try_get("unique_provers_locking_requests")?;
    let total_fees_locked_str: String = row.try_get("total_fees_locked")?;
    let total_collateral_locked_str: String = row.try_get("total_collateral_locked")?;
    let total_locked_and_expired_collateral_str: String =
        row.try_get("total_locked_and_expired_collateral")?;
    let total_requests_submitted: i64 = row.try_get("total_requests_submitted")?;
    let total_requests_submitted_onchain: i64 = row.try_get("total_requests_submitted_onchain")?;
    let total_requests_submitted_offchain: i64 =
        row.try_get("total_requests_submitted_offchain")?;
    let total_requests_locked: i64 = row.try_get("total_requests_locked")?;
    let total_requests_slashed: i64 = row.try_get("total_requests_slashed")?;
    let total_expired: i64 = row.try_get("total_expired")?;
    let total_locked_and_expired: i64 = row.try_get("total_locked_and_expired")?;
    let total_locked_and_fulfilled: i64 = row.try_get("total_locked_and_fulfilled")?;
    let total_secondary_fulfillments: i64 = row.try_get("total_secondary_fulfillments")?;
    let locked_orders_fulfillment_rate: f64 = row.try_get("locked_orders_fulfillment_rate")?;
    let locked_orders_fulfillment_rate_adjusted: f64 = row
        .try_get::<Option<f64>, _>("locked_orders_fulfillment_rate_adjusted")
        .ok()
        .flatten()
        .unwrap_or(0.0);
    let total_program_cycles_str: String = row.try_get("total_program_cycles")?;
    let total_cycles_str: String = row.try_get("total_cycles")?;
    let best_peak_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_peak_prove_mhz_v2").ok().flatten().unwrap_or(0.0);
    let best_peak_prove_mhz_prover: Option<String> = row.try_get("best_peak_prove_mhz_prover").ok();
    let best_peak_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_peak_prove_mhz_request_id").ok();
    let best_effective_prove_mhz: f64 =
        row.try_get::<Option<f64>, _>("best_effective_prove_mhz_v2").ok().flatten().unwrap_or(0.0);
    let best_effective_prove_mhz_prover: Option<String> =
        row.try_get("best_effective_prove_mhz_prover").ok();
    let best_effective_prove_mhz_request_id_str: Option<String> =
        row.try_get("best_effective_prove_mhz_request_id").ok();

    Ok(AllTimeRequestorSummary {
        period_timestamp: period_timestamp as u64,
        requestor_address,
        total_fulfilled: total_fulfilled as u64,
        unique_provers_locking_requests: unique_provers as u64,
        total_fees_locked: padded_string_to_u256(&total_fees_locked_str)?,
        total_collateral_locked: padded_string_to_u256(&total_collateral_locked_str)?,
        total_locked_and_expired_collateral: padded_string_to_u256(
            &total_locked_and_expired_collateral_str,
        )?,
        total_requests_submitted: total_requests_submitted as u64,
        total_requests_submitted_onchain: total_requests_submitted_onchain as u64,
        total_requests_submitted_offchain: total_requests_submitted_offchain as u64,
        total_requests_locked: total_requests_locked as u64,
        total_requests_slashed: total_requests_slashed as u64,
        total_expired: total_expired as u64,
        total_locked_and_expired: total_locked_and_expired as u64,
        total_locked_and_fulfilled: total_locked_and_fulfilled as u64,
        total_secondary_fulfillments: total_secondary_fulfillments as u64,
        locked_orders_fulfillment_rate: locked_orders_fulfillment_rate as f32,
        locked_orders_fulfillment_rate_adjusted: locked_orders_fulfillment_rate_adjusted as f32,
        total_program_cycles: padded_string_to_u256(&total_program_cycles_str)?,
        total_cycles: padded_string_to_u256(&total_cycles_str)?,
        best_peak_prove_mhz,
        best_peak_prove_mhz_prover,
        best_peak_prove_mhz_request_id: best_peak_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
        best_effective_prove_mhz,
        best_effective_prove_mhz_prover,
        best_effective_prove_mhz_request_id: best_effective_prove_mhz_request_id_str
            .and_then(|s| U256::from_str(&s).ok()),
    })
}

/// Extension trait for requestor-related database operations.
/// Extends IndexerDb to get pool() access.
#[async_trait]
pub trait RequestorDb: IndexerDb {
    // === List requests by requestor ===

    async fn list_requests_by_requestor(
        &self,
        client_address: Address,
        cursor: Option<RequestCursor>,
        limit: u32,
        sort_by: RequestSortField,
    ) -> Result<(Vec<RequestStatus>, Option<RequestCursor>), DbError> {
        let client_str = format!("{:x}", client_address);
        let sort_field = match sort_by {
            RequestSortField::UpdatedAt => "updated_at",
            RequestSortField::CreatedAt => "created_at",
        };

        let rows = if let Some(c) = &cursor {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE client_address = $1
                   AND ({} < $2 OR ({} = $2 AND request_digest < $3))
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $4",
                sort_field, sort_field, sort_field
            );
            sqlx::query(&query_str)
                .bind(&client_str)
                .bind(c.timestamp as i64)
                .bind(&c.request_digest)
                .bind(limit as i64)
                .fetch_all(self.pool())
                .await?
        } else {
            let query_str = format!(
                "SELECT * FROM request_status
                 WHERE client_address = $1
                 ORDER BY {} DESC, request_digest DESC
                 LIMIT $2",
                sort_field
            );
            sqlx::query(&query_str)
                .bind(&client_str)
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

    // === Per-Requestor Aggregate Methods ===

    async fn upsert_hourly_requestor_summary(
        &self,
        summary: PeriodRequestorSummary,
    ) -> Result<(), DbError> {
        upsert_requestor_summary_generic(self.pool(), summary, "hourly_requestor_summary").await
    }

    async fn upsert_daily_requestor_summary(
        &self,
        summary: DailyRequestorSummary,
    ) -> Result<(), DbError> {
        upsert_requestor_summary_generic(self.pool(), summary, "daily_requestor_summary").await
    }

    async fn upsert_weekly_requestor_summary(
        &self,
        summary: WeeklyRequestorSummary,
    ) -> Result<(), DbError> {
        upsert_requestor_summary_generic(self.pool(), summary, "weekly_requestor_summary").await
    }

    async fn upsert_monthly_requestor_summary(
        &self,
        summary: MonthlyRequestorSummary,
    ) -> Result<(), DbError> {
        upsert_requestor_summary_generic(self.pool(), summary, "monthly_requestor_summary").await
    }

    async fn upsert_all_time_requestor_summary(
        &self,
        summary: AllTimeRequestorSummary,
    ) -> Result<(), DbError> {
        let query_str = "INSERT INTO all_time_requestor_summary (
                period_timestamp,
                requestor_address,
                total_fulfilled,
                unique_provers_locking_requests,
                total_fees_locked,
                total_collateral_locked,
                total_locked_and_expired_collateral,
                total_requests_submitted,
                total_requests_submitted_onchain,
                total_requests_submitted_offchain,
                total_requests_locked,
                total_requests_slashed,
                total_expired,
                total_locked_and_expired,
                total_locked_and_fulfilled,
                total_secondary_fulfillments,
                locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted,
                total_program_cycles,
                total_cycles,
                best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id,
                best_peak_prove_mhz_v2,
                best_effective_prove_mhz_v2,
                updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, CAST($25 AS DOUBLE PRECISION), CAST($26 AS DOUBLE PRECISION), CURRENT_TIMESTAMP)
            ON CONFLICT (period_timestamp, requestor_address) DO UPDATE SET
                total_fulfilled = EXCLUDED.total_fulfilled,
                unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
                total_fees_locked = EXCLUDED.total_fees_locked,
                total_collateral_locked = EXCLUDED.total_collateral_locked,
                total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
                total_requests_submitted = EXCLUDED.total_requests_submitted,
                total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
                total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
                total_requests_locked = EXCLUDED.total_requests_locked,
                total_requests_slashed = EXCLUDED.total_requests_slashed,
                total_expired = EXCLUDED.total_expired,
                total_locked_and_expired = EXCLUDED.total_locked_and_expired,
                total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
                total_secondary_fulfillments = EXCLUDED.total_secondary_fulfillments,
                locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
                locked_orders_fulfillment_rate_adjusted = EXCLUDED.locked_orders_fulfillment_rate_adjusted,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                best_peak_prove_mhz_v2 = EXCLUDED.best_peak_prove_mhz_v2,
                best_effective_prove_mhz_v2 = EXCLUDED.best_effective_prove_mhz_v2,
                updated_at = CURRENT_TIMESTAMP";

        sqlx::query(query_str)
            .bind(summary.period_timestamp as i64)
            .bind(format!("{:x}", summary.requestor_address))
            .bind(summary.total_fulfilled as i64)
            .bind(summary.unique_provers_locking_requests as i64)
            .bind(u256_to_padded_string(summary.total_fees_locked))
            .bind(u256_to_padded_string(summary.total_collateral_locked))
            .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
            .bind(summary.total_requests_submitted as i64)
            .bind(summary.total_requests_submitted_onchain as i64)
            .bind(summary.total_requests_submitted_offchain as i64)
            .bind(summary.total_requests_locked as i64)
            .bind(summary.total_requests_slashed as i64)
            .bind(summary.total_expired as i64)
            .bind(summary.total_locked_and_expired as i64)
            .bind(summary.total_locked_and_fulfilled as i64)
            .bind(summary.total_secondary_fulfillments as i64)
            .bind(summary.locked_orders_fulfillment_rate)
            .bind(summary.locked_orders_fulfillment_rate_adjusted)
            .bind(u256_to_padded_string(summary.total_program_cycles))
            .bind(u256_to_padded_string(summary.total_cycles))
            .bind(summary.best_peak_prove_mhz_prover)
            .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_effective_prove_mhz_prover)
            .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
            .bind(summary.best_peak_prove_mhz.to_string())
            .bind(summary.best_effective_prove_mhz.to_string())
            .execute(self.pool())
            .await?;

        Ok(())
    }

    async fn get_hourly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError> {
        get_requestor_summaries_by_range_generic(
            self.pool(),
            requestor_address,
            start_ts,
            end_ts,
            "hourly_requestor_summary",
        )
        .await
    }

    async fn get_daily_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<DailyRequestorSummary>, DbError> {
        get_requestor_summaries_by_range_generic(
            self.pool(),
            requestor_address,
            start_ts,
            end_ts,
            "daily_requestor_summary",
        )
        .await
    }

    async fn get_weekly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<WeeklyRequestorSummary>, DbError> {
        get_requestor_summaries_by_range_generic(
            self.pool(),
            requestor_address,
            start_ts,
            end_ts,
            "weekly_requestor_summary",
        )
        .await
    }

    async fn get_monthly_requestor_summaries_by_range(
        &self,
        requestor_address: Address,
        start_ts: u64,
        end_ts: u64,
    ) -> Result<Vec<MonthlyRequestorSummary>, DbError> {
        get_requestor_summaries_by_range_generic(
            self.pool(),
            requestor_address,
            start_ts,
            end_ts,
            "monthly_requestor_summary",
        )
        .await
    }

    async fn get_hourly_requestor_summaries(
        &self,
        requestor_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<PeriodRequestorSummary>, DbError> {
        get_requestor_summaries_generic(
            self.pool(),
            requestor_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "hourly_requestor_summary",
        )
        .await
    }

    async fn get_daily_requestor_summaries(
        &self,
        requestor_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<DailyRequestorSummary>, DbError> {
        get_requestor_summaries_generic(
            self.pool(),
            requestor_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "daily_requestor_summary",
        )
        .await
    }

    async fn get_weekly_requestor_summaries(
        &self,
        requestor_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<WeeklyRequestorSummary>, DbError> {
        get_requestor_summaries_generic(
            self.pool(),
            requestor_address,
            cursor,
            limit,
            sort,
            before,
            after,
            "weekly_requestor_summary",
        )
        .await
    }

    async fn get_all_time_requestor_summaries(
        &self,
        requestor_address: Address,
        cursor: Option<i64>,
        limit: i64,
        sort: SortDirection,
        before: Option<i64>,
        after: Option<i64>,
    ) -> Result<Vec<AllTimeRequestorSummary>, DbError> {
        get_all_time_requestor_summaries_generic(
            self.pool(),
            requestor_address,
            cursor,
            limit,
            sort,
            before,
            after,
        )
        .await
    }

    async fn get_latest_all_time_requestor_summary(
        &self,
        requestor_address: Address,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        let query_str = "SELECT 
                period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
                total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
                total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
                total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
                total_locked_and_fulfilled, total_secondary_fulfillments, locked_orders_fulfillment_rate,
                total_program_cycles, total_cycles,
                best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id,
                best_peak_prove_mhz_v2, best_effective_prove_mhz_v2
            FROM all_time_requestor_summary 
            WHERE requestor_address = $1 
            ORDER BY period_timestamp DESC 
            LIMIT 1";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", requestor_address))
            .fetch_optional(self.pool())
            .await?;

        match row {
            Some(row) => Ok(Some(parse_all_time_requestor_summary_row(&row)?)),
            None => Ok(None),
        }
    }

    async fn get_all_time_requestor_summary_by_timestamp(
        &self,
        requestor_address: Address,
        period_timestamp: u64,
    ) -> Result<Option<AllTimeRequestorSummary>, DbError> {
        let query_str = "SELECT 
                period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
                total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
                total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
                total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
                total_locked_and_fulfilled, total_secondary_fulfillments, locked_orders_fulfillment_rate,
                total_program_cycles, total_cycles,
                best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id,
                best_peak_prove_mhz_v2, best_effective_prove_mhz_v2
            FROM all_time_requestor_summary 
            WHERE requestor_address = $1 AND period_timestamp = $2";

        let row = sqlx::query(query_str)
            .bind(format!("{:x}", requestor_address))
            .bind(period_timestamp as i64)
            .fetch_optional(self.pool())
            .await?;

        match row {
            Some(row) => Ok(Some(parse_all_time_requestor_summary_row(&row)?)),
            None => Ok(None),
        }
    }

    /// Gets all unique requestor addresses that have submitted requests
    async fn get_all_requestor_addresses(&self) -> Result<Vec<Address>, DbError> {
        let query_str =
            "SELECT DISTINCT client_address FROM proof_requests ORDER BY client_address";

        let rows = sqlx::query(query_str).fetch_all(self.pool()).await?;

        let addresses = rows
            .iter()
            .map(|row| {
                let addr_str: String = row.try_get("client_address")?;
                Address::from_str(&addr_str)
                    .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(addresses)
    }

    /// Gets requestor addresses that were active (submitted requests) in the given period
    async fn get_active_requestor_addresses_in_period(
        &self,
        period_start: u64,
        period_end: u64,
    ) -> Result<Vec<Address>, DbError> {
        let query_str = "SELECT DISTINCT client_address 
            FROM (
                -- Submissions from proof_requests
                SELECT DISTINCT pr.client_address 
                FROM proof_requests pr
                WHERE pr.submission_timestamp >= $1 AND pr.submission_timestamp < $2
                
                UNION
                
                -- Lock events
                SELECT DISTINCT rs.client_address
                FROM request_locked_events rle
                JOIN request_status rs ON rle.request_digest = rs.request_digest
                WHERE rle.block_timestamp >= $1 AND rle.block_timestamp < $2
                
                UNION
                
                -- Fulfillment events
                SELECT DISTINCT rs.client_address
                FROM request_fulfilled_events rfe
                JOIN request_status rs ON rfe.request_digest = rs.request_digest
                WHERE rfe.block_timestamp >= $1 AND rfe.block_timestamp < $2
                
                UNION
                
                -- Slash events
                SELECT DISTINCT rs.client_address
                FROM prover_slashed_events pse
                JOIN request_status rs ON pse.request_id = rs.request_id
                WHERE pse.block_timestamp >= $1 AND pse.block_timestamp < $2
                
                UNION
                
                -- Expiration events
                SELECT DISTINCT rs.client_address
                FROM request_status rs
                WHERE rs.expires_at >= $1 AND rs.expires_at < $2
                AND rs.request_status = 'expired'
            ) AS active_requestors
            ORDER BY client_address";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .fetch_all(self.pool())
            .await?;

        let addresses = rows
            .iter()
            .map(|row| {
                let addr_str: String = row.try_get("client_address")?;
                Address::from_str(&addr_str)
                    .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(addresses)
    }

    // === Per-requestor period query methods ===

    async fn get_period_requestor_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_fulfilled_events rfe
            JOIN request_status rs ON rfe.request_digest = rs.request_digest
            WHERE rfe.block_timestamp >= $1 
            AND rfe.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_unique_provers(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT rle.prover_address) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_total_requests_submitted(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE created_at >= $1 
            AND created_at < $2
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_total_requests_submitted_onchain(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_submitted_events rse
            JOIN request_status rs ON rse.request_digest = rs.request_digest
            WHERE rse.block_timestamp >= $1 
            AND rse.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_total_requests_locked(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_total_requests_slashed(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM prover_slashed_events pse
            JOIN request_status rs ON pse.request_id = rs.request_id
            WHERE pse.block_timestamp >= $1 
            AND pse.block_timestamp < $2
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_lock_pricing_data(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<LockPricingData>, DbError> {
        let query_str = "SELECT 
                rs.request_digest,
                rs.min_price,
                rs.max_price,
                rs.ramp_up_start,
                rs.ramp_up_period,
                rs.lock_end,
                rs.fulfilled_at as lock_timestamp,
                rs.lock_price,
                rs.lock_price_per_cycle,
                rs.lock_collateral
            FROM request_status rs
            WHERE rs.fulfilled_at IS NOT NULL
            AND rs.fulfilled_at >= $1
            AND rs.fulfilled_at < $2
            AND rs.lock_prover_address = rs.fulfill_prover_address
            AND rs.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
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

    async fn get_period_requestor_all_lock_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        let query_str = "SELECT pr.lock_collateral 
            FROM request_locked_events rle
            JOIN proof_requests pr ON rle.request_digest = pr.request_digest
            WHERE rle.block_timestamp >= $1 
            AND rle.block_timestamp < $2
            AND pr.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_all(self.pool())
            .await?;

        let collaterals = rows
            .iter()
            .map(|row| row.try_get("lock_collateral"))
            .collect::<Result<Vec<String>, _>>()?;

        Ok(collaterals)
    }

    async fn get_period_requestor_locked_and_expired_collateral(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<Vec<String>, DbError> {
        let query_str = "SELECT rs.lock_collateral 
            FROM request_status rs
            WHERE rs.expires_at >= $1 
            AND rs.expires_at < $2
            AND rs.request_status = 'expired'
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";

        let rows = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_all(self.pool())
            .await?;

        let collaterals = rows
            .iter()
            .map(|row| row.try_get("lock_collateral"))
            .collect::<Result<Vec<String>, _>>()?;

        Ok(collaterals)
    }

    async fn get_period_requestor_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE expires_at >= $1 
            AND expires_at < $2
            AND request_status = 'expired'
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_locked_and_expired_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE expires_at >= $1 
            AND expires_at < $2
            AND request_status = 'expired'
            AND locked_at IS NOT NULL
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_locked_and_fulfilled_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE fulfilled_at >= $1 
            AND fulfilled_at < $2
            AND locked_at IS NOT NULL
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_secondary_fulfillments_count(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(*) as count 
            FROM request_status 
            WHERE request_status = 'fulfilled'
            AND fulfilled_at IS NOT NULL
            AND fulfilled_at > lock_end
            AND fulfilled_at < expires_at
            AND fulfilled_at >= $1 
            AND fulfilled_at < $2
            AND client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_locked_and_fulfilled_count_adjusted(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count 
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.fulfilled_at >= $1 
            AND rs.fulfilled_at < $2
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_locked_and_expired_count_adjusted(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count 
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.expires_at >= $1 
            AND rs.expires_at < $2
            AND rs.request_status = 'expired'
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";

        let row = sqlx::query(query_str)
            .bind(period_start as i64)
            .bind(period_end as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_period_requestor_total_program_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT program_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND program_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2
             AND client_address = $3",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .bind(format!("{:x}", requestor_address))
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

    async fn get_period_requestor_total_cycles(
        &self,
        period_start: u64,
        period_end: u64,
        requestor_address: Address,
    ) -> Result<U256, DbError> {
        let rows = sqlx::query(
            "SELECT total_cycles FROM request_status
             WHERE request_status = 'fulfilled'
             AND total_cycles IS NOT NULL
             AND fulfilled_at IS NOT NULL
             AND fulfilled_at >= $1 AND fulfilled_at < $2
             AND client_address = $3",
        )
        .bind(period_start as i64)
        .bind(period_end as i64)
        .bind(format!("{:x}", requestor_address))
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

    async fn get_all_time_requestor_unique_provers(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT rle.prover_address) as count 
            FROM request_locked_events rle
            JOIN request_status rs ON rle.request_digest = rs.request_digest
            WHERE rle.block_timestamp < $1
            AND rs.client_address = $2";

        let row = sqlx::query(query_str)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_all_time_requestor_locked_and_fulfilled_count_adjusted(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count 
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.fulfilled_at < $1
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $2";

        let row = sqlx::query(query_str)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    async fn get_all_time_requestor_locked_and_expired_count_adjusted(
        &self,
        end_ts: u64,
        requestor_address: Address,
    ) -> Result<u64, DbError> {
        let query_str = "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count 
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.expires_at < $1
            AND rs.request_status = 'expired'
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $2";

        let row = sqlx::query(query_str)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_one(self.pool())
            .await?;

        let count: i64 = row.try_get("count")?;
        Ok(count as u64)
    }

    // Leaderboard query: returns aggregated stats for all requestors in a time period
    // Sorted by orders_requested DESC
    async fn get_requestor_leaderboard(
        &self,
        start_ts: u64,
        end_ts: u64,
        use_hourly_table: bool,
        cursor_orders: Option<u64>,
        cursor_address: Option<Address>,
        limit: i64,
    ) -> Result<Vec<RequestorLeaderboardEntry>, DbError> {
        get_requestor_leaderboard_impl(
            self.pool(),
            start_ts,
            end_ts,
            use_hourly_table,
            cursor_orders,
            cursor_address,
            limit,
        )
        .await
    }

    // Get median lock price per cycle for a list of requestors in a time period
    async fn get_requestor_median_lock_prices(
        &self,
        requestor_addresses: &[Address],
        start_ts: u64,
        end_ts: u64,
    ) -> Result<std::collections::HashMap<Address, U256>, DbError> {
        get_requestor_median_lock_prices_impl(self.pool(), requestor_addresses, start_ts, end_ts)
            .await
    }

    // Get last activity time for a list of requestors
    async fn get_requestor_last_activity_times(
        &self,
        requestor_addresses: &[Address],
    ) -> Result<std::collections::HashMap<Address, u64>, DbError> {
        get_requestor_last_activity_times_impl(self.pool(), requestor_addresses).await
    }
}

// Blanket implementation for all IndexerDb implementors
impl<T: IndexerDb + Send + Sync> RequestorDb for T {}

// === Standalone helper functions for generic operations ===

async fn upsert_requestor_summary_generic(
    pool: &PgPool,
    summary: PeriodRequestorSummary,
    table_name: &str,
) -> Result<(), DbError> {
    let query_str = format!(
        "INSERT INTO {} (
            period_timestamp,
            requestor_address,
            total_fulfilled,
            unique_provers_locking_requests,
            total_fees_locked,
            total_collateral_locked,
            total_locked_and_expired_collateral,
            p10_lock_price_per_cycle,
            p25_lock_price_per_cycle,
            p50_lock_price_per_cycle,
            p75_lock_price_per_cycle,
            p90_lock_price_per_cycle,
            p95_lock_price_per_cycle,
            p99_lock_price_per_cycle,
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            total_secondary_fulfillments,
            locked_orders_fulfillment_rate,
            locked_orders_fulfillment_rate_adjusted,
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id,
            best_peak_prove_mhz_v2,
            best_effective_prove_mhz_v2,
            updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, CAST($32 AS DOUBLE PRECISION), CAST($33 AS DOUBLE PRECISION), CURRENT_TIMESTAMP)
        ON CONFLICT (period_timestamp, requestor_address) DO UPDATE SET
            total_fulfilled = EXCLUDED.total_fulfilled,
            unique_provers_locking_requests = EXCLUDED.unique_provers_locking_requests,
            total_fees_locked = EXCLUDED.total_fees_locked,
            total_collateral_locked = EXCLUDED.total_collateral_locked,
            total_locked_and_expired_collateral = EXCLUDED.total_locked_and_expired_collateral,
            p10_lock_price_per_cycle = EXCLUDED.p10_lock_price_per_cycle,
            p25_lock_price_per_cycle = EXCLUDED.p25_lock_price_per_cycle,
            p50_lock_price_per_cycle = EXCLUDED.p50_lock_price_per_cycle,
            p75_lock_price_per_cycle = EXCLUDED.p75_lock_price_per_cycle,
            p90_lock_price_per_cycle = EXCLUDED.p90_lock_price_per_cycle,
            p95_lock_price_per_cycle = EXCLUDED.p95_lock_price_per_cycle,
            p99_lock_price_per_cycle = EXCLUDED.p99_lock_price_per_cycle,
            total_requests_submitted = EXCLUDED.total_requests_submitted,
            total_requests_submitted_onchain = EXCLUDED.total_requests_submitted_onchain,
            total_requests_submitted_offchain = EXCLUDED.total_requests_submitted_offchain,
            total_requests_locked = EXCLUDED.total_requests_locked,
            total_requests_slashed = EXCLUDED.total_requests_slashed,
            total_expired = EXCLUDED.total_expired,
            total_locked_and_expired = EXCLUDED.total_locked_and_expired,
            total_locked_and_fulfilled = EXCLUDED.total_locked_and_fulfilled,
            total_secondary_fulfillments = EXCLUDED.total_secondary_fulfillments,
            locked_orders_fulfillment_rate = EXCLUDED.locked_orders_fulfillment_rate,
            locked_orders_fulfillment_rate_adjusted = EXCLUDED.locked_orders_fulfillment_rate_adjusted,
                total_program_cycles = EXCLUDED.total_program_cycles,
                total_cycles = EXCLUDED.total_cycles,
                best_peak_prove_mhz_prover = EXCLUDED.best_peak_prove_mhz_prover,
                best_peak_prove_mhz_request_id = EXCLUDED.best_peak_prove_mhz_request_id,
                best_effective_prove_mhz_prover = EXCLUDED.best_effective_prove_mhz_prover,
                best_effective_prove_mhz_request_id = EXCLUDED.best_effective_prove_mhz_request_id,
                best_peak_prove_mhz_v2 = EXCLUDED.best_peak_prove_mhz_v2,
                best_effective_prove_mhz_v2 = EXCLUDED.best_effective_prove_mhz_v2,
                updated_at = CURRENT_TIMESTAMP",
        table_name
    );

    sqlx::query(&query_str)
        .bind(summary.period_timestamp as i64)
        .bind(format!("{:x}", summary.requestor_address))
        .bind(summary.total_fulfilled as i64)
        .bind(summary.unique_provers_locking_requests as i64)
        .bind(u256_to_padded_string(summary.total_fees_locked))
        .bind(u256_to_padded_string(summary.total_collateral_locked))
        .bind(u256_to_padded_string(summary.total_locked_and_expired_collateral))
        .bind(u256_to_padded_string(summary.p10_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p25_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p50_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p75_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p90_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p95_lock_price_per_cycle))
        .bind(u256_to_padded_string(summary.p99_lock_price_per_cycle))
        .bind(summary.total_requests_submitted as i64)
        .bind(summary.total_requests_submitted_onchain as i64)
        .bind(summary.total_requests_submitted_offchain as i64)
        .bind(summary.total_requests_locked as i64)
        .bind(summary.total_requests_slashed as i64)
        .bind(summary.total_expired as i64)
        .bind(summary.total_locked_and_expired as i64)
        .bind(summary.total_locked_and_fulfilled as i64)
        .bind(summary.total_secondary_fulfillments as i64)
        .bind(summary.locked_orders_fulfillment_rate)
        .bind(summary.locked_orders_fulfillment_rate_adjusted)
        .bind(u256_to_padded_string(summary.total_program_cycles))
        .bind(u256_to_padded_string(summary.total_cycles))
        .bind(summary.best_peak_prove_mhz_prover)
        .bind(summary.best_peak_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .bind(summary.best_effective_prove_mhz_prover)
        .bind(summary.best_effective_prove_mhz_request_id.map(|id| format!("{:x}", id)))
        .bind(summary.best_peak_prove_mhz.to_string())
        .bind(summary.best_effective_prove_mhz.to_string())
        .execute(pool)
        .await?;

    Ok(())
}

async fn get_requestor_summaries_by_range_generic(
    pool: &PgPool,
    requestor_address: Address,
    start_ts: u64,
    end_ts: u64,
    table_name: &str,
) -> Result<Vec<PeriodRequestorSummary>, DbError> {
    let query_str = format!(
        "SELECT 
            period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
            total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
            p10_lock_price_per_cycle, p25_lock_price_per_cycle, p50_lock_price_per_cycle,
            p75_lock_price_per_cycle, p90_lock_price_per_cycle, p95_lock_price_per_cycle, p99_lock_price_per_cycle,
            total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
            total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
            total_locked_and_fulfilled, total_secondary_fulfillments, locked_orders_fulfillment_rate,
            total_program_cycles, total_cycles,
            best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz_v2, best_effective_prove_mhz_v2
        FROM {} WHERE requestor_address = $1 AND period_timestamp >= $2 AND period_timestamp < $3 ORDER BY period_timestamp ASC",
        table_name
    );

    let rows = sqlx::query(&query_str)
        .bind(format!("{:x}", requestor_address))
        .bind(start_ts as i64)
        .bind(end_ts as i64)
        .fetch_all(pool)
        .await?;

    let mut summaries = Vec::new();
    for row in rows {
        summaries.push(parse_period_requestor_summary_row(&row)?);
    }

    Ok(summaries)
}

#[allow(clippy::too_many_arguments)]
async fn get_requestor_summaries_generic(
    pool: &PgPool,
    requestor_address: Address,
    cursor: Option<i64>,
    limit: i64,
    sort: SortDirection,
    before: Option<i64>,
    after: Option<i64>,
    table_name: &str,
) -> Result<Vec<PeriodRequestorSummary>, DbError> {
    let mut conditions = Vec::new();
    let mut bind_count = 0;

    // Always filter by requestor_address
    bind_count += 1;
    conditions.push(format!("requestor_address = ${}", bind_count));

    // Add cursor condition
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

    // Add after condition
    if after.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp > ${}", bind_count));
    }

    // Add before condition
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
            period_timestamp, requestor_address, total_fulfilled, unique_provers_locking_requests,
            total_fees_locked, total_collateral_locked, total_locked_and_expired_collateral,
            p10_lock_price_per_cycle, p25_lock_price_per_cycle, p50_lock_price_per_cycle,
            p75_lock_price_per_cycle, p90_lock_price_per_cycle, p95_lock_price_per_cycle, p99_lock_price_per_cycle,
            total_requests_submitted, total_requests_submitted_onchain, total_requests_submitted_offchain,
            total_requests_locked, total_requests_slashed, total_expired, total_locked_and_expired,
            total_locked_and_fulfilled, total_secondary_fulfillments, locked_orders_fulfillment_rate,
            total_program_cycles, total_cycles,
            best_peak_prove_mhz_prover, best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_prover, best_effective_prove_mhz_request_id,
            best_peak_prove_mhz_v2, best_effective_prove_mhz_v2
        FROM {}
        {}
        {}
        LIMIT ${}",
        table_name, where_clause, order_clause, bind_count
    );

    let mut query = sqlx::query(&query_str);

    // Bind parameters in the same order as bind_count increments
    query = query.bind(format!("{:x}", requestor_address));

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
        .map(|row| parse_period_requestor_summary_row(&row))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(summaries)
}

async fn get_all_time_requestor_summaries_generic(
    pool: &PgPool,
    requestor_address: Address,
    cursor: Option<i64>,
    limit: i64,
    sort: SortDirection,
    before: Option<i64>,
    after: Option<i64>,
) -> Result<Vec<AllTimeRequestorSummary>, DbError> {
    let mut conditions = Vec::new();
    let mut bind_count = 0;

    // Always filter by requestor_address
    bind_count += 1;
    conditions.push(format!("requestor_address = ${}", bind_count));

    // Add cursor condition
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

    // Add after condition
    if after.is_some() {
        bind_count += 1;
        conditions.push(format!("period_timestamp > ${}", bind_count));
    }

    // Add before condition
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
            period_timestamp,
            requestor_address,
            total_fulfilled,
            unique_provers_locking_requests,
            total_fees_locked,
            total_collateral_locked,
            total_locked_and_expired_collateral,
            total_requests_submitted,
            total_requests_submitted_onchain,
            total_requests_submitted_offchain,
            total_requests_locked,
            total_requests_slashed,
            total_expired,
            total_locked_and_expired,
            total_locked_and_fulfilled,
            total_secondary_fulfillments,
            locked_orders_fulfillment_rate,
            total_program_cycles,
            total_cycles,
            best_peak_prove_mhz_prover,
            best_peak_prove_mhz_request_id,
            best_effective_prove_mhz_prover,
            best_effective_prove_mhz_request_id,
            best_peak_prove_mhz_v2,
            best_effective_prove_mhz_v2
        FROM all_time_requestor_summary
        {}
        {}
        LIMIT ${}",
        where_clause, order_clause, bind_count
    );

    let mut query = sqlx::query(&query_str);

    // Bind parameters in the same order as bind_count increments
    query = query.bind(format!("{:x}", requestor_address));

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
        .map(|row| parse_all_time_requestor_summary_row(&row))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(summaries)
}

// Leaderboard query implementation: aggregates stats for all requestors in a time period
// Sorted by orders_requested DESC
async fn get_requestor_leaderboard_impl(
    pool: &PgPool,
    start_ts: u64,
    end_ts: u64,
    use_hourly_table: bool,
    cursor_orders: Option<u64>,
    cursor_address: Option<Address>,
    limit: i64,
) -> Result<Vec<RequestorLeaderboardEntry>, DbError> {
    let table_name =
        if use_hourly_table { "hourly_requestor_summary" } else { "daily_requestor_summary" };

    // Build the query with cursor-based pagination on (orders_requested, address)
    // Sorted by orders_requested DESC, address DESC
    // Cast SUM results to BIGINT to match Rust i64
    let query_str = if cursor_orders.is_some() && cursor_address.is_some() {
        format!(
            "SELECT 
                requestor_address,
                SUM(total_requests_submitted)::BIGINT as orders_requested,
                SUM(total_requests_locked)::BIGINT as orders_locked,
                SUM(total_locked_and_fulfilled)::BIGINT as fulfilled,
                SUM(total_locked_and_expired)::BIGINT as expired,
                SUM(total_expired)::BIGINT as total_expired,
                LPAD(SUM(CAST(total_cycles AS NUMERIC))::TEXT, 78, '0') as cycles
            FROM {}
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            GROUP BY requestor_address
            HAVING (
                SUM(total_requests_submitted) < $3
                OR (SUM(total_requests_submitted) = $3 AND requestor_address < $4)
            )
            ORDER BY orders_requested DESC, requestor_address DESC
            LIMIT $5",
            table_name
        )
    } else {
        format!(
            "SELECT 
                requestor_address,
                SUM(total_requests_submitted)::BIGINT as orders_requested,
                SUM(total_requests_locked)::BIGINT as orders_locked,
                SUM(total_locked_and_fulfilled)::BIGINT as fulfilled,
                SUM(total_locked_and_expired)::BIGINT as expired,
                SUM(total_expired)::BIGINT as total_expired,
                LPAD(SUM(CAST(total_cycles AS NUMERIC))::TEXT, 78, '0') as cycles
            FROM {}
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            GROUP BY requestor_address
            ORDER BY orders_requested DESC, requestor_address DESC
            LIMIT $3",
            table_name
        )
    };

    let rows = if let (Some(cursor_o), Some(cursor_a)) = (cursor_orders, cursor_address) {
        sqlx::query(&query_str)
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .bind(cursor_o as i64)
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
        let requestor_address_str: String = row.try_get("requestor_address")?;
        let requestor_address = Address::from_str(&requestor_address_str)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid address: {}", e)))?;
        let orders_requested: i64 = row.try_get("orders_requested")?;
        let orders_locked: i64 = row.try_get("orders_locked")?;
        let fulfilled: i64 = row.try_get("fulfilled")?;
        let expired: i64 = row.try_get("expired")?;
        let total_expired_all: i64 = row.try_get("total_expired")?;
        let cycles_str: String = row.try_get("cycles")?;
        let cycles_requested = padded_string_to_u256(&cycles_str)?;

        let not_locked_and_expired = total_expired_all - expired;
        let acceptance_rate = if (orders_locked + not_locked_and_expired) > 0 {
            (orders_locked as f32 / (orders_locked + not_locked_and_expired) as f32) * 100.0
        } else {
            0.0
        };

        let total_outcomes = fulfilled + expired;
        let locked_order_fulfillment_rate = if total_outcomes > 0 {
            (fulfilled as f32 / total_outcomes as f32) * 100.0
        } else {
            0.0
        };

        let adjusted_fulfilled_query =
            "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.fulfilled_at >= $1
            AND rs.fulfilled_at < $2
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";
        let adjusted_fulfilled_row = sqlx::query(adjusted_fulfilled_query)
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_optional(pool)
            .await?;
        let adjusted_fulfilled: i64 =
            adjusted_fulfilled_row.and_then(|row| row.try_get("count").ok()).unwrap_or(0);

        let adjusted_expired_query = "SELECT COUNT(DISTINCT (pr.input_data, pr.image_url)) as count
            FROM request_status rs
            JOIN proof_requests pr ON rs.request_digest = pr.request_digest
            WHERE rs.expires_at >= $1
            AND rs.expires_at < $2
            AND rs.request_status = 'expired'
            AND rs.locked_at IS NOT NULL
            AND rs.client_address = $3";
        let adjusted_expired_row = sqlx::query(adjusted_expired_query)
            .bind(start_ts as i64)
            .bind(end_ts as i64)
            .bind(format!("{:x}", requestor_address))
            .fetch_optional(pool)
            .await?;
        let adjusted_expired: i64 =
            adjusted_expired_row.and_then(|row| row.try_get("count").ok()).unwrap_or(0);

        let total_outcomes_adjusted = adjusted_fulfilled + adjusted_expired;
        let locked_orders_fulfillment_rate_adjusted = if total_outcomes_adjusted > 0 {
            (adjusted_fulfilled as f32 / total_outcomes_adjusted as f32) * 100.0
        } else {
            0.0
        };

        results.push(RequestorLeaderboardEntry {
            requestor_address,
            orders_requested: orders_requested as u64,
            orders_locked: orders_locked as u64,
            cycles_requested,
            median_lock_price_per_cycle: None,
            acceptance_rate,
            locked_order_fulfillment_rate,
            locked_orders_fulfillment_rate_adjusted,
            last_activity_time: 0,
        });
    }

    Ok(results)
}

// Get median lock price per cycle for a list of requestors in a time period
async fn get_requestor_median_lock_prices_impl(
    pool: &PgPool,
    requestor_addresses: &[Address],
    start_ts: u64,
    end_ts: u64,
) -> Result<std::collections::HashMap<Address, U256>, DbError> {
    if requestor_addresses.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> =
        (3..=requestor_addresses.len() + 2).map(|i| format!("${}", i)).collect();
    let placeholders_str = placeholders.join(", ");

    let query_str = format!(
        "SELECT 
            client_address,
            LPAD(
                ROUND(PERCENTILE_CONT(0.5) WITHIN GROUP (ORDER BY CAST(lock_price_per_cycle AS NUMERIC)))::TEXT,
                78, '0'
            ) as median_price
        FROM request_status
        WHERE locked_at >= $1 AND locked_at < $2
          AND client_address IN ({})
          AND lock_price_per_cycle IS NOT NULL
        GROUP BY client_address",
        placeholders_str
    );

    let mut query = sqlx::query(&query_str).bind(start_ts as i64).bind(end_ts as i64);

    for addr in requestor_addresses {
        query = query.bind(format!("{:x}", addr));
    }

    let rows = query.fetch_all(pool).await?;

    let mut result = std::collections::HashMap::new();
    for row in rows {
        let addr_str: String = row.try_get("client_address")?;
        let median_str: String = row.try_get("median_price")?;
        if let Ok(addr) = Address::from_str(&addr_str) {
            if let Ok(median) = padded_string_to_u256(&median_str) {
                result.insert(addr, median);
            }
        }
    }

    Ok(result)
}

// Get last activity time for a list of requestors
async fn get_requestor_last_activity_times_impl(
    pool: &PgPool,
    requestor_addresses: &[Address],
) -> Result<std::collections::HashMap<Address, u64>, DbError> {
    if requestor_addresses.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    // Build placeholders for IN clause
    let placeholders: Vec<String> =
        (1..=requestor_addresses.len()).map(|i| format!("${}", i)).collect();
    let placeholders_str = placeholders.join(", ");

    let query_str = format!(
        "SELECT 
            client_address,
            MAX(updated_at) as last_activity
        FROM request_status
        WHERE client_address IN ({})
        GROUP BY client_address",
        placeholders_str
    );

    let mut query = sqlx::query(&query_str);
    for addr in requestor_addresses {
        query = query.bind(format!("{:x}", addr));
    }

    let rows = query.fetch_all(pool).await?;

    let mut result = std::collections::HashMap::new();
    for row in rows {
        let addr_str: String = row.try_get("client_address")?;
        let last_activity: i64 = row.try_get("last_activity")?;
        if let Ok(addr) = Address::from_str(&addr_str) {
            result.insert(addr, last_activity as u64);
        }
    }

    Ok(result)
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use super::*;
    use crate::db::events::EventsDb;
    use crate::db::market::{
        HourlyRequestorSummary, MarketDb, RequestStatus, RequestStatusType, SlashedStatus,
        TxMetadata,
    };
    use crate::test_utils::TestDb;
    use alloy::primitives::{Address, Bytes, B256, U256};
    use boundless_market::contracts::{
        Offer, Predicate, ProofRequest, RequestId, RequestInput, Requirements,
    };
    use risc0_zkvm::Digest;

    // Helper functions for test data generation
    fn generate_request(id: u32, addr: &Address) -> ProofRequest {
        generate_request_with_collateral(id, addr, U256::from(10))
    }

    fn generate_request_with_collateral(id: u32, addr: &Address, collateral: U256) -> ProofRequest {
        ProofRequest::new(
            RequestId::new(*addr, id),
            Requirements::new(Predicate::prefix_match(Digest::default(), Bytes::default())),
            "https://image_url.dev",
            RequestInput::builder().write_slice(&[0x41, 0x41, 0x41, 0x41]).build_inline().unwrap(),
            Offer {
                minPrice: U256::from(20000000000000u64),
                maxPrice: U256::from(40000000000000u64),
                rampUpStart: 0,
                timeout: 420,
                lockTimeout: 420,
                rampUpPeriod: 1,
                lockCollateral: collateral,
            },
        )
    }

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

    // Helper to setup period query test data
    async fn setup_period_requestor_test_data(db: &MarketDb, requestor: Address, base_ts: u64) {
        let submit_metadata =
            TxMetadata::new(B256::from([0x01; 32]), Address::ZERO, 100, base_ts + 100, 0);
        let lock_metadata =
            TxMetadata::new(B256::from([0x02; 32]), Address::ZERO, 101, base_ts + 150, 0);
        let fulfill_metadata =
            TxMetadata::new(B256::from([0x03; 32]), Address::ZERO, 102, base_ts + 200, 0);

        // Add proof requests
        for i in 0..5 {
            let collateral = U256::from(100 * (i + 1));
            let request = generate_request_with_collateral(i, &requestor, collateral);
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);
            db.add_proof_requests(&[(
                digest,
                request,
                submit_metadata,
                "onchain".to_string(),
                submit_metadata.block_timestamp,
            )])
            .await
            .unwrap();
        }

        // Add request statuses
        for i in 0..5 {
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);
            let status = RequestStatus {
                request_digest: digest,
                request_id: U256::from(i),
                request_status: if i < 3 {
                    RequestStatusType::Fulfilled
                } else {
                    RequestStatusType::Submitted
                },
                slashed_status: if i == 4 {
                    SlashedStatus::Slashed
                } else {
                    SlashedStatus::NotApplicable
                },
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: if i < 3 { Some(Address::from([0xAA; 20])) } else { None },
                created_at: base_ts + 100,
                updated_at: base_ts + 200,
                locked_at: Some(base_ts + 150),
                fulfilled_at: if i < 3 { Some(base_ts + 200) } else { None },
                slashed_at: if i == 4 { Some(base_ts + 300) } else { None },
                lock_prover_delivered_proof_at: if i < 3 { Some(base_ts + 180) } else { None },
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: if i < 3 { Some(102) } else { None },
                slashed_block: if i == 4 { Some(103) } else { None },
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: format!("{}", 100 * (i + 1)),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: if i == 3 { base_ts + 250 } else { base_ts + 10000 },
                lock_end: base_ts + 500,
                slash_recipient: if i == 4 { Some(Address::from([0xBB; 20])) } else { None },
                slash_transferred_amount: if i == 4 { Some("50".to_string()) } else { None },
                slash_burned_amount: if i == 4 { Some("50".to_string()) } else { None },
                program_cycles: if i < 3 { Some(U256::from(50_000_000 * (i + 1))) } else { None },
                total_cycles: if i < 3 { Some(U256::from(50_790_000 * (i + 1))) } else { None },
                peak_prove_mhz: if i < 3 { Some((1000 + (i * 100)) as f64) } else { None },
                effective_prove_mhz: if i < 3 { Some((900 + (i * 100)) as f64) } else { None },
                prover_effective_prove_mhz: if i < 3 {
                    Some((1100 + (i * 100)) as f64)
                } else {
                    None
                },
                cycle_status: if i < 3 { Some("resolved".to_string()) } else { None },
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: if i < 3 { Some(B256::from([0x03; 32])) } else { None },
                slash_tx_hash: if i == 4 { Some(B256::from([0x04; 32])) } else { None },
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

        // Add events
        for i in 0..5 {
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);

            db.add_request_submitted_events(&[(digest, U256::from(i), submit_metadata)])
                .await
                .unwrap();
            db.add_request_locked_events(&[(
                digest,
                U256::from(i),
                Address::from([0xAA; 20]),
                lock_metadata,
            )])
            .await
            .unwrap();

            if i < 3 {
                db.add_request_fulfilled_events(&[(
                    digest,
                    U256::from(i),
                    Address::from([0xAA; 20]),
                    fulfill_metadata,
                )])
                .await
                .unwrap();
            }

            if i == 4 {
                let slash_metadata =
                    TxMetadata::new(B256::from([0x04; 32]), Address::ZERO, 103, base_ts + 300, 0);
                db.add_prover_slashed_events(&[(
                    U256::from(i),
                    U256::from(50),
                    U256::from(50),
                    Address::from([0xBB; 20]),
                    slash_metadata,
                )])
                .await
                .unwrap();
            }
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_and_get_hourly_requestor_summary(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x42; 20]);
        let period_ts = 1700000000u64;

        let summary = HourlyRequestorSummary {
            period_timestamp: period_ts,
            requestor_address: requestor,
            total_fulfilled: 5,
            unique_provers_locking_requests: 2,
            total_fees_locked: U256::from(1000),
            total_collateral_locked: U256::from(2000),
            total_locked_and_expired_collateral: U256::ZERO,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 10,
            total_requests_submitted_onchain: 7,
            total_requests_submitted_offchain: 3,
            total_requests_locked: 8,
            total_requests_slashed: 1,
            total_expired: 2,
            total_locked_and_expired: 1,
            total_locked_and_fulfilled: 5,
            total_secondary_fulfillments: 2,
            locked_orders_fulfillment_rate: 0.625,
            locked_orders_fulfillment_rate_adjusted: 0.625,
            total_program_cycles: U256::from(50_000_000_000u64),
            total_cycles: U256::from(50_790_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_prover: Some("0x1234".to_string()),
            best_peak_prove_mhz_request_id: Some(U256::from(123)),
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_prover: Some("0x5678".to_string()),
            best_effective_prove_mhz_request_id: Some(U256::from(456)),
        };

        db.upsert_hourly_requestor_summary(summary.clone()).await.unwrap();

        let results = db
            .get_hourly_requestor_summaries_by_range(requestor, period_ts, period_ts + 1)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].period_timestamp, period_ts);
        assert_eq!(results[0].requestor_address, requestor);
        assert_eq!(results[0].total_fulfilled, 5);
        assert_eq!(results[0].total_program_cycles, U256::from(50_000_000_000u64));

        let mut updated = summary.clone();
        updated.total_fulfilled = 10;
        db.upsert_hourly_requestor_summary(updated).await.unwrap();

        let results = db
            .get_hourly_requestor_summaries_by_range(requestor, period_ts, period_ts + 1)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_all_time_requestor_summary(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x42; 20]);
        let period_ts = 1700000000u64;

        let summary = AllTimeRequestorSummary {
            period_timestamp: period_ts,
            requestor_address: requestor,
            total_fulfilled: 100,
            unique_provers_locking_requests: 10,
            total_fees_locked: U256::from(50000),
            total_collateral_locked: U256::from(100000),
            total_locked_and_expired_collateral: U256::from(5000),
            total_requests_submitted: 150,
            total_requests_submitted_onchain: 120,
            total_requests_submitted_offchain: 30,
            total_requests_locked: 110,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 100,
            total_secondary_fulfillments: 20,
            locked_orders_fulfillment_rate: 0.909,
            locked_orders_fulfillment_rate_adjusted: 0.909,
            total_program_cycles: U256::from(500_000_000_000u64),
            total_cycles: U256::from(507_900_000_000u64),
            best_peak_prove_mhz: 2000.0,
            best_peak_prove_mhz_prover: Some("0xaaa".to_string()),
            best_peak_prove_mhz_request_id: Some(U256::from(999)),
            best_effective_prove_mhz: 1900.0,
            best_effective_prove_mhz_prover: Some("0xbbb".to_string()),
            best_effective_prove_mhz_request_id: Some(U256::from(888)),
        };

        db.upsert_all_time_requestor_summary(summary.clone()).await.unwrap();

        let result = db.get_latest_all_time_requestor_summary(requestor).await.unwrap();
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.period_timestamp, period_ts);
        assert_eq!(result.requestor_address, requestor);
        assert_eq!(result.total_fulfilled, 100);
        assert_eq!(
            result.total_secondary_fulfillments, 20,
            "Should have 20 secondary fulfillments"
        );
        assert_eq!(result.total_program_cycles, U256::from(500_000_000_000u64));

        let mut updated = summary.clone();
        updated.total_fulfilled = 200;
        updated.total_secondary_fulfillments = 25;
        updated.period_timestamp = period_ts + 1;
        db.upsert_all_time_requestor_summary(updated).await.unwrap();

        let result = db.get_latest_all_time_requestor_summary(requestor).await.unwrap().unwrap();
        assert_eq!(result.total_fulfilled, 200);
        assert_eq!(
            result.total_secondary_fulfillments, 25,
            "Should have updated secondary fulfillments"
        );
        assert_eq!(result.period_timestamp, period_ts + 1);

        // Test ON CONFLICT update by upserting with same timestamp
        let mut conflict_update = summary.clone();
        conflict_update.total_secondary_fulfillments = 30;
        conflict_update.period_timestamp = period_ts + 1; // Same timestamp as previous update
        db.upsert_all_time_requestor_summary(conflict_update).await.unwrap();

        let result = db.get_latest_all_time_requestor_summary(requestor).await.unwrap().unwrap();
        assert_eq!(
            result.total_secondary_fulfillments, 30,
            "ON CONFLICT should update total_secondary_fulfillments"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_and_get_daily_requestor_summary(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x43; 20]);
        let base_ts = 1700000000u64;
        let day_seconds = 86400u64;

        for i in 0..3 {
            let summary = DailyRequestorSummary {
                period_timestamp: base_ts + (i * day_seconds),
                requestor_address: requestor,
                total_fulfilled: 10 * (i + 1),
                unique_provers_locking_requests: 2 * (i + 1),
                total_fees_locked: U256::from(1000 * (i + 1)),
                total_collateral_locked: U256::from(2000 * (i + 1)),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(10),
                p25_lock_price_per_cycle: U256::from(25),
                p50_lock_price_per_cycle: U256::from(50),
                p75_lock_price_per_cycle: U256::from(75),
                p90_lock_price_per_cycle: U256::from(90),
                p95_lock_price_per_cycle: U256::from(95),
                p99_lock_price_per_cycle: U256::from(99),
                total_requests_submitted: 20 * (i + 1),
                total_requests_submitted_onchain: 15 * (i + 1),
                total_requests_submitted_offchain: 5 * (i + 1),
                total_requests_locked: 12 * (i + 1),
                total_requests_slashed: i,
                total_expired: i,
                total_locked_and_expired: i,
                total_locked_and_fulfilled: 10 * (i + 1),
                total_secondary_fulfillments: 2 * (i + 1),
                locked_orders_fulfillment_rate: 0.8,
                locked_orders_fulfillment_rate_adjusted: 0.8,
                total_program_cycles: U256::from(100_000_000 * (i + 1)),
                total_cycles: U256::from(101_580_000 * (i + 1)),
                best_peak_prove_mhz: 1000.0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 900.0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_daily_requestor_summary(summary).await.unwrap();
        }

        let results = db
            .get_daily_requestor_summaries_by_range(requestor, base_ts, base_ts + (3 * day_seconds))
            .await
            .unwrap();
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].total_fulfilled, 10);
        assert_eq!(results[1].total_fulfilled, 20);
        assert_eq!(results[2].total_fulfilled, 30);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_and_get_weekly_requestor_summary(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x44; 20]);
        let base_ts = 1700000000u64;
        let week_seconds = 604800u64;

        let summary = WeeklyRequestorSummary {
            period_timestamp: base_ts,
            requestor_address: requestor,
            total_fulfilled: 50,
            unique_provers_locking_requests: 5,
            total_fees_locked: U256::from(10000),
            total_collateral_locked: U256::from(20000),
            total_locked_and_expired_collateral: U256::from(500),
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 100,
            total_requests_submitted_onchain: 80,
            total_requests_submitted_offchain: 20,
            total_requests_locked: 60,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 50,
            total_secondary_fulfillments: 10,
            locked_orders_fulfillment_rate: 0.833,
            locked_orders_fulfillment_rate_adjusted: 0.833,
            total_program_cycles: U256::from(1_000_000_000),
            total_cycles: U256::from(1_015_800_000),
            best_peak_prove_mhz: 1200.0,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1100.0,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        db.upsert_weekly_requestor_summary(summary.clone()).await.unwrap();

        let results = db
            .get_weekly_requestor_summaries_by_range(requestor, base_ts, base_ts + week_seconds)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 50);
        assert_eq!(results[0].requestor_address, requestor);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_upsert_and_get_monthly_requestor_summary(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x45; 20]);
        let base_ts = 1700000000u64;

        let summary = MonthlyRequestorSummary {
            period_timestamp: base_ts,
            requestor_address: requestor,
            total_fulfilled: 200,
            unique_provers_locking_requests: 15,
            total_fees_locked: U256::from(50000),
            total_collateral_locked: U256::from(100000),
            total_locked_and_expired_collateral: U256::from(5000),
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 300,
            total_requests_submitted_onchain: 250,
            total_requests_submitted_offchain: 50,
            total_requests_locked: 220,
            total_requests_slashed: 10,
            total_expired: 20,
            total_locked_and_expired: 15,
            total_locked_and_fulfilled: 200,
            total_secondary_fulfillments: 40,
            locked_orders_fulfillment_rate: 0.909,
            locked_orders_fulfillment_rate_adjusted: 0.909,
            total_program_cycles: U256::from(5_000_000_000u64),
            total_cycles: U256::from(5_079_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        db.upsert_monthly_requestor_summary(summary.clone()).await.unwrap();

        let results = db
            .get_monthly_requestor_summaries_by_range(requestor, base_ts, base_ts + 2_628_000)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].total_fulfilled, 200);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_all_time_requestor_summary_by_timestamp(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x46; 20]);
        let ts1 = 1700000000u64;
        let ts2 = 1700010000u64;

        let summary1 = AllTimeRequestorSummary {
            period_timestamp: ts1,
            requestor_address: requestor,
            total_fulfilled: 100,
            unique_provers_locking_requests: 10,
            total_fees_locked: U256::from(10000),
            total_collateral_locked: U256::from(20000),
            total_locked_and_expired_collateral: U256::ZERO,
            total_requests_submitted: 150,
            total_requests_submitted_onchain: 120,
            total_requests_submitted_offchain: 30,
            total_requests_locked: 110,
            total_requests_slashed: 5,
            total_expired: 10,
            total_locked_and_expired: 8,
            total_locked_and_fulfilled: 100,
            total_secondary_fulfillments: 20,
            locked_orders_fulfillment_rate: 0.909,
            locked_orders_fulfillment_rate_adjusted: 0.909,
            total_program_cycles: U256::from(1_000_000_000),
            total_cycles: U256::from(1_015_800_000),
            best_peak_prove_mhz: 1000.0,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 900.0,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        let summary2 = AllTimeRequestorSummary {
            period_timestamp: ts2,
            total_fulfilled: 200,
            ..summary1.clone()
        };

        db.upsert_all_time_requestor_summary(summary1).await.unwrap();
        db.upsert_all_time_requestor_summary(summary2).await.unwrap();

        let result = db.get_all_time_requestor_summary_by_timestamp(requestor, ts1).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().total_fulfilled, 100);

        let result = db.get_all_time_requestor_summary_by_timestamp(requestor, ts2).await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().total_fulfilled, 200);

        let result =
            db.get_all_time_requestor_summary_by_timestamp(requestor, 9999999).await.unwrap();
        assert!(result.is_none());
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_all_requestor_addresses(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let addr1 = Address::from([0x10; 20]);
        let addr2 = Address::from([0x20; 20]);
        let addr3 = Address::from([0x30; 20]);

        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);

        let request1 = generate_request(1, &addr1);
        let request2 = generate_request(2, &addr2);
        let request3 = generate_request(3, &addr3);
        let request4 = generate_request(4, &addr1);

        db.add_proof_requests(&[(
            B256::from([1; 32]),
            request1,
            metadata,
            "onchain".to_string(),
            metadata.block_timestamp,
        )])
        .await
        .unwrap();
        db.add_proof_requests(&[(
            B256::from([2; 32]),
            request2,
            metadata,
            "onchain".to_string(),
            metadata.block_timestamp,
        )])
        .await
        .unwrap();
        db.add_proof_requests(&[(
            B256::from([3; 32]),
            request3,
            metadata,
            "onchain".to_string(),
            metadata.block_timestamp,
        )])
        .await
        .unwrap();
        db.add_proof_requests(&[(
            B256::from([4; 32]),
            request4,
            metadata,
            "onchain".to_string(),
            metadata.block_timestamp,
        )])
        .await
        .unwrap();

        let addresses = db.get_all_requestor_addresses().await.unwrap();
        assert_eq!(addresses.len(), 3);
        assert!(addresses.contains(&addr1));
        assert!(addresses.contains(&addr2));
        assert!(addresses.contains(&addr3));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_active_requestor_addresses_in_period(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let addr1 = Address::from([0x11; 20]);
        let addr2 = Address::from([0x22; 20]);
        let addr3 = Address::from([0x33; 20]);

        let base_ts = 1700000000u64;

        let digest1 = B256::from([1; 32]);
        let digest2 = B256::from([2; 32]);
        let digest3 = B256::from([3; 32]);

        let request1 = generate_request(1, &addr1);
        let request2 = generate_request(2, &addr2);
        let request3 = generate_request(3, &addr3);

        let metadata1 = TxMetadata::new(B256::ZERO, addr1, 100, base_ts + 100, 0);
        let metadata2 = TxMetadata::new(B256::from([1; 32]), addr2, 101, base_ts + 500, 0);
        let metadata3 = TxMetadata::new(B256::from([2; 32]), addr3, 102, base_ts + 1500, 0);

        db.add_proof_requests(&[
            (digest1, request1, metadata1, "onchain".to_string(), base_ts + 100),
            (digest2, request2, metadata2, "onchain".to_string(), base_ts + 500),
            (digest3, request3, metadata3, "onchain".to_string(), base_ts + 1500),
        ])
        .await
        .unwrap();

        let addresses =
            db.get_active_requestor_addresses_in_period(base_ts, base_ts + 1000).await.unwrap();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&addr1));
        assert!(addresses.contains(&addr2));
        assert!(!addresses.contains(&addr3));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_active_requestor_addresses_in_period_all_activities(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        // Period boundaries: activities should occur between period_start and period_end
        let period_start = 1700000000u64;
        let period_end = period_start + 1000;
        let before_period = period_start - 1000; // Requests submitted before period
        let after_period = period_end + 1000; // Activities after period (should be excluded)

        // 1. Submissions only: Requestor with submissions in period
        let addr_submissions = Address::from([0xA1; 20]);
        let digest_submissions = B256::from([0xA1; 32]);
        let request_submissions = generate_request(1, &addr_submissions);
        let submit_meta = TxMetadata::new(
            B256::from([0xA1; 32]),
            Address::ZERO,
            100,
            period_start + 100, // During period
            0,
        );
        db.add_proof_requests(&[(
            digest_submissions,
            request_submissions,
            submit_meta,
            "onchain".to_string(),
            submit_meta.block_timestamp,
        )])
        .await
        .unwrap();

        // 2. Lock events only: Requestor who submitted before period but had requests locked during period
        let addr_locks = Address::from([0xA2; 20]);
        let digest_locks = B256::from([0xA2; 32]);
        let request_locks = generate_request(2, &addr_locks);
        let submit_meta_before = TxMetadata::new(
            B256::from([0xA2; 32]),
            Address::ZERO,
            101,
            before_period, // Before period
            0,
        );
        db.add_proof_requests(&[(
            digest_locks,
            request_locks.clone(),
            submit_meta_before,
            "onchain".to_string(),
            submit_meta_before.block_timestamp,
        )])
        .await
        .unwrap();

        // Add request status for lock event
        let status_locks = RequestStatus {
            request_digest: digest_locks,
            request_id: request_locks.id,
            request_status: RequestStatusType::Locked,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: addr_locks,
            lock_prover_address: Some(Address::from([0xBB; 20])),
            fulfill_prover_address: None,
            created_at: before_period,
            updated_at: period_start + 200,
            locked_at: Some(period_start + 200), // During period
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(101),
            lock_block: Some(102),
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: before_period,
            ramp_up_period: 10,
            expires_at: period_start + 10000,
            lock_end: period_start + 500,
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
            lock_price_per_cycle: Some("30".to_string()),
            submit_tx_hash: Some(B256::from([0xA2; 32])),
            lock_tx_hash: Some({
                let mut bytes = [0xA2; 32];
                bytes[1] = 0xA2;
                B256::from(bytes)
            }),
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
        db.upsert_request_statuses(&[status_locks]).await.unwrap();

        let lock_meta = TxMetadata::new(
            {
                let mut bytes = [0xA2; 32];
                bytes[1] = 0xA2;
                B256::from(bytes)
            },
            Address::ZERO,
            102,
            period_start + 200, // During period
            0,
        );
        db.add_request_locked_events(&[(
            digest_locks,
            request_locks.id,
            Address::from([0xBB; 20]),
            lock_meta,
        )])
        .await
        .unwrap();

        // 3. Fulfillment events only: Requestor who submitted before period but had requests fulfilled during period
        let addr_fulfilled = Address::from([0xA3; 20]);
        let digest_fulfilled = B256::from([0xA3; 32]);
        let request_fulfilled = generate_request(3, &addr_fulfilled);
        let submit_meta_before2 = TxMetadata::new(
            B256::from([0xA3; 32]),
            Address::ZERO,
            103,
            before_period, // Before period
            0,
        );
        db.add_proof_requests(&[(
            digest_fulfilled,
            request_fulfilled.clone(),
            submit_meta_before2,
            "onchain".to_string(),
            submit_meta_before2.block_timestamp,
        )])
        .await
        .unwrap();

        let status_fulfilled = RequestStatus {
            request_digest: digest_fulfilled,
            request_id: request_fulfilled.id,
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: addr_fulfilled,
            lock_prover_address: Some(Address::from([0xCC; 20])),
            fulfill_prover_address: Some(Address::from([0xCC; 20])),
            created_at: before_period,
            updated_at: period_start + 300,
            locked_at: Some(before_period + 50),
            fulfilled_at: Some(period_start + 300), // During period
            slashed_at: None,
            lock_prover_delivered_proof_at: Some(before_period + 100),
            submit_block: Some(103),
            lock_block: Some(104),
            fulfill_block: Some(105),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: before_period,
            ramp_up_period: 10,
            expires_at: period_start + 10000,
            lock_end: period_start + 500,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(50_000_000)),
            total_cycles: Some(U256::from(50_790_000)),
            peak_prove_mhz: Some(1000.0),
            effective_prove_mhz: Some(900.0),
            prover_effective_prove_mhz: Some(900.0),
            cycle_status: Some("resolved".to_string()),
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some("30".to_string()),
            submit_tx_hash: Some(B256::from([0xA3; 32])),
            lock_tx_hash: Some({
                let mut bytes = [0xA3; 32];
                bytes[1] = 0xA3;
                B256::from(bytes)
            }),
            fulfill_tx_hash: Some({
                let mut bytes = [0xA3; 32];
                bytes[1] = 0xA3;
                bytes[2] = 0xA3;
                B256::from(bytes)
            }),
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
        db.upsert_request_statuses(&[status_fulfilled]).await.unwrap();

        let fulfill_meta = TxMetadata::new(
            {
                let mut bytes = [0xA3; 32];
                bytes[1] = 0xA3;
                bytes[2] = 0xA3;
                B256::from(bytes)
            },
            Address::ZERO,
            105,
            period_start + 300, // During period
            0,
        );
        db.add_request_fulfilled_events(&[(
            digest_fulfilled,
            request_fulfilled.id,
            Address::from([0xCC; 20]),
            fulfill_meta,
        )])
        .await
        .unwrap();

        // 4. Slash events only: Requestor who submitted before period but had provers slashed during period
        let addr_slashed = Address::from([0xA4; 20]);
        let digest_slashed = B256::from([0xA4; 32]);
        let request_slashed = generate_request(4, &addr_slashed);
        let submit_meta_before3 = TxMetadata::new(
            B256::from([0xA4; 32]),
            Address::ZERO,
            106,
            before_period, // Before period
            0,
        );
        db.add_proof_requests(&[(
            digest_slashed,
            request_slashed.clone(),
            submit_meta_before3,
            "onchain".to_string(),
            submit_meta_before3.block_timestamp,
        )])
        .await
        .unwrap();

        let status_slashed = RequestStatus {
            request_digest: digest_slashed,
            request_id: request_slashed.id,
            request_status: RequestStatusType::Locked,
            slashed_status: SlashedStatus::Slashed,
            source: "onchain".to_string(),
            client_address: addr_slashed,
            lock_prover_address: Some(Address::from([0xDD; 20])),
            fulfill_prover_address: None,
            created_at: before_period,
            updated_at: period_start + 400,
            locked_at: Some(before_period + 50),
            fulfilled_at: None,
            slashed_at: Some(period_start + 400), // During period
            lock_prover_delivered_proof_at: None,
            submit_block: Some(106),
            lock_block: Some(107),
            fulfill_block: None,
            slashed_block: Some(108),
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: before_period,
            ramp_up_period: 10,
            expires_at: period_start + 10000,
            lock_end: period_start + 500,
            slash_recipient: Some(Address::from([0xEE; 20])),
            slash_transferred_amount: Some("50".to_string()),
            slash_burned_amount: Some("50".to_string()),
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some("30".to_string()),
            submit_tx_hash: Some(B256::from([0xA4; 32])),
            lock_tx_hash: Some({
                let mut bytes = [0xA4; 32];
                bytes[1] = 0xA4;
                B256::from(bytes)
            }),
            fulfill_tx_hash: None,
            slash_tx_hash: Some({
                let mut bytes = [0xA4; 32];
                bytes[1] = 0xA4;
                bytes[2] = 0xA4;
                B256::from(bytes)
            }),
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
        db.upsert_request_statuses(&[status_slashed]).await.unwrap();

        let slash_meta = TxMetadata::new(
            {
                let mut bytes = [0xA4; 32];
                bytes[1] = 0xA4;
                bytes[2] = 0xA4;
                B256::from(bytes)
            },
            Address::ZERO,
            108,
            period_start + 400, // During period
            0,
        );
        db.add_prover_slashed_events(&[(
            request_slashed.id,
            U256::from(50),
            U256::from(50),
            Address::from([0xEE; 20]),
            slash_meta,
        )])
        .await
        .unwrap();

        // 5. Expiration only: Requestor who submitted before period but had requests expire during period
        let addr_expired = Address::from([0xA5; 20]);
        let digest_expired = B256::from([0xA5; 32]);
        let request_expired = generate_request(5, &addr_expired);
        let submit_meta_before4 = TxMetadata::new(
            B256::from([0xA5; 32]),
            Address::ZERO,
            109,
            before_period, // Before period
            0,
        );
        db.add_proof_requests(&[(
            digest_expired,
            request_expired.clone(),
            submit_meta_before4,
            "onchain".to_string(),
            submit_meta_before4.block_timestamp,
        )])
        .await
        .unwrap();

        let status_expired = RequestStatus {
            request_digest: digest_expired,
            request_id: request_expired.id,
            request_status: RequestStatusType::Expired,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: addr_expired,
            lock_prover_address: None,
            fulfill_prover_address: None,
            created_at: before_period,
            updated_at: period_start + 500,
            locked_at: None,
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(109),
            lock_block: None,
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: before_period,
            ramp_up_period: 10,
            expires_at: period_start + 500, // During period
            lock_end: period_start + 500,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::from([0xA5; 32])),
            lock_tx_hash: None,
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
        db.upsert_request_statuses(&[status_expired]).await.unwrap();

        // 6. Multiple activities: Requestor with multiple activity types in period
        let addr_multiple = Address::from([0xA6; 20]);
        let digest_multiple1 = {
            let mut bytes = [0xA6; 32];
            bytes[1] = 0x01;
            B256::from(bytes)
        };
        let digest_multiple2 = {
            let mut bytes = [0xA6; 32];
            bytes[1] = 0x02;
            B256::from(bytes)
        };
        let request_multiple1 = generate_request(6, &addr_multiple);
        let request_multiple2 = generate_request(7, &addr_multiple);
        let submit_meta_multiple = TxMetadata::new(
            B256::from([0xA6; 32]),
            Address::ZERO,
            110,
            period_start + 100, // During period - submission
            0,
        );
        db.add_proof_requests(&[
            (
                digest_multiple1,
                request_multiple1.clone(),
                submit_meta_multiple,
                "onchain".to_string(),
                submit_meta_multiple.block_timestamp,
            ),
            (
                digest_multiple2,
                request_multiple2.clone(),
                submit_meta_multiple,
                "onchain".to_string(),
                submit_meta_multiple.block_timestamp,
            ),
        ])
        .await
        .unwrap();

        // Add lock event for first request
        let status_multiple1 = RequestStatus {
            request_digest: digest_multiple1,
            request_id: request_multiple1.id,
            request_status: RequestStatusType::Locked,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: addr_multiple,
            lock_prover_address: Some(Address::from([0xFF; 20])),
            fulfill_prover_address: None,
            created_at: period_start + 100,
            updated_at: period_start + 200,
            locked_at: Some(period_start + 200), // During period
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(110),
            lock_block: Some(111),
            fulfill_block: None,
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: period_start + 100,
            ramp_up_period: 10,
            expires_at: period_start + 10000,
            lock_end: period_start + 500,
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
            lock_price_per_cycle: Some("30".to_string()),
            submit_tx_hash: Some(B256::from([0xA6; 32])),
            lock_tx_hash: Some({
                let mut bytes = [0xA6; 32];
                bytes[1] = 0xA6;
                B256::from(bytes)
            }),
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
        db.upsert_request_statuses(&[status_multiple1]).await.unwrap();

        let lock_meta_multiple = TxMetadata::new(
            {
                let mut bytes = [0xA6; 32];
                bytes[1] = 0xA6;
                B256::from(bytes)
            },
            Address::ZERO,
            111,
            period_start + 200, // During period
            0,
        );
        db.add_request_locked_events(&[(
            digest_multiple1,
            request_multiple1.id,
            Address::from([0xFF; 20]),
            lock_meta_multiple,
        )])
        .await
        .unwrap();

        // Add fulfillment event for second request
        let status_multiple2 = RequestStatus {
            request_digest: digest_multiple2,
            request_id: request_multiple2.id,
            request_status: RequestStatusType::Fulfilled,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: addr_multiple,
            lock_prover_address: Some(Address::from([0xFF; 20])),
            fulfill_prover_address: Some(Address::from([0xFF; 20])),
            created_at: period_start + 100,
            updated_at: period_start + 300,
            locked_at: Some(period_start + 150),
            fulfilled_at: Some(period_start + 300), // During period
            slashed_at: None,
            lock_prover_delivered_proof_at: Some(period_start + 200),
            submit_block: Some(110),
            lock_block: Some(111),
            fulfill_block: Some(112),
            slashed_block: None,
            min_price: "1000".to_string(),
            max_price: "2000".to_string(),
            lock_collateral: "100".to_string(),
            ramp_up_start: period_start + 100,
            ramp_up_period: 10,
            expires_at: period_start + 10000,
            lock_end: period_start + 500,
            slash_recipient: None,
            slash_transferred_amount: None,
            slash_burned_amount: None,
            program_cycles: Some(U256::from(50_000_000)),
            total_cycles: Some(U256::from(50_790_000)),
            peak_prove_mhz: Some(1000.0),
            effective_prove_mhz: Some(900.0),
            prover_effective_prove_mhz: Some(900.0),
            cycle_status: Some("resolved".to_string()),
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some("30".to_string()),
            submit_tx_hash: Some(B256::from([0xA6; 32])),
            lock_tx_hash: Some({
                let mut bytes = [0xA6; 32];
                bytes[1] = 0xA6;
                B256::from(bytes)
            }),
            fulfill_tx_hash: Some({
                let mut bytes = [0xA6; 32];
                bytes[1] = 0xA6;
                bytes[2] = 0xA6;
                B256::from(bytes)
            }),
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
        db.upsert_request_statuses(&[status_multiple2]).await.unwrap();

        let fulfill_meta_multiple = TxMetadata::new(
            {
                let mut bytes = [0xA6; 32];
                bytes[1] = 0xA6;
                bytes[2] = 0xA6;
                B256::from(bytes)
            },
            Address::ZERO,
            112,
            period_start + 300, // During period
            0,
        );
        db.add_request_fulfilled_events(&[(
            digest_multiple2,
            request_multiple2.id,
            Address::from([0xFF; 20]),
            fulfill_meta_multiple,
        )])
        .await
        .unwrap();

        // 7. Activities outside period: Requestor with activities outside the period (should NOT be included)
        let addr_outside = Address::from([0xA7; 20]);
        let digest_outside = B256::from([0xA7; 32]);
        let request_outside = generate_request(8, &addr_outside);
        let submit_meta_outside = TxMetadata::new(
            B256::from([0xA7; 32]),
            Address::ZERO,
            113,
            after_period, // After period
            0,
        );
        db.add_proof_requests(&[(
            digest_outside,
            request_outside,
            submit_meta_outside,
            "onchain".to_string(),
            submit_meta_outside.block_timestamp,
        )])
        .await
        .unwrap();

        // 8. No activity: Requestor with no activity in period (should NOT be included)
        let addr_no_activity = Address::from([0xA8; 20]);
        let digest_no_activity = B256::from([0xA8; 32]);
        let request_no_activity = generate_request(9, &addr_no_activity);
        let submit_meta_no_activity = TxMetadata::new(
            B256::from([0xA8; 32]),
            Address::ZERO,
            114,
            before_period - 1000, // Well before period
            0,
        );
        db.add_proof_requests(&[(
            digest_no_activity,
            request_no_activity,
            submit_meta_no_activity,
            "onchain".to_string(),
            submit_meta_no_activity.block_timestamp,
        )])
        .await
        .unwrap();

        // Query active requestors in period
        let addresses =
            db.get_active_requestor_addresses_in_period(period_start, period_end).await.unwrap();

        // Verify expected requestors are included
        assert_eq!(addresses.len(), 6, "Should have 6 active requestors");
        assert!(addresses.contains(&addr_submissions), "Should include requestor with submissions");
        assert!(addresses.contains(&addr_locks), "Should include requestor with lock events");
        assert!(
            addresses.contains(&addr_fulfilled),
            "Should include requestor with fulfillment events"
        );
        assert!(addresses.contains(&addr_slashed), "Should include requestor with slash events");
        assert!(addresses.contains(&addr_expired), "Should include requestor with expiration");
        assert!(
            addresses.contains(&addr_multiple),
            "Should include requestor with multiple activities"
        );

        // Verify requestors with activities outside period are NOT included
        assert!(
            !addresses.contains(&addr_outside),
            "Should NOT include requestor with activities outside period"
        );
        assert!(
            !addresses.contains(&addr_no_activity),
            "Should NOT include requestor with no activity in period"
        );
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_list_requests_by_requestor(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let addr1 = Address::from([0x12; 20]);
        let addr2 = Address::from([0x34; 20]);
        let base_ts = 1700000000u64;

        for i in 0..5 {
            let status = RequestStatus {
                request_digest: B256::from([i as u8; 32]),
                request_id: U256::from(i),
                request_status: RequestStatusType::Submitted,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: addr1,
                lock_prover_address: None,
                fulfill_prover_address: None,
                created_at: base_ts + (i as u64 * 100),
                updated_at: base_ts + (i as u64 * 100),
                locked_at: None,
                fulfilled_at: None,
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: None,
                fulfill_block: None,
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
                program_cycles: None,
                total_cycles: None,
                peak_prove_mhz: None,
                effective_prove_mhz: None,
                prover_effective_prove_mhz: None,
                cycle_status: None,
                lock_price: None,
                lock_price_per_cycle: None,
                submit_tx_hash: Some(B256::ZERO),
                lock_tx_hash: None,
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

        let status_addr2 = RequestStatus {
            request_digest: B256::from([99; 32]),
            request_id: U256::from(99),
            client_address: addr2,
            created_at: base_ts,
            updated_at: base_ts,
            request_status: RequestStatusType::Submitted,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            lock_prover_address: None,
            fulfill_prover_address: None,
            locked_at: None,
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: None,
            fulfill_block: None,
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
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
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
        db.upsert_request_statuses(&[status_addr2]).await.unwrap();

        let (results, _cursor) = db
            .list_requests_by_requestor(addr1, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 5);
        assert!(results.iter().all(|r| r.client_address == addr1));

        let (results, cursor) = db
            .list_requests_by_requestor(addr1, None, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert!(cursor.is_some());

        let (results2, _) = db
            .list_requests_by_requestor(addr1, cursor, 2, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        assert_ne!(results[0].request_id, results2[0].request_id);

        let (results, _) = db
            .list_requests_by_requestor(addr2, None, 10, RequestSortField::CreatedAt)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].client_address, addr2);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_fulfilled_count(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x50; 20]);
        let base_ts = 1700000000u64;

        for i in 0..3 {
            let digest = B256::from([(i + 100) as u8; 32]);
            let request = generate_request(i, &requestor);

            let submit_meta = TxMetadata::new(
                B256::from([i as u8; 32]),
                Address::ZERO,
                100 + (i as u64),
                base_ts + 100,
                0,
            );
            db.add_proof_requests(&[(
                digest,
                request.clone(),
                submit_meta,
                "onchain".to_string(),
                submit_meta.block_timestamp,
            )])
            .await
            .unwrap();

            let fulfill_meta = TxMetadata::new(
                B256::from([(i + 50) as u8; 32]),
                Address::ZERO,
                102 + (i as u64),
                base_ts + 200,
                0,
            );
            db.add_request_fulfilled_events(&[(
                digest,
                request.id,
                Address::from([0xAA; 20]),
                fulfill_meta,
            )])
            .await
            .unwrap();

            let status = RequestStatus {
                request_digest: digest,
                request_id: request.id,
                request_status: RequestStatusType::Fulfilled,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: Some(Address::from([0xAA; 20])),
                created_at: base_ts + 100,
                updated_at: base_ts + 200,
                locked_at: Some(base_ts + 150),
                fulfilled_at: Some(base_ts + 200),
                slashed_at: None,
                lock_prover_delivered_proof_at: Some(base_ts + 180),
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: Some(102),
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "100".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 500,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: Some(U256::from(50_000_000)),
                total_cycles: Some(U256::from(50_790_000)),
                peak_prove_mhz: Some(1000.0),
                effective_prove_mhz: Some(900.0),
                prover_effective_prove_mhz: Some(900.0),
                cycle_status: Some("resolved".to_string()),
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: Some(B256::from([0x03; 32])),
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

        let count = db
            .get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_unique_provers(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x51; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_unique_provers(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_requests_submitted(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x52; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_total_requests_submitted(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 5);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_requests_submitted_onchain(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x53; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_total_requests_submitted_onchain(
                base_ts,
                base_ts + 1000,
                requestor,
            )
            .await
            .unwrap();
        assert_eq!(count, 5);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_requests_locked(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x54; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_total_requests_locked(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 5);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_requests_slashed(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x55; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_total_requests_slashed(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_locked_and_fulfilled_count_adjusted(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5D; 20]);
        let base_ts = 1700000000u64;

        let submit_metadata =
            TxMetadata::new(B256::from([0x01; 32]), Address::ZERO, 100, base_ts + 100, 0);
        let lock_metadata =
            TxMetadata::new(B256::from([0x02; 32]), Address::ZERO, 101, base_ts + 150, 0);
        let fulfill_metadata =
            TxMetadata::new(B256::from([0x03; 32]), Address::ZERO, 102, base_ts + 200, 0);

        let shared_input_data = "0x41414141";
        let shared_image_url = "https://shared_image_url.dev";

        for i in 0..5 {
            let collateral = U256::from(100 * (i + 1));
            let mut request = generate_request_with_collateral(i, &requestor, collateral);
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);

            if i < 3 {
                request = ProofRequest::new(
                    RequestId::new(requestor, i),
                    Requirements::new(Predicate::prefix_match(Digest::default(), Bytes::default())),
                    shared_image_url.to_string(),
                    RequestInput::builder()
                        .write_slice(&[0x41, 0x41, 0x41, 0x41])
                        .build_inline()
                        .unwrap(),
                    request.offer,
                );
            }

            db.add_proof_requests(&[(
                digest,
                request,
                submit_metadata,
                "onchain".to_string(),
                submit_metadata.block_timestamp,
            )])
            .await
            .unwrap();

            let status = RequestStatus {
                request_digest: digest,
                request_id: U256::from(i),
                request_status: if i < 3 {
                    RequestStatusType::Fulfilled
                } else {
                    RequestStatusType::Submitted
                },
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: if i < 3 { Some(Address::from([0xAA; 20])) } else { None },
                created_at: base_ts + 100,
                updated_at: base_ts + 200,
                locked_at: Some(base_ts + 150),
                fulfilled_at: if i < 3 { Some(base_ts + 200) } else { None },
                slashed_at: None,
                lock_prover_delivered_proof_at: if i < 3 { Some(base_ts + 180) } else { None },
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: if i < 3 { Some(102) } else { None },
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: format!("{}", 100 * (i + 1)),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 10000,
                lock_end: base_ts + 500,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: if i < 3 { Some(U256::from(50_000_000)) } else { None },
                total_cycles: if i < 3 { Some(U256::from(50_790_000)) } else { None },
                peak_prove_mhz: if i < 3 { Some(1000.0) } else { None },
                effective_prove_mhz: if i < 3 { Some(900.0) } else { None },
                prover_effective_prove_mhz: if i < 3 { Some(900.0) } else { None },
                cycle_status: if i < 3 { Some("resolved".to_string()) } else { None },
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: if i < 3 { Some(B256::from([0x03; 32])) } else { None },
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: if i < 3 { Some(shared_image_url.to_string()) } else { None },
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: if i < 3 { shared_input_data.to_string() } else { "0x00".to_string() },
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();

            db.add_request_submitted_events(&[(digest, U256::from(i), submit_metadata)])
                .await
                .unwrap();
            db.add_request_locked_events(&[(
                digest,
                U256::from(i),
                Address::from([0xAA; 20]),
                lock_metadata,
            )])
            .await
            .unwrap();

            if i < 3 {
                db.add_request_fulfilled_events(&[(
                    digest,
                    U256::from(i),
                    Address::from([0xAA; 20]),
                    fulfill_metadata,
                )])
                .await
                .unwrap();
            }
        }

        let regular_count = db
            .get_period_requestor_locked_and_fulfilled_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(regular_count, 3);

        let adjusted_count = db
            .get_period_requestor_locked_and_fulfilled_count_adjusted(
                base_ts,
                base_ts + 1000,
                requestor,
            )
            .await
            .unwrap();
        assert_eq!(adjusted_count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_locked_and_expired_count_adjusted(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5E; 20]);
        let base_ts = 1700000000u64;

        let submit_metadata =
            TxMetadata::new(B256::from([0x01; 32]), Address::ZERO, 100, base_ts + 100, 0);
        let lock_metadata =
            TxMetadata::new(B256::from([0x02; 32]), Address::ZERO, 101, base_ts + 150, 0);

        let shared_input_data = "0x42424242";
        let shared_image_url = "https://shared_expired_image_url.dev";

        for i in 0..3 {
            let collateral = U256::from(100 * (i + 1));
            let request = ProofRequest::new(
                RequestId::new(requestor, i),
                Requirements::new(Predicate::prefix_match(Digest::default(), Bytes::default())),
                shared_image_url.to_string(),
                RequestInput::builder()
                    .write_slice(&[0x42, 0x42, 0x42, 0x42])
                    .build_inline()
                    .unwrap(),
                Offer {
                    minPrice: U256::from(20000000000000u64),
                    maxPrice: U256::from(40000000000000u64),
                    rampUpStart: 0,
                    timeout: 420,
                    lockTimeout: 420,
                    rampUpPeriod: 1,
                    lockCollateral: collateral,
                },
            );
            let mut digest_bytes = [i as u8; 32];
            digest_bytes[0] = requestor.0[0];
            let digest = B256::from(digest_bytes);

            db.add_proof_requests(&[(
                digest,
                request,
                submit_metadata,
                "onchain".to_string(),
                submit_metadata.block_timestamp,
            )])
            .await
            .unwrap();

            let status = RequestStatus {
                request_digest: digest,
                request_id: U256::from(i),
                request_status: RequestStatusType::Expired,
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: None,
                created_at: base_ts + 100,
                updated_at: base_ts + 250,
                locked_at: Some(base_ts + 150),
                fulfilled_at: None,
                slashed_at: None,
                lock_prover_delivered_proof_at: None,
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: None,
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: format!("{}", 100 * (i + 1)),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at: base_ts + 250,
                lock_end: base_ts + 500,
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
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: None,
                slash_tx_hash: None,
                image_id: "test".to_string(),
                image_url: Some(shared_image_url.to_string()),
                selector: "test".to_string(),
                predicate_type: "digest_match".to_string(),
                predicate_data: "0x00".to_string(),
                input_type: "inline".to_string(),
                input_data: shared_input_data.to_string(),
                fulfill_journal: None,
                fulfill_seal: None,
            };
            db.upsert_request_statuses(&[status]).await.unwrap();

            db.add_request_submitted_events(&[(digest, U256::from(i), submit_metadata)])
                .await
                .unwrap();
            db.add_request_locked_events(&[(
                digest,
                U256::from(i),
                Address::from([0xAA; 20]),
                lock_metadata,
            )])
            .await
            .unwrap();
        }

        let regular_count = db
            .get_period_requestor_locked_and_expired_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(regular_count, 3);

        let adjusted_count = db
            .get_period_requestor_locked_and_expired_count_adjusted(
                base_ts,
                base_ts + 1000,
                requestor,
            )
            .await
            .unwrap();
        assert_eq!(adjusted_count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_lock_pricing_data(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x56; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let data = db
            .get_period_requestor_lock_pricing_data(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(data.len(), 3);

        for item in &data {
            assert_eq!(item.min_price, "1000");
            assert_eq!(item.max_price, "2000");
        }
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_all_lock_collateral(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x57; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let collaterals = db
            .get_period_requestor_all_lock_collateral(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(collaterals.len(), 5);

        let total: u64 = collaterals.iter().map(|c| c.parse::<u64>().unwrap()).sum();
        assert_eq!(total, 100 + 200 + 300 + 400 + 500);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_locked_and_expired_collateral(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x58; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let collaterals = db
            .get_period_requestor_locked_and_expired_collateral(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert!(collaterals.len() <= 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_expired_count(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x59; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_expired_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert!(count <= 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_locked_and_expired_count(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5A; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_locked_and_expired_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert!(count <= 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_locked_and_fulfilled_count(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5B; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count = db
            .get_period_requestor_locked_and_fulfilled_count(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(count, 3);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_program_cycles(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5C; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let total = db
            .get_period_requestor_total_program_cycles(base_ts, base_ts + 1000, requestor)
            .await
            .unwrap();
        assert_eq!(total, U256::from(50_000_000 + 100_000_000 + 150_000_000));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_total_cycles(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5D; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let total =
            db.get_period_requestor_total_cycles(base_ts, base_ts + 1000, requestor).await.unwrap();
        assert_eq!(total, U256::from(50_790_000 + 101_580_000 + 152_370_000));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_period_requestor_secondary_fulfillments_count(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5F; 20]);
        let base_ts = 1700000000u64;

        // Create test data with different fulfillment scenarios:
        // - Request 0: fulfilled BEFORE lock_end (not secondary)
        // - Request 1: fulfilled AFTER lock_end but BEFORE expires_at (secondary)
        // - Request 2: fulfilled AFTER lock_end but BEFORE expires_at (secondary)
        // - Request 3: fulfilled AFTER expires_at (not secondary, edge case)
        // - Request 4: not fulfilled (not secondary)

        let lock_end = base_ts + 500;
        let expires_at = base_ts + 1000;

        for i in 0..5 {
            let digest = B256::from([(i + 200) as u8; 32]);
            let request = generate_request(i, &requestor);

            let submit_meta = TxMetadata::new(
                B256::from([i as u8; 32]),
                Address::ZERO,
                100 + (i as u64),
                base_ts + 100,
                0,
            );
            db.add_proof_requests(&[(
                digest,
                request.clone(),
                submit_meta,
                "onchain".to_string(),
                submit_meta.block_timestamp,
            )])
            .await
            .unwrap();

            // Determine fulfillment timestamp based on scenario
            let fulfilled_at = match i {
                0 => Some(base_ts + 400),  // Before lock_end (not secondary)
                1 => Some(base_ts + 600),  // After lock_end, before expires_at (secondary)
                2 => Some(base_ts + 700),  // After lock_end, before expires_at (secondary)
                3 => Some(base_ts + 1100), // After expires_at (not secondary)
                4 => None,                 // Not fulfilled
                _ => unreachable!(),
            };

            if fulfilled_at.is_some() {
                let fulfill_meta = TxMetadata::new(
                    B256::from([(i + 50) as u8; 32]),
                    Address::ZERO,
                    102 + (i as u64),
                    fulfilled_at.unwrap(),
                    0,
                );
                db.add_request_fulfilled_events(&[(
                    digest,
                    request.id,
                    Address::from([0xAA; 20]),
                    fulfill_meta,
                )])
                .await
                .unwrap();
            }

            let status = RequestStatus {
                request_digest: digest,
                request_id: request.id,
                request_status: if fulfilled_at.is_some() {
                    RequestStatusType::Fulfilled
                } else {
                    RequestStatusType::Submitted
                },
                slashed_status: SlashedStatus::NotApplicable,
                source: "onchain".to_string(),
                client_address: requestor,
                lock_prover_address: Some(Address::from([0xAA; 20])),
                fulfill_prover_address: if fulfilled_at.is_some() {
                    Some(Address::from([0xAA; 20]))
                } else {
                    None
                },
                created_at: base_ts + 100,
                updated_at: fulfilled_at.unwrap_or(base_ts + 200),
                locked_at: Some(base_ts + 150),
                fulfilled_at,
                slashed_at: None,
                lock_prover_delivered_proof_at: if fulfilled_at.is_some() {
                    Some(fulfilled_at.unwrap() - 20)
                } else {
                    None
                },
                submit_block: Some(100),
                lock_block: Some(101),
                fulfill_block: if fulfilled_at.is_some() { Some(102) } else { None },
                slashed_block: None,
                min_price: "1000".to_string(),
                max_price: "2000".to_string(),
                lock_collateral: "100".to_string(),
                ramp_up_start: base_ts,
                ramp_up_period: 10,
                expires_at,
                lock_end,
                slash_recipient: None,
                slash_transferred_amount: None,
                slash_burned_amount: None,
                program_cycles: if fulfilled_at.is_some() {
                    Some(U256::from(50_000_000))
                } else {
                    None
                },
                total_cycles: if fulfilled_at.is_some() {
                    Some(U256::from(50_790_000))
                } else {
                    None
                },
                peak_prove_mhz: if fulfilled_at.is_some() { Some(1000.0) } else { None },
                effective_prove_mhz: if fulfilled_at.is_some() { Some(900.0) } else { None },
                prover_effective_prove_mhz: if fulfilled_at.is_some() { Some(900.0) } else { None },
                cycle_status: if fulfilled_at.is_some() {
                    Some("resolved".to_string())
                } else {
                    None
                },
                lock_price: Some("1500".to_string()),
                lock_price_per_cycle: Some("30".to_string()),
                submit_tx_hash: Some(B256::from([0x01; 32])),
                lock_tx_hash: Some(B256::from([0x02; 32])),
                fulfill_tx_hash: if fulfilled_at.is_some() {
                    Some(B256::from([0x03; 32]))
                } else {
                    None
                },
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

        // Test period that includes all fulfillments
        let count = db
            .get_period_requestor_secondary_fulfillments_count(base_ts, base_ts + 2000, requestor)
            .await
            .unwrap();
        // Should count requests 1 and 2 (fulfilled after lock_end but before expires_at)
        assert_eq!(count, 2, "Should count 2 secondary fulfillments (requests 1 and 2)");

        // Test period that only includes one secondary fulfillment
        let count_partial = db
            .get_period_requestor_secondary_fulfillments_count(
                base_ts + 550,
                base_ts + 650,
                requestor,
            )
            .await
            .unwrap();
        assert_eq!(
            count_partial, 1,
            "Should count 1 secondary fulfillment in partial period (request 1)"
        );

        // Test period that includes no secondary fulfillments
        let count_none = db
            .get_period_requestor_secondary_fulfillments_count(base_ts, base_ts + 450, requestor)
            .await
            .unwrap();
        assert_eq!(count_none, 0, "Should count 0 secondary fulfillments before lock_end");
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_all_time_requestor_unique_provers(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x5E; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor, base_ts).await;

        let count =
            db.get_all_time_requestor_unique_provers(base_ts + 10000, requestor).await.unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_requestor_methods_with_multiple_requestors(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor1 = Address::from([0x60; 20]);
        let requestor2 = Address::from([0x61; 20]);
        let base_ts = 1700000000u64;

        setup_period_requestor_test_data(db, requestor1, base_ts).await;
        setup_period_requestor_test_data(db, requestor2, base_ts + 10).await;

        let count1 = db
            .get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor1)
            .await
            .unwrap();
        let count2 = db
            .get_period_requestor_fulfilled_count(base_ts, base_ts + 1000, requestor2)
            .await
            .unwrap();

        assert_eq!(count1, 3);
        assert_eq!(count2, 3);

        let all_addresses = db.get_all_requestor_addresses().await.unwrap();
        assert!(all_addresses.contains(&requestor1));
        assert!(all_addresses.contains(&requestor2));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_requestor_summaries_with_empty_results(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x62; 20]);
        let base_ts = 1700000000u64;

        let results = db
            .get_hourly_requestor_summaries_by_range(requestor, base_ts, base_ts + 3600)
            .await
            .unwrap();
        assert_eq!(results.len(), 0);

        let latest = db.get_latest_all_time_requestor_summary(requestor).await.unwrap();
        assert!(latest.is_none());

        let by_ts =
            db.get_all_time_requestor_summary_by_timestamp(requestor, base_ts).await.unwrap();
        assert!(by_ts.is_none());
    }

    // ==================== Cursor-based Pagination Tests ====================

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_hourly_requestor_summaries_with_cursor(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x70; 20]);
        let base_ts = 1700000000u64;
        let hour_seconds = 3600u64;

        // Insert 5 hourly summaries
        for i in 0..5 {
            let summary = HourlyRequestorSummary {
                period_timestamp: base_ts + (i * hour_seconds),
                requestor_address: requestor,
                total_fulfilled: 10 * (i + 1),
                unique_provers_locking_requests: 1,
                total_fees_locked: U256::from(1000),
                total_collateral_locked: U256::from(2000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(10),
                p25_lock_price_per_cycle: U256::from(25),
                p50_lock_price_per_cycle: U256::from(50),
                p75_lock_price_per_cycle: U256::from(75),
                p90_lock_price_per_cycle: U256::from(90),
                p95_lock_price_per_cycle: U256::from(95),
                p99_lock_price_per_cycle: U256::from(99),
                total_requests_submitted: 10,
                total_requests_submitted_onchain: 7,
                total_requests_submitted_offchain: 3,
                total_requests_locked: 8,
                total_requests_slashed: 0,
                total_expired: 0,
                total_locked_and_expired: 0,
                total_locked_and_fulfilled: 5,
                total_secondary_fulfillments: 2,
                locked_orders_fulfillment_rate: 0.5,
                locked_orders_fulfillment_rate_adjusted: 0.5,
                total_program_cycles: U256::from(50_000_000),
                total_cycles: U256::from(50_790_000),
                best_peak_prove_mhz: 1000.0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 900.0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_hourly_requestor_summary(summary).await.unwrap();
        }

        // Test basic pagination with limit - DESC order (newest first)
        let results = db
            .get_hourly_requestor_summaries(requestor, None, 2, SortDirection::Desc, None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].period_timestamp, base_ts + (4 * hour_seconds)); // Newest
        assert_eq!(results[1].period_timestamp, base_ts + (3 * hour_seconds));

        // Test cursor pagination - get next page using last timestamp as cursor
        let cursor = results[1].period_timestamp as i64;
        let results2 = db
            .get_hourly_requestor_summaries(
                requestor,
                Some(cursor),
                2,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        assert_eq!(results2[0].period_timestamp, base_ts + (2 * hour_seconds));
        assert_eq!(results2[1].period_timestamp, base_ts + hour_seconds);

        // Test ASC order (oldest first)
        let results_asc = db
            .get_hourly_requestor_summaries(requestor, None, 2, SortDirection::Asc, None, None)
            .await
            .unwrap();
        assert_eq!(results_asc.len(), 2);
        assert_eq!(results_asc[0].period_timestamp, base_ts); // Oldest
        assert_eq!(results_asc[1].period_timestamp, base_ts + hour_seconds);

        // Test 'after' filter - get summaries after a specific timestamp
        let results_after = db
            .get_hourly_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Asc,
                None,
                Some((base_ts + (2 * hour_seconds)) as i64),
            )
            .await
            .unwrap();
        assert_eq!(results_after.len(), 2); // Only timestamps 3 and 4 hours
        assert_eq!(results_after[0].period_timestamp, base_ts + (3 * hour_seconds));
        assert_eq!(results_after[1].period_timestamp, base_ts + (4 * hour_seconds));

        // Test 'before' filter - get summaries before a specific timestamp
        let results_before = db
            .get_hourly_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Desc,
                Some((base_ts + (2 * hour_seconds)) as i64),
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_before.len(), 2); // Only timestamps 0 and 1 hours
        assert_eq!(results_before[0].period_timestamp, base_ts + hour_seconds);
        assert_eq!(results_before[1].period_timestamp, base_ts);

        // Test combined before/after filters
        let results_range = db
            .get_hourly_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Asc,
                Some((base_ts + (3 * hour_seconds)) as i64), // before
                Some((base_ts) as i64),                      // after
            )
            .await
            .unwrap();
        assert_eq!(results_range.len(), 2); // Timestamps 1 and 2 hours
        assert_eq!(results_range[0].period_timestamp, base_ts + hour_seconds);
        assert_eq!(results_range[1].period_timestamp, base_ts + (2 * hour_seconds));

        // Test empty results with no matching data
        let other_requestor = Address::from([0x71; 20]);
        let results_empty = db
            .get_hourly_requestor_summaries(
                other_requestor,
                None,
                10,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_empty.len(), 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_daily_requestor_summaries_with_cursor(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x72; 20]);
        let base_ts = 1700000000u64;
        let day_seconds = 86400u64;

        // Insert 5 daily summaries
        for i in 0..5 {
            let summary = DailyRequestorSummary {
                period_timestamp: base_ts + (i * day_seconds),
                requestor_address: requestor,
                total_fulfilled: 10 * (i + 1),
                unique_provers_locking_requests: 1,
                total_fees_locked: U256::from(1000),
                total_collateral_locked: U256::from(2000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(10),
                p25_lock_price_per_cycle: U256::from(25),
                p50_lock_price_per_cycle: U256::from(50),
                p75_lock_price_per_cycle: U256::from(75),
                p90_lock_price_per_cycle: U256::from(90),
                p95_lock_price_per_cycle: U256::from(95),
                p99_lock_price_per_cycle: U256::from(99),
                total_requests_submitted: 20,
                total_requests_submitted_onchain: 15,
                total_requests_submitted_offchain: 5,
                total_requests_locked: 12,
                total_requests_slashed: 0,
                total_expired: 0,
                total_locked_and_expired: 0,
                total_locked_and_fulfilled: 10,
                total_secondary_fulfillments: 4,
                locked_orders_fulfillment_rate: 0.8,
                locked_orders_fulfillment_rate_adjusted: 0.8,
                total_program_cycles: U256::from(100_000_000),
                total_cycles: U256::from(101_580_000),
                best_peak_prove_mhz: 1000.0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 900.0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_daily_requestor_summary(summary).await.unwrap();
        }

        // Test basic pagination with DESC order
        let results = db
            .get_daily_requestor_summaries(requestor, None, 2, SortDirection::Desc, None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].period_timestamp, base_ts + (4 * day_seconds));
        assert_eq!(results[1].period_timestamp, base_ts + (3 * day_seconds));

        // Test cursor pagination
        let cursor = results[1].period_timestamp as i64;
        let results2 = db
            .get_daily_requestor_summaries(
                requestor,
                Some(cursor),
                2,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        assert_eq!(results2[0].period_timestamp, base_ts + (2 * day_seconds));

        // Test ASC order
        let results_asc = db
            .get_daily_requestor_summaries(requestor, None, 3, SortDirection::Asc, None, None)
            .await
            .unwrap();
        assert_eq!(results_asc.len(), 3);
        assert_eq!(results_asc[0].period_timestamp, base_ts);
        assert_eq!(results_asc[2].period_timestamp, base_ts + (2 * day_seconds));

        // Test 'after' filter
        let results_after = db
            .get_daily_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Asc,
                None,
                Some((base_ts + day_seconds) as i64),
            )
            .await
            .unwrap();
        assert_eq!(results_after.len(), 3); // Days 2, 3, 4
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_weekly_requestor_summaries_with_cursor(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x73; 20]);
        let base_ts = 1700000000u64;
        let week_seconds = 604800u64;

        // Insert 4 weekly summaries
        for i in 0..4 {
            let summary = WeeklyRequestorSummary {
                period_timestamp: base_ts + (i * week_seconds),
                requestor_address: requestor,
                total_fulfilled: 50 * (i + 1),
                unique_provers_locking_requests: 5,
                total_fees_locked: U256::from(10000),
                total_collateral_locked: U256::from(20000),
                total_locked_and_expired_collateral: U256::ZERO,
                p10_lock_price_per_cycle: U256::from(10),
                p25_lock_price_per_cycle: U256::from(25),
                p50_lock_price_per_cycle: U256::from(50),
                p75_lock_price_per_cycle: U256::from(75),
                p90_lock_price_per_cycle: U256::from(90),
                p95_lock_price_per_cycle: U256::from(95),
                p99_lock_price_per_cycle: U256::from(99),
                total_requests_submitted: 100,
                total_requests_submitted_onchain: 80,
                total_requests_submitted_offchain: 20,
                total_requests_locked: 60,
                total_requests_slashed: 0,
                total_expired: 0,
                total_locked_and_expired: 0,
                total_locked_and_fulfilled: 50,
                total_secondary_fulfillments: 10,
                locked_orders_fulfillment_rate: 0.83,
                locked_orders_fulfillment_rate_adjusted: 0.83,
                total_program_cycles: U256::from(500_000_000),
                total_cycles: U256::from(507_900_000),
                best_peak_prove_mhz: 1200.0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 1100.0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_weekly_requestor_summary(summary).await.unwrap();
        }

        // Test basic pagination
        let results = db
            .get_weekly_requestor_summaries(requestor, None, 2, SortDirection::Desc, None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].period_timestamp, base_ts + (3 * week_seconds));
        assert_eq!(results[1].period_timestamp, base_ts + (2 * week_seconds));

        // Test cursor pagination
        let cursor = results[1].period_timestamp as i64;
        let results2 = db
            .get_weekly_requestor_summaries(
                requestor,
                Some(cursor),
                2,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);

        // Test 'before' filter
        let results_before = db
            .get_weekly_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Desc,
                Some((base_ts + (2 * week_seconds)) as i64),
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_before.len(), 2); // Weeks 0 and 1
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_all_time_requestor_summaries_with_cursor(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x74; 20]);
        let base_ts = 1700000000u64;
        let interval = 10000u64;

        // Insert 5 all-time summaries at different timestamps
        for i in 0..5 {
            let summary = AllTimeRequestorSummary {
                period_timestamp: base_ts + (i * interval),
                requestor_address: requestor,
                total_fulfilled: 100 * (i + 1),
                unique_provers_locking_requests: 10,
                total_fees_locked: U256::from(50000),
                total_collateral_locked: U256::from(100000),
                total_locked_and_expired_collateral: U256::ZERO,
                total_requests_submitted: 150,
                total_requests_submitted_onchain: 120,
                total_requests_submitted_offchain: 30,
                total_requests_locked: 110,
                total_requests_slashed: 0,
                total_expired: 0,
                total_locked_and_expired: 0,
                total_locked_and_fulfilled: 100,
                total_secondary_fulfillments: 20 * (i + 1),
                locked_orders_fulfillment_rate: 0.9,
                locked_orders_fulfillment_rate_adjusted: 0.9,
                total_program_cycles: U256::from(1_000_000_000u64 * (i + 1)),
                total_cycles: U256::from(1_015_800_000u64 * (i + 1)),
                best_peak_prove_mhz: 2000.0,
                best_peak_prove_mhz_prover: None,
                best_peak_prove_mhz_request_id: None,
                best_effective_prove_mhz: 1900.0,
                best_effective_prove_mhz_prover: None,
                best_effective_prove_mhz_request_id: None,
            };
            db.upsert_all_time_requestor_summary(summary).await.unwrap();
        }

        // Test basic pagination with DESC order (newest first)
        let results = db
            .get_all_time_requestor_summaries(requestor, None, 2, SortDirection::Desc, None, None)
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].period_timestamp, base_ts + (4 * interval));
        assert_eq!(results[0].total_fulfilled, 500);
        assert_eq!(results[1].period_timestamp, base_ts + (3 * interval));

        // Test cursor pagination
        let cursor = results[1].period_timestamp as i64;
        let results2 = db
            .get_all_time_requestor_summaries(
                requestor,
                Some(cursor),
                2,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results2.len(), 2);
        assert_eq!(results2[0].period_timestamp, base_ts + (2 * interval));

        // Test ASC order (oldest first)
        let results_asc = db
            .get_all_time_requestor_summaries(requestor, None, 3, SortDirection::Asc, None, None)
            .await
            .unwrap();
        assert_eq!(results_asc.len(), 3);
        assert_eq!(results_asc[0].period_timestamp, base_ts);
        assert_eq!(results_asc[0].total_fulfilled, 100);

        // Test 'after' filter
        let results_after = db
            .get_all_time_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Asc,
                None,
                Some((base_ts + (2 * interval)) as i64),
            )
            .await
            .unwrap();
        assert_eq!(results_after.len(), 2); // Timestamps 3 and 4

        // Test 'before' filter
        let results_before = db
            .get_all_time_requestor_summaries(
                requestor,
                None,
                10,
                SortDirection::Desc,
                Some((base_ts + (3 * interval)) as i64),
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_before.len(), 3); // Timestamps 0, 1, 2

        // Test combined cursor + before/after
        let results_combined = db
            .get_all_time_requestor_summaries(
                requestor,
                Some((base_ts + (3 * interval)) as i64), // cursor
                10,
                SortDirection::Desc,
                Some((base_ts + interval) as i64), // before
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_combined.len(), 1); // Only timestamp 2 * interval

        // Test empty results
        let other_requestor = Address::from([0x75; 20]);
        let results_empty = db
            .get_all_time_requestor_summaries(
                other_requestor,
                None,
                10,
                SortDirection::Desc,
                None,
                None,
            )
            .await
            .unwrap();
        assert_eq!(results_empty.len(), 0);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_requestor_leaderboard(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor1 = Address::from([0x01; 20]);
        let requestor2 = Address::from([0x02; 20]);
        let requestor3 = Address::from([0x03; 20]);
        let period_ts = 1700000000u64;

        // Create hourly summaries for 3 requestors with different order counts
        let summary1 = HourlyRequestorSummary {
            period_timestamp: period_ts,
            requestor_address: requestor1,
            total_fulfilled: 5,
            unique_provers_locking_requests: 2,
            total_fees_locked: U256::from(1000),
            total_collateral_locked: U256::from(5000),
            total_locked_and_expired_collateral: U256::ZERO,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 100, // Most orders
            total_requests_submitted_onchain: 70,
            total_requests_submitted_offchain: 30,
            total_requests_locked: 80,
            total_requests_slashed: 1,
            total_expired: 2,
            total_locked_and_expired: 1,
            total_locked_and_fulfilled: 5,
            total_secondary_fulfillments: 0,
            locked_orders_fulfillment_rate: 83.3,
            locked_orders_fulfillment_rate_adjusted: 83.3,
            total_program_cycles: U256::from(1_000_000_000u64),
            total_cycles: U256::from(1_100_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        let mut summary2 = summary1.clone();
        summary2.requestor_address = requestor2;
        summary2.total_requests_submitted = 50; // Second most

        let mut summary3 = summary1.clone();
        summary3.requestor_address = requestor3;
        summary3.total_requests_submitted = 25; // Least orders

        db.upsert_hourly_requestor_summary(summary1).await.unwrap();
        db.upsert_hourly_requestor_summary(summary2).await.unwrap();
        db.upsert_hourly_requestor_summary(summary3).await.unwrap();

        // Test leaderboard query - should be sorted by orders_requested DESC
        let results = db
            .get_requestor_leaderboard(
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
        assert_eq!(results[0].requestor_address, requestor1);
        assert_eq!(results[0].orders_requested, 100);
        assert_eq!(results[1].requestor_address, requestor2);
        assert_eq!(results[1].orders_requested, 50);
        assert_eq!(results[2].requestor_address, requestor3);
        assert_eq!(results[2].orders_requested, 25);

        // Test pagination with cursor
        let results_page1 = db
            .get_requestor_leaderboard(period_ts, period_ts + 3600, true, None, None, 2)
            .await
            .unwrap();
        assert_eq!(results_page1.len(), 2);
        assert_eq!(results_page1[0].orders_requested, 100);
        assert_eq!(results_page1[1].orders_requested, 50);

        // Get page 2 using cursor from last item of page 1
        let last = &results_page1[1];
        let results_page2 = db
            .get_requestor_leaderboard(
                period_ts,
                period_ts + 3600,
                true,
                Some(last.orders_requested),
                Some(last.requestor_address),
                2,
            )
            .await
            .unwrap();
        assert_eq!(results_page2.len(), 1);
        assert_eq!(results_page2[0].orders_requested, 25);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_requestor_leaderboard_aggregation(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor = Address::from([0x01; 20]);
        let hour1 = 1700000000u64;
        let hour2 = hour1 + 3600;

        // Create summaries for two hours
        let summary1 = HourlyRequestorSummary {
            period_timestamp: hour1,
            requestor_address: requestor,
            total_fulfilled: 5,
            unique_provers_locking_requests: 2,
            total_fees_locked: U256::from(1000),
            total_collateral_locked: U256::from(5000),
            total_locked_and_expired_collateral: U256::ZERO,
            p10_lock_price_per_cycle: U256::from(10),
            p25_lock_price_per_cycle: U256::from(25),
            p50_lock_price_per_cycle: U256::from(50),
            p75_lock_price_per_cycle: U256::from(75),
            p90_lock_price_per_cycle: U256::from(90),
            p95_lock_price_per_cycle: U256::from(95),
            p99_lock_price_per_cycle: U256::from(99),
            total_requests_submitted: 30,
            total_requests_submitted_onchain: 20,
            total_requests_submitted_offchain: 10,
            total_requests_locked: 25,
            total_requests_slashed: 0,
            total_expired: 1,
            total_locked_and_expired: 1,
            total_locked_and_fulfilled: 20,
            total_secondary_fulfillments: 0,
            locked_orders_fulfillment_rate: 95.0,
            locked_orders_fulfillment_rate_adjusted: 95.0,
            total_program_cycles: U256::from(500_000_000u64),
            total_cycles: U256::from(550_000_000u64),
            best_peak_prove_mhz: 1500.0,
            best_peak_prove_mhz_prover: None,
            best_peak_prove_mhz_request_id: None,
            best_effective_prove_mhz: 1400.0,
            best_effective_prove_mhz_prover: None,
            best_effective_prove_mhz_request_id: None,
        };

        let mut summary2 = summary1.clone();
        summary2.period_timestamp = hour2;
        summary2.total_requests_submitted = 20;
        summary2.total_requests_locked = 15;
        summary2.total_locked_and_fulfilled = 10;
        summary2.total_locked_and_expired = 2;
        summary2.total_cycles = U256::from(300_000_000u64);

        db.upsert_hourly_requestor_summary(summary1).await.unwrap();
        db.upsert_hourly_requestor_summary(summary2).await.unwrap();

        // Query spanning both hours - should aggregate
        let results = db
            .get_requestor_leaderboard(
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
        assert_eq!(results[0].orders_requested, 50); // 30 + 20
        assert_eq!(results[0].orders_locked, 40); // 25 + 15
        assert_eq!(results[0].cycles_requested, U256::from(850_000_000u64)); // 550M + 300M

        // Locked order fulfillment rate: (20+10) / ((20+10) + (1+2)) = 30/33 = 90.9%
        let expected_rate = (30.0 / 33.0) * 100.0;
        assert!((results[0].locked_order_fulfillment_rate - expected_rate).abs() < 0.1);
    }

    fn create_locked_request_status(
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
            lock_block: Some(101),
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
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: Some("1500".to_string()),
            lock_price_per_cycle: Some(lock_price_per_cycle.to_string()),
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: Some(B256::from([0x02; 32])),
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
    async fn test_get_requestor_median_lock_prices(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor1 = Address::from([0x01; 20]);
        let requestor2 = Address::from([0x02; 20]);
        let prover = Address::from([0xAA; 20]);
        let base_ts = 1700000000u64;

        // Insert request_status entries with lock prices for requestor1
        // Prices: 100, 200 -> median = 150.0 (interpolated, will round to 150)
        let mut statuses = Vec::new();
        for i in 0..2u8 {
            let digest = B256::from([i + 1; 32]);
            let lock_price = U256::from(100 * (i as u64 + 1));
            statuses.push(create_locked_request_status(
                digest,
                requestor1,
                prover,
                base_ts + 100,
                lock_price,
            ));
        }

        // Add requests for requestor2 with different prices
        // Prices: 100, 200, 300, 400 -> median = 250.0 (interpolated, will round to 250)
        for i in 0..4u8 {
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = 0x10 + i;
            let digest = B256::from(digest_bytes);
            let lock_price = U256::from(100 * (i as u64 + 1));
            statuses.push(create_locked_request_status(
                digest,
                requestor2,
                prover,
                base_ts + 100,
                lock_price,
            ));
        }

        db.upsert_request_statuses(&statuses).await.unwrap();

        // Query median prices
        let medians = db
            .get_requestor_median_lock_prices(&[requestor1, requestor2], base_ts, base_ts + 200)
            .await
            .unwrap();

        assert_eq!(medians.len(), 2);

        // Requestor1 has prices [100, 200], median = 150.0 (interpolated, rounded to 150)
        assert_eq!(medians.get(&requestor1), Some(&U256::from(150)));

        // Requestor2 has prices [100, 200, 300, 400], median = 250.0 (interpolated, rounded to 250)
        assert_eq!(medians.get(&requestor2), Some(&U256::from(250)));
    }

    fn create_submitted_request_status(
        digest: B256,
        client: Address,
        created_at: u64,
        updated_at: u64,
    ) -> RequestStatus {
        RequestStatus {
            request_digest: digest,
            request_id: U256::from(12345),
            request_status: RequestStatusType::Submitted,
            slashed_status: SlashedStatus::NotApplicable,
            source: "onchain".to_string(),
            client_address: client,
            lock_prover_address: None,
            fulfill_prover_address: None,
            created_at,
            updated_at,
            locked_at: None,
            fulfilled_at: None,
            slashed_at: None,
            lock_prover_delivered_proof_at: None,
            submit_block: Some(100),
            lock_block: None,
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
            program_cycles: None,
            total_cycles: None,
            peak_prove_mhz: None,
            effective_prove_mhz: None,
            prover_effective_prove_mhz: None,
            cycle_status: None,
            lock_price: None,
            lock_price_per_cycle: None,
            submit_tx_hash: Some(B256::ZERO),
            lock_tx_hash: None,
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
    async fn test_get_requestor_last_activity_times(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        let requestor1 = Address::from([0x01; 20]);
        let requestor2 = Address::from([0x02; 20]);
        let base_ts = 1700000000u64;

        let mut statuses = Vec::new();

        // Create requests for requestor1 with different timestamps
        for i in 0..3u8 {
            let digest = B256::from([i + 1; 32]);
            let ts = base_ts + (i as u64 * 1000);
            statuses.push(create_submitted_request_status(digest, requestor1, ts, ts));
        }

        // Create request for requestor2
        let digest = B256::from([0x10; 32]);
        let ts2 = base_ts + 5000;
        statuses.push(create_submitted_request_status(digest, requestor2, ts2, ts2));

        db.upsert_request_statuses(&statuses).await.unwrap();

        let activities =
            db.get_requestor_last_activity_times(&[requestor1, requestor2]).await.unwrap();

        assert_eq!(activities.len(), 2);
        // Requestor1's last activity is at base_ts + 2000 (the third request)
        assert_eq!(activities.get(&requestor1), Some(&(base_ts + 2000)));
        // Requestor2's last activity is at base_ts + 5000
        assert_eq!(activities.get(&requestor2), Some(&(base_ts + 5000)));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_get_requestor_leaderboard_empty(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &test_db.db;

        // Query leaderboard with no data
        let results = db
            .get_requestor_leaderboard(1700000000, 1700003600, true, None, None, 10)
            .await
            .unwrap();

        assert!(results.is_empty());
    }
}
