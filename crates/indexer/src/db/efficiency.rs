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

use std::sync::Arc;

use alloy::primitives::{B256, U256};
use async_trait::async_trait;
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    ConnectOptions, PgPool, Row,
};
use std::str::FromStr;
use std::time::Duration;

use super::DbError;

pub type EfficiencyDbObj = Arc<dyn EfficiencyDb + Send + Sync>;

const U256_PADDING_WIDTH: usize = 78;

fn pad_u256(value: U256) -> String {
    format!("{:0>width$}", value, width = U256_PADDING_WIDTH)
}

fn unpad_u256(s: &str) -> Result<U256, DbError> {
    let trimmed = s.trim_start_matches('0');
    if trimmed.is_empty() {
        Ok(U256::ZERO)
    } else {
        U256::from_str(trimmed)
            .map_err(|e| DbError::Error(anyhow::anyhow!("Failed to parse U256: {}", e)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoreProfitableSample {
    pub request_digest: String,
    pub request_id: String,
    pub lock_price_at_time: String,
    pub program_cycles: String,
    pub price_per_cycle_at_time: String,
}

#[derive(Debug, Clone)]
pub struct MarketEfficiencyOrder {
    pub request_digest: B256,
    pub request_id: U256,
    pub locked_at: u64,
    pub lock_price: U256,
    pub program_cycles: U256,
    pub lock_price_per_cycle: U256,
    pub num_orders_more_profitable: u64,
    pub num_orders_less_profitable: u64,
    pub num_orders_available_unfulfilled: u64,
    pub is_most_profitable: bool,
    pub more_profitable_sample: Option<Vec<MoreProfitableSample>>,
}

#[derive(Debug, Clone)]
pub struct MarketEfficiencyHourly {
    pub period_timestamp: u64,
    pub num_most_profitable_locked: u64,
    pub num_not_most_profitable_locked: u64,
    pub efficiency_rate: f64,
}

#[derive(Debug, Clone)]
pub struct MarketEfficiencyDaily {
    pub period_timestamp: u64,
    pub num_most_profitable_locked: u64,
    pub num_not_most_profitable_locked: u64,
    pub efficiency_rate: f64,
}

#[derive(Debug, Clone)]
pub struct EfficiencySummary {
    pub latest_hourly_efficiency_rate: Option<f64>,
    pub latest_daily_efficiency_rate: Option<f64>,
    pub total_requests_analyzed: u64,
    pub most_profitable_locked: u64,
    pub not_most_profitable_locked: u64,
    pub last_updated: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct EfficiencyAggregate {
    pub period_timestamp: u64,
    pub num_most_profitable_locked: u64,
    pub num_not_most_profitable_locked: u64,
    pub efficiency_rate: f64,
}

#[derive(Debug, Clone)]
pub struct RequestForEfficiency {
    pub request_digest: B256,
    pub request_id: U256,
    pub created_at: u64,
    pub locked_at: Option<u64>,
    pub fulfilled_at: Option<u64>,
    pub lock_end: u64,
    pub min_price: U256,
    pub max_price: U256,
    pub ramp_up_start: u64,
    pub ramp_up_period: u64,
    pub program_cycles: Option<U256>,
    pub lock_price: Option<U256>,
    pub lock_price_per_cycle: Option<U256>,
}

#[async_trait]
pub trait EfficiencyDb {
    async fn get_requests_for_efficiency(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<RequestForEfficiency>, DbError>;

    async fn get_last_processed_locked_at(&self) -> Result<u64, DbError>;

    async fn set_last_processed_locked_at(&self, timestamp: u64) -> Result<(), DbError>;

    async fn upsert_market_efficiency_orders(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_hourly(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_daily(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError>;

    async fn get_market_efficiency_hourly(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<MarketEfficiencyHourly>, DbError>;

    async fn get_market_efficiency_daily(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<MarketEfficiencyDaily>, DbError>;

    async fn get_market_efficiency_orders(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
        limit: u64,
    ) -> Result<Vec<MarketEfficiencyOrder>, DbError>;

    async fn get_efficiency_summary(&self) -> Result<EfficiencySummary, DbError>;

    async fn get_efficiency_aggregates(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError>;

    async fn get_efficiency_requests_paginated(
        &self,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<MarketEfficiencyOrder>, DbError>;

    async fn get_efficiency_request_by_id(
        &self,
        request_id: &str,
    ) -> Result<Option<MarketEfficiencyOrder>, DbError>;
}

pub struct EfficiencyDbImpl {
    pool: PgPool,
}

impl EfficiencyDbImpl {
    pub async fn new(
        db_conn: &str,
        acquire_timeout: Option<Duration>,
        run_migrations: bool,
    ) -> Result<Self, DbError> {
        let opts = db_conn
            .parse::<PgConnectOptions>()?
            .log_statements(LevelFilter::Trace)
            .log_slow_statements(LevelFilter::Warn, Duration::from_secs(1));

        let pool = PgPoolOptions::new()
            .max_connections(10)
            .acquire_timeout(acquire_timeout.unwrap_or(Duration::from_secs(30)))
            .connect_with(opts)
            .await?;

        if run_migrations {
            sqlx::migrate!("./migrations").run(&pool).await?;
        }

        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl EfficiencyDb for EfficiencyDbImpl {
    async fn get_requests_for_efficiency(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<RequestForEfficiency>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                request_digest,
                request_id,
                created_at,
                locked_at,
                fulfilled_at,
                lock_end,
                min_price,
                max_price,
                ramp_up_start,
                ramp_up_period,
                program_cycles,
                lock_price,
                lock_price_per_cycle
            FROM request_status
            WHERE created_at >= $1 AND created_at < $2
            "#,
        )
        .bind(from_timestamp as i64)
        .bind(to_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let request_digest_str: String = row.get("request_digest");
            let request_digest = B256::from_str(&request_digest_str)
                .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid request_digest: {}", e)))?;

            let request_id_str: String = row.get("request_id");
            let request_id = unpad_u256(&request_id_str)?;

            let min_price_str: String = row.get("min_price");
            let max_price_str: String = row.get("max_price");

            let program_cycles: Option<U256> =
                row.get::<Option<String>, _>("program_cycles").and_then(|s| unpad_u256(&s).ok());

            let lock_price: Option<U256> =
                row.get::<Option<String>, _>("lock_price").and_then(|s| unpad_u256(&s).ok());

            let lock_price_per_cycle: Option<U256> = row
                .get::<Option<String>, _>("lock_price_per_cycle")
                .and_then(|s| unpad_u256(&s).ok());

            results.push(RequestForEfficiency {
                request_digest,
                request_id,
                created_at: row.get::<i64, _>("created_at") as u64,
                locked_at: row.get::<Option<i64>, _>("locked_at").map(|v| v as u64),
                fulfilled_at: row.get::<Option<i64>, _>("fulfilled_at").map(|v| v as u64),
                lock_end: row.get::<i64, _>("lock_end") as u64,
                min_price: unpad_u256(&min_price_str)?,
                max_price: unpad_u256(&max_price_str)?,
                ramp_up_start: row.get::<i64, _>("ramp_up_start") as u64,
                ramp_up_period: row.get::<i64, _>("ramp_up_period") as u64,
                program_cycles,
                lock_price,
                lock_price_per_cycle,
            });
        }

        Ok(results)
    }

    async fn get_last_processed_locked_at(&self) -> Result<u64, DbError> {
        let row = sqlx::query(
            "SELECT last_processed_locked_at FROM market_efficiency_state WHERE id = 0",
        )
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(r) => Ok(r.get::<i64, _>("last_processed_locked_at") as u64),
            None => Ok(0),
        }
    }

    async fn set_last_processed_locked_at(&self, timestamp: u64) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO market_efficiency_state (id, last_processed_locked_at, updated_at)
            VALUES (0, $1, CURRENT_TIMESTAMP)
            ON CONFLICT (id) DO UPDATE SET 
                last_processed_locked_at = $1,
                updated_at = CURRENT_TIMESTAMP
            "#,
        )
        .bind(timestamp as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn upsert_market_efficiency_orders(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError> {
        if orders.is_empty() {
            return Ok(());
        }

        for order in orders {
            let sample_json = order
                .more_profitable_sample
                .as_ref()
                .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));

            sqlx::query(
                r#"
                INSERT INTO market_efficiency_orders (
                    request_digest, request_id, locked_at, lock_price, program_cycles,
                    lock_price_per_cycle, num_orders_more_profitable, num_orders_less_profitable,
                    num_orders_available_unfulfilled, is_most_profitable, more_profitable_sample,
                    updated_at
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, CURRENT_TIMESTAMP)
                ON CONFLICT (request_digest) DO UPDATE SET
                    request_id = $2,
                    locked_at = $3,
                    lock_price = $4,
                    program_cycles = $5,
                    lock_price_per_cycle = $6,
                    num_orders_more_profitable = $7,
                    num_orders_less_profitable = $8,
                    num_orders_available_unfulfilled = $9,
                    is_most_profitable = $10,
                    more_profitable_sample = $11,
                    updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(format!("{:x}", order.request_digest))
            .bind(pad_u256(order.request_id))
            .bind(order.locked_at as i64)
            .bind(pad_u256(order.lock_price))
            .bind(pad_u256(order.program_cycles))
            .bind(pad_u256(order.lock_price_per_cycle))
            .bind(order.num_orders_more_profitable as i64)
            .bind(order.num_orders_less_profitable as i64)
            .bind(order.num_orders_available_unfulfilled as i64)
            .bind(order.is_most_profitable)
            .bind(sample_json)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn upsert_market_efficiency_hourly(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError> {
        if summaries.is_empty() {
            return Ok(());
        }

        for summary in summaries {
            sqlx::query(
                r#"
                INSERT INTO market_efficiency_hourly (
                    period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked,
                    efficiency_rate, updated_at
                ) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
                ON CONFLICT (period_timestamp) DO UPDATE SET
                    num_most_profitable_locked = $2,
                    num_not_most_profitable_locked = $3,
                    efficiency_rate = $4,
                    updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(summary.period_timestamp as i64)
            .bind(summary.num_most_profitable_locked as i64)
            .bind(summary.num_not_most_profitable_locked as i64)
            .bind(summary.efficiency_rate)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn upsert_market_efficiency_daily(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError> {
        if summaries.is_empty() {
            return Ok(());
        }

        for summary in summaries {
            sqlx::query(
                r#"
                INSERT INTO market_efficiency_daily (
                    period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked,
                    efficiency_rate, updated_at
                ) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
                ON CONFLICT (period_timestamp) DO UPDATE SET
                    num_most_profitable_locked = $2,
                    num_not_most_profitable_locked = $3,
                    efficiency_rate = $4,
                    updated_at = CURRENT_TIMESTAMP
                "#,
            )
            .bind(summary.period_timestamp as i64)
            .bind(summary.num_most_profitable_locked as i64)
            .bind(summary.num_not_most_profitable_locked as i64)
            .bind(summary.efficiency_rate)
            .execute(&self.pool)
            .await?;
        }

        Ok(())
    }

    async fn get_market_efficiency_hourly(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<MarketEfficiencyHourly>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked, efficiency_rate
            FROM market_efficiency_hourly
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            ORDER BY period_timestamp DESC
            "#,
        )
        .bind(from_timestamp as i64)
        .bind(to_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| MarketEfficiencyHourly {
                period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                num_most_profitable_locked: row.get::<i64, _>("num_most_profitable_locked") as u64,
                num_not_most_profitable_locked: row.get::<i64, _>("num_not_most_profitable_locked")
                    as u64,
                efficiency_rate: row.get("efficiency_rate"),
            })
            .collect())
    }

    async fn get_market_efficiency_daily(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
    ) -> Result<Vec<MarketEfficiencyDaily>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked, efficiency_rate
            FROM market_efficiency_daily
            WHERE period_timestamp >= $1 AND period_timestamp < $2
            ORDER BY period_timestamp DESC
            "#,
        )
        .bind(from_timestamp as i64)
        .bind(to_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| MarketEfficiencyDaily {
                period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                num_most_profitable_locked: row.get::<i64, _>("num_most_profitable_locked") as u64,
                num_not_most_profitable_locked: row.get::<i64, _>("num_not_most_profitable_locked")
                    as u64,
                efficiency_rate: row.get("efficiency_rate"),
            })
            .collect())
    }

    async fn get_market_efficiency_orders(
        &self,
        from_timestamp: u64,
        to_timestamp: u64,
        limit: u64,
    ) -> Result<Vec<MarketEfficiencyOrder>, DbError> {
        let rows = sqlx::query(
            r#"
            SELECT 
                request_digest, request_id, locked_at, lock_price, program_cycles,
                lock_price_per_cycle, num_orders_more_profitable, num_orders_less_profitable,
                num_orders_available_unfulfilled, is_most_profitable, more_profitable_sample
            FROM market_efficiency_orders
            WHERE locked_at >= $1 AND locked_at < $2
            ORDER BY locked_at DESC
            LIMIT $3
            "#,
        )
        .bind(from_timestamp as i64)
        .bind(to_timestamp as i64)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let request_digest_str: String = row.get("request_digest");
            let request_digest = B256::from_str(&request_digest_str)
                .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid request_digest: {}", e)))?;

            let sample: Option<Vec<MoreProfitableSample>> = row
                .get::<Option<serde_json::Value>, _>("more_profitable_sample")
                .and_then(|v| serde_json::from_value(v).ok());

            results.push(MarketEfficiencyOrder {
                request_digest,
                request_id: unpad_u256(&row.get::<String, _>("request_id"))?,
                locked_at: row.get::<i64, _>("locked_at") as u64,
                lock_price: unpad_u256(&row.get::<String, _>("lock_price"))?,
                program_cycles: unpad_u256(&row.get::<String, _>("program_cycles"))?,
                lock_price_per_cycle: unpad_u256(&row.get::<String, _>("lock_price_per_cycle"))?,
                num_orders_more_profitable: row.get::<i64, _>("num_orders_more_profitable") as u64,
                num_orders_less_profitable: row.get::<i64, _>("num_orders_less_profitable") as u64,
                num_orders_available_unfulfilled: row
                    .get::<i64, _>("num_orders_available_unfulfilled")
                    as u64,
                is_most_profitable: row.get("is_most_profitable"),
                more_profitable_sample: sample,
            });
        }

        Ok(results)
    }

    async fn get_efficiency_summary(&self) -> Result<EfficiencySummary, DbError> {
        // Get latest hourly rate
        let latest_hourly = sqlx::query(
            r#"
            SELECT efficiency_rate, period_timestamp
            FROM market_efficiency_hourly
            ORDER BY period_timestamp DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        // Get latest daily rate
        let latest_daily = sqlx::query(
            r#"
            SELECT efficiency_rate
            FROM market_efficiency_daily
            ORDER BY period_timestamp DESC
            LIMIT 1
            "#,
        )
        .fetch_optional(&self.pool)
        .await?;

        // Get totals from orders table
        let totals = sqlx::query(
            r#"
            SELECT 
                COUNT(*) as total,
                SUM(CASE WHEN is_most_profitable THEN 1 ELSE 0 END) as most_profitable,
                SUM(CASE WHEN NOT is_most_profitable THEN 1 ELSE 0 END) as not_most_profitable
            FROM market_efficiency_orders
            "#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(EfficiencySummary {
            latest_hourly_efficiency_rate: latest_hourly.as_ref().map(|r| r.get("efficiency_rate")),
            latest_daily_efficiency_rate: latest_daily.as_ref().map(|r| r.get("efficiency_rate")),
            total_requests_analyzed: totals.get::<i64, _>("total") as u64,
            most_profitable_locked: totals.get::<Option<i64>, _>("most_profitable").unwrap_or(0)
                as u64,
            not_most_profitable_locked: totals
                .get::<Option<i64>, _>("not_most_profitable")
                .unwrap_or(0) as u64,
            last_updated: latest_hourly.as_ref().map(|r| r.get("period_timestamp")),
        })
    }

    async fn get_efficiency_aggregates(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError> {
        let table = match granularity {
            "daily" => "market_efficiency_daily",
            _ => "market_efficiency_hourly",
        };

        let order_dir = if sort_desc { "DESC" } else { "ASC" };
        let cursor_op = if sort_desc { "<" } else { ">" };

        let mut conditions = Vec::new();
        let mut bind_values: Vec<i64> = Vec::new();

        if let Some(b) = before {
            bind_values.push(b as i64);
            conditions.push(format!("period_timestamp < ${}", bind_values.len()));
        }
        if let Some(a) = after {
            bind_values.push(a as i64);
            conditions.push(format!("period_timestamp > ${}", bind_values.len()));
        }
        if let Some(c) = cursor {
            bind_values.push(c as i64);
            conditions.push(format!("period_timestamp {} ${}", cursor_op, bind_values.len()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        bind_values.push(limit as i64);
        let limit_param = bind_values.len();

        let query = format!(
            r#"
            SELECT period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked, efficiency_rate
            FROM {}
            {}
            ORDER BY period_timestamp {}
            LIMIT ${}
            "#,
            table, where_clause, order_dir, limit_param
        );

        let mut q = sqlx::query(&query);
        for val in &bind_values {
            q = q.bind(*val);
        }

        let rows = q.fetch_all(&self.pool).await?;

        Ok(rows
            .into_iter()
            .map(|row| EfficiencyAggregate {
                period_timestamp: row.get::<i64, _>("period_timestamp") as u64,
                num_most_profitable_locked: row.get::<i64, _>("num_most_profitable_locked") as u64,
                num_not_most_profitable_locked: row.get::<i64, _>("num_not_most_profitable_locked")
                    as u64,
                efficiency_rate: row.get("efficiency_rate"),
            })
            .collect())
    }

    async fn get_efficiency_requests_paginated(
        &self,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<MarketEfficiencyOrder>, DbError> {
        let order_dir = if sort_desc { "DESC" } else { "ASC" };
        let cursor_op = if sort_desc { "<" } else { ">" };

        let mut conditions = Vec::new();
        let mut bind_values: Vec<i64> = Vec::new();

        if let Some(b) = before {
            bind_values.push(b as i64);
            conditions.push(format!("locked_at < ${}", bind_values.len()));
        }
        if let Some(a) = after {
            bind_values.push(a as i64);
            conditions.push(format!("locked_at > ${}", bind_values.len()));
        }
        if let Some(c) = cursor {
            bind_values.push(c as i64);
            conditions.push(format!("locked_at {} ${}", cursor_op, bind_values.len()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        bind_values.push(limit as i64);
        let limit_param = bind_values.len();

        let query = format!(
            r#"
            SELECT 
                request_digest, request_id, locked_at, lock_price, program_cycles,
                lock_price_per_cycle, num_orders_more_profitable, num_orders_less_profitable,
                num_orders_available_unfulfilled, is_most_profitable, more_profitable_sample
            FROM market_efficiency_orders
            {}
            ORDER BY locked_at {}
            LIMIT ${}
            "#,
            where_clause, order_dir, limit_param
        );

        let mut q = sqlx::query(&query);
        for val in &bind_values {
            q = q.bind(*val);
        }

        let rows = q.fetch_all(&self.pool).await?;

        let mut results = Vec::with_capacity(rows.len());
        for row in rows {
            let request_digest_str: String = row.get("request_digest");
            let request_digest = B256::from_str(&request_digest_str)
                .map_err(|e| DbError::Error(anyhow::anyhow!("Invalid request_digest: {}", e)))?;

            let sample: Option<Vec<MoreProfitableSample>> = row
                .get::<Option<serde_json::Value>, _>("more_profitable_sample")
                .and_then(|v| serde_json::from_value(v).ok());

            results.push(MarketEfficiencyOrder {
                request_digest,
                request_id: unpad_u256(&row.get::<String, _>("request_id"))?,
                locked_at: row.get::<i64, _>("locked_at") as u64,
                lock_price: unpad_u256(&row.get::<String, _>("lock_price"))?,
                program_cycles: unpad_u256(&row.get::<String, _>("program_cycles"))?,
                lock_price_per_cycle: unpad_u256(&row.get::<String, _>("lock_price_per_cycle"))?,
                num_orders_more_profitable: row.get::<i64, _>("num_orders_more_profitable") as u64,
                num_orders_less_profitable: row.get::<i64, _>("num_orders_less_profitable") as u64,
                num_orders_available_unfulfilled: row
                    .get::<i64, _>("num_orders_available_unfulfilled")
                    as u64,
                is_most_profitable: row.get("is_most_profitable"),
                more_profitable_sample: sample,
            });
        }

        Ok(results)
    }

    async fn get_efficiency_request_by_id(
        &self,
        request_id: &str,
    ) -> Result<Option<MarketEfficiencyOrder>, DbError> {
        // Pad the request_id to match the stored format
        let padded_request_id = if request_id.len() < U256_PADDING_WIDTH {
            format!("{:0>width$}", request_id, width = U256_PADDING_WIDTH)
        } else {
            request_id.to_string()
        };

        let row = sqlx::query(
            r#"
            SELECT 
                request_digest, request_id, locked_at, lock_price, program_cycles,
                lock_price_per_cycle, num_orders_more_profitable, num_orders_less_profitable,
                num_orders_available_unfulfilled, is_most_profitable, more_profitable_sample
            FROM market_efficiency_orders
            WHERE request_id = $1
            "#,
        )
        .bind(&padded_request_id)
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => {
                let request_digest_str: String = row.get("request_digest");
                let request_digest = B256::from_str(&request_digest_str).map_err(|e| {
                    DbError::Error(anyhow::anyhow!("Invalid request_digest: {}", e))
                })?;

                let sample: Option<Vec<MoreProfitableSample>> = row
                    .get::<Option<serde_json::Value>, _>("more_profitable_sample")
                    .and_then(|v| serde_json::from_value(v).ok());

                Ok(Some(MarketEfficiencyOrder {
                    request_digest,
                    request_id: unpad_u256(&row.get::<String, _>("request_id"))?,
                    locked_at: row.get::<i64, _>("locked_at") as u64,
                    lock_price: unpad_u256(&row.get::<String, _>("lock_price"))?,
                    program_cycles: unpad_u256(&row.get::<String, _>("program_cycles"))?,
                    lock_price_per_cycle: unpad_u256(
                        &row.get::<String, _>("lock_price_per_cycle"),
                    )?,
                    num_orders_more_profitable: row.get::<i64, _>("num_orders_more_profitable")
                        as u64,
                    num_orders_less_profitable: row.get::<i64, _>("num_orders_less_profitable")
                        as u64,
                    num_orders_available_unfulfilled: row
                        .get::<i64, _>("num_orders_available_unfulfilled")
                        as u64,
                    is_most_profitable: row.get("is_most_profitable"),
                    more_profitable_sample: sample,
                }))
            }
            None => Ok(None),
        }
    }
}
