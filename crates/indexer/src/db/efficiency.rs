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

use super::market::{padded_string_to_u256, u256_to_padded_string};
use super::DbError;

pub type EfficiencyDbObj = Arc<dyn EfficiencyDb + Send + Sync>;

const U256_PADDING_WIDTH: usize = 78;

const EFFICIENCY_READ_BATCH_SIZE: i64 = 200;
const EFFICIENCY_UPSERT_BATCH_SIZE: usize = 100;
const EFFICIENCY_READ_BATCH_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoreProfitableSample {
    pub request_digest: String,
    pub request_id: String,
    #[serde(default)]
    pub requestor_address: String,
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
    pub client_address: String,
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
    pub selector: String,
    pub lock_base_fee: Option<U256>,
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

    async fn upsert_market_efficiency_orders_gas_adjusted(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_hourly_gas_adjusted(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_daily_gas_adjusted(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError>;

    async fn get_efficiency_summary_gas_adjusted(&self) -> Result<EfficiencySummary, DbError>;

    async fn get_efficiency_aggregates_gas_adjusted(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError>;

    async fn upsert_market_efficiency_orders_gas_adjusted_with_exclusions(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_hourly_gas_adjusted_with_exclusions(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError>;

    async fn upsert_market_efficiency_daily_gas_adjusted_with_exclusions(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError>;

    async fn get_efficiency_summary_gas_adjusted_with_exclusions(
        &self,
    ) -> Result<EfficiencySummary, DbError>;

    async fn get_efficiency_aggregates_gas_adjusted_with_exclusions(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError>;
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
        let trivial_timeout_secs = 10u64;
        match tokio::time::timeout(
            Duration::from_secs(trivial_timeout_secs),
            sqlx::query("SELECT 1").fetch_one(&self.pool),
        )
        .await
        {
            Ok(Ok(_)) => tracing::info!("Trivial query returned; database connection is responsive"),
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                return Err(DbError::Error(anyhow::anyhow!(
                    "Trivial query (SELECT 1) timed out after {}s; database may be unreachable from this host",
                    trivial_timeout_secs
                )))
            }
        }

        let mut results = Vec::new();
        let mut offset = 0i64;
        let mut batch_num = 0u64;

        loop {
            batch_num += 1;
            tracing::info!(
                "Fetching batch {} (offset {}, limit {})",
                batch_num,
                offset,
                EFFICIENCY_READ_BATCH_SIZE
            );
            let query_future = sqlx::query(
                r#"
                SELECT 
                    request_digest,
                    request_id,
                    client_address,
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
                    lock_price_per_cycle,
                    selector,
                    lock_base_fee
                FROM request_status
                WHERE created_at >= $1 AND created_at < $2
                ORDER BY created_at ASC
                LIMIT $3 OFFSET $4
                "#,
            )
            .bind(from_timestamp as i64)
            .bind(to_timestamp as i64)
            .bind(EFFICIENCY_READ_BATCH_SIZE)
            .bind(offset)
            .fetch_all(&self.pool);

            let rows = match tokio::time::timeout(
                Duration::from_secs(EFFICIENCY_READ_BATCH_TIMEOUT_SECS),
                query_future,
            )
            .await
            {
                Ok(Ok(rows)) => rows,
                Ok(Err(e)) => return Err(e.into()),
                Err(_) => {
                    return Err(DbError::Error(anyhow::anyhow!(
                    "get_requests_for_efficiency: batch {} query timed out after {}s (offset {}). \
                         Database may be slow or unreachable from this host.",
                    batch_num,
                    EFFICIENCY_READ_BATCH_TIMEOUT_SECS,
                    offset
                )))
                }
            };

            let batch_size = rows.len();
            tracing::info!("Batch {} query returned, {} rows", batch_num, batch_size);
            for row in rows.into_iter() {
                let request_digest_str: String = row.get("request_digest");
                let request_digest = B256::from_str(&request_digest_str).map_err(|e| {
                    DbError::Error(anyhow::anyhow!("Invalid request_digest: {}", e))
                })?;

                let request_id_str: String = row.get("request_id");
                let request_id = U256::from_str_radix(&request_id_str, 16)
                    .map_err(|e| DbError::BadTransaction(format!("Invalid request_id: {}", e)))?;

                let min_price_str: String = row.get("min_price");
                let max_price_str: String = row.get("max_price");

                let program_cycles: Option<U256> = row
                    .get::<Option<String>, _>("program_cycles")
                    .and_then(|s| padded_string_to_u256(&s).ok());

                let lock_price: Option<U256> = row
                    .get::<Option<String>, _>("lock_price")
                    .and_then(|s| padded_string_to_u256(&s).ok());

                let lock_price_per_cycle: Option<U256> = row
                    .get::<Option<String>, _>("lock_price_per_cycle")
                    .and_then(|s| padded_string_to_u256(&s).ok());

                let lock_base_fee: Option<U256> = row
                    .get::<Option<String>, _>("lock_base_fee")
                    .and_then(|s| padded_string_to_u256(&s).ok());

                results.push(RequestForEfficiency {
                    request_digest,
                    request_id,
                    client_address: row.get::<String, _>("client_address"),
                    created_at: row.get::<i64, _>("created_at") as u64,
                    locked_at: row.get::<Option<i64>, _>("locked_at").map(|v| v as u64),
                    fulfilled_at: row.get::<Option<i64>, _>("fulfilled_at").map(|v| v as u64),
                    lock_end: row.get::<i64, _>("lock_end") as u64,
                    min_price: padded_string_to_u256(&min_price_str)?,
                    max_price: padded_string_to_u256(&max_price_str)?,
                    ramp_up_start: row.get::<i64, _>("ramp_up_start") as u64,
                    ramp_up_period: row.get::<i64, _>("ramp_up_period") as u64,
                    program_cycles,
                    lock_price,
                    lock_price_per_cycle,
                    selector: row.get::<String, _>("selector"),
                    lock_base_fee,
                });
            }

            tracing::info!(
                "Loaded batch {}: {} rows (total so far: {})",
                batch_num,
                batch_size,
                results.len()
            );

            if batch_size < EFFICIENCY_READ_BATCH_SIZE as usize {
                break;
            }
            offset += EFFICIENCY_READ_BATCH_SIZE;
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
        self.upsert_orders_to_table("market_efficiency_orders", orders).await
    }

    async fn upsert_market_efficiency_hourly(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError> {
        self.upsert_hourly_to_table("market_efficiency_hourly", summaries).await
    }

    async fn upsert_market_efficiency_daily(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError> {
        self.upsert_daily_to_table("market_efficiency_daily", summaries).await
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
                request_id: padded_string_to_u256(&row.get::<String, _>("request_id"))?,
                locked_at: row.get::<i64, _>("locked_at") as u64,
                lock_price: padded_string_to_u256(&row.get::<String, _>("lock_price"))?,
                program_cycles: padded_string_to_u256(&row.get::<String, _>("program_cycles"))?,
                lock_price_per_cycle: padded_string_to_u256(
                    &row.get::<String, _>("lock_price_per_cycle"),
                )?,
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
        self.get_summary_from_tables(
            "market_efficiency_hourly",
            "market_efficiency_daily",
            "market_efficiency_orders",
        )
        .await
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
        self.get_aggregates_from_table(table, before, after, limit, cursor, sort_desc).await
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
                request_id: padded_string_to_u256(&row.get::<String, _>("request_id"))?,
                locked_at: row.get::<i64, _>("locked_at") as u64,
                lock_price: padded_string_to_u256(&row.get::<String, _>("lock_price"))?,
                program_cycles: padded_string_to_u256(&row.get::<String, _>("program_cycles"))?,
                lock_price_per_cycle: padded_string_to_u256(
                    &row.get::<String, _>("lock_price_per_cycle"),
                )?,
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
                    request_id: padded_string_to_u256(&row.get::<String, _>("request_id"))?,
                    locked_at: row.get::<i64, _>("locked_at") as u64,
                    lock_price: padded_string_to_u256(&row.get::<String, _>("lock_price"))?,
                    program_cycles: padded_string_to_u256(&row.get::<String, _>("program_cycles"))?,
                    lock_price_per_cycle: padded_string_to_u256(
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

    async fn upsert_market_efficiency_orders_gas_adjusted(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError> {
        self.upsert_orders_to_table("market_efficiency_orders_gas_adjusted", orders).await
    }

    async fn upsert_market_efficiency_hourly_gas_adjusted(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError> {
        self.upsert_hourly_to_table("market_efficiency_hourly_gas_adjusted", summaries).await
    }

    async fn upsert_market_efficiency_daily_gas_adjusted(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError> {
        self.upsert_daily_to_table("market_efficiency_daily_gas_adjusted", summaries).await
    }

    async fn get_efficiency_summary_gas_adjusted(&self) -> Result<EfficiencySummary, DbError> {
        self.get_summary_from_tables(
            "market_efficiency_hourly_gas_adjusted",
            "market_efficiency_daily_gas_adjusted",
            "market_efficiency_orders_gas_adjusted",
        )
        .await
    }

    async fn get_efficiency_aggregates_gas_adjusted(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError> {
        let table = match granularity {
            "daily" => "market_efficiency_daily_gas_adjusted",
            _ => "market_efficiency_hourly_gas_adjusted",
        };
        self.get_aggregates_from_table(table, before, after, limit, cursor, sort_desc).await
    }

    async fn upsert_market_efficiency_orders_gas_adjusted_with_exclusions(
        &self,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError> {
        self.upsert_orders_to_table("market_efficiency_orders_gas_adjusted_with_exclusions", orders)
            .await
    }

    async fn upsert_market_efficiency_hourly_gas_adjusted_with_exclusions(
        &self,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError> {
        self.upsert_hourly_to_table(
            "market_efficiency_hourly_gas_adjusted_with_exclusions",
            summaries,
        )
        .await
    }

    async fn upsert_market_efficiency_daily_gas_adjusted_with_exclusions(
        &self,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError> {
        self.upsert_daily_to_table(
            "market_efficiency_daily_gas_adjusted_with_exclusions",
            summaries,
        )
        .await
    }

    async fn get_efficiency_summary_gas_adjusted_with_exclusions(
        &self,
    ) -> Result<EfficiencySummary, DbError> {
        self.get_summary_from_tables(
            "market_efficiency_hourly_gas_adjusted_with_exclusions",
            "market_efficiency_daily_gas_adjusted_with_exclusions",
            "market_efficiency_orders_gas_adjusted_with_exclusions",
        )
        .await
    }

    async fn get_efficiency_aggregates_gas_adjusted_with_exclusions(
        &self,
        granularity: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError> {
        let table = match granularity {
            "daily" => "market_efficiency_daily_gas_adjusted_with_exclusions",
            _ => "market_efficiency_hourly_gas_adjusted_with_exclusions",
        };
        self.get_aggregates_from_table(table, before, after, limit, cursor, sort_desc).await
    }
}

impl EfficiencyDbImpl {
    async fn upsert_orders_to_table(
        &self,
        table: &str,
        orders: &[MarketEfficiencyOrder],
    ) -> Result<(), DbError> {
        if orders.is_empty() {
            return Ok(());
        }

        let total_chunks =
            (orders.len() + EFFICIENCY_UPSERT_BATCH_SIZE - 1) / EFFICIENCY_UPSERT_BATCH_SIZE;

        for (chunk_idx, chunk) in orders.chunks(EFFICIENCY_UPSERT_BATCH_SIZE).enumerate() {
            if chunk.is_empty() {
                continue;
            }

            let mut value_clauses = Vec::with_capacity(chunk.len());
            let mut param_idx = 1i32;
            for _ in chunk {
                value_clauses.push(format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, ${}, CURRENT_TIMESTAMP)",
                    param_idx,
                    param_idx + 1,
                    param_idx + 2,
                    param_idx + 3,
                    param_idx + 4,
                    param_idx + 5,
                    param_idx + 6,
                    param_idx + 7,
                    param_idx + 8,
                    param_idx + 9,
                    param_idx + 10,
                ));
                param_idx += 11;
            }

            let query = format!(
                r#"
                INSERT INTO {} (
                    request_digest, request_id, locked_at, lock_price, program_cycles,
                    lock_price_per_cycle, num_orders_more_profitable, num_orders_less_profitable,
                    num_orders_available_unfulfilled, is_most_profitable, more_profitable_sample,
                    updated_at
                ) VALUES {}
                ON CONFLICT (request_digest) DO UPDATE SET
                    request_id = EXCLUDED.request_id,
                    locked_at = EXCLUDED.locked_at,
                    lock_price = EXCLUDED.lock_price,
                    program_cycles = EXCLUDED.program_cycles,
                    lock_price_per_cycle = EXCLUDED.lock_price_per_cycle,
                    num_orders_more_profitable = EXCLUDED.num_orders_more_profitable,
                    num_orders_less_profitable = EXCLUDED.num_orders_less_profitable,
                    num_orders_available_unfulfilled = EXCLUDED.num_orders_available_unfulfilled,
                    is_most_profitable = EXCLUDED.is_most_profitable,
                    more_profitable_sample = EXCLUDED.more_profitable_sample,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                table,
                value_clauses.join(", ")
            );

            let mut q = sqlx::query(&query);
            for order in chunk {
                let sample_json = order
                    .more_profitable_sample
                    .as_ref()
                    .map(|s| serde_json::to_value(s).unwrap_or(serde_json::Value::Null));
                q = q
                    .bind(format!("{:x}", order.request_digest))
                    .bind(u256_to_padded_string(order.request_id))
                    .bind(order.locked_at as i64)
                    .bind(u256_to_padded_string(order.lock_price))
                    .bind(u256_to_padded_string(order.program_cycles))
                    .bind(u256_to_padded_string(order.lock_price_per_cycle))
                    .bind(order.num_orders_more_profitable as i64)
                    .bind(order.num_orders_less_profitable as i64)
                    .bind(order.num_orders_available_unfulfilled as i64)
                    .bind(order.is_most_profitable)
                    .bind(sample_json);
            }
            q.execute(&self.pool).await?;

            tracing::info!(
                "Upserted batch {}/{} to {}: {} orders",
                chunk_idx + 1,
                total_chunks,
                table,
                chunk.len()
            );
        }

        Ok(())
    }

    async fn upsert_hourly_to_table(
        &self,
        table: &str,
        summaries: &[MarketEfficiencyHourly],
    ) -> Result<(), DbError> {
        if summaries.is_empty() {
            return Ok(());
        }

        for summary in summaries {
            let query = format!(
                r#"
                INSERT INTO {} (
                    period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked,
                    efficiency_rate, updated_at
                ) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
                ON CONFLICT (period_timestamp) DO UPDATE SET
                    num_most_profitable_locked = $2,
                    num_not_most_profitable_locked = $3,
                    efficiency_rate = $4,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                table
            );
            sqlx::query(&query)
                .bind(summary.period_timestamp as i64)
                .bind(summary.num_most_profitable_locked as i64)
                .bind(summary.num_not_most_profitable_locked as i64)
                .bind(summary.efficiency_rate)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    async fn upsert_daily_to_table(
        &self,
        table: &str,
        summaries: &[MarketEfficiencyDaily],
    ) -> Result<(), DbError> {
        if summaries.is_empty() {
            return Ok(());
        }

        for summary in summaries {
            let query = format!(
                r#"
                INSERT INTO {} (
                    period_timestamp, num_most_profitable_locked, num_not_most_profitable_locked,
                    efficiency_rate, updated_at
                ) VALUES ($1, $2, $3, $4, CURRENT_TIMESTAMP)
                ON CONFLICT (period_timestamp) DO UPDATE SET
                    num_most_profitable_locked = $2,
                    num_not_most_profitable_locked = $3,
                    efficiency_rate = $4,
                    updated_at = CURRENT_TIMESTAMP
                "#,
                table
            );
            sqlx::query(&query)
                .bind(summary.period_timestamp as i64)
                .bind(summary.num_most_profitable_locked as i64)
                .bind(summary.num_not_most_profitable_locked as i64)
                .bind(summary.efficiency_rate)
                .execute(&self.pool)
                .await?;
        }

        Ok(())
    }

    async fn get_summary_from_tables(
        &self,
        hourly_table: &str,
        daily_table: &str,
        orders_table: &str,
    ) -> Result<EfficiencySummary, DbError> {
        let latest_hourly = sqlx::query(&format!(
            "SELECT efficiency_rate, period_timestamp FROM {} ORDER BY period_timestamp DESC LIMIT 1",
            hourly_table
        ))
        .fetch_optional(&self.pool)
        .await?;

        let latest_daily = sqlx::query(&format!(
            "SELECT efficiency_rate FROM {} ORDER BY period_timestamp DESC LIMIT 1",
            daily_table
        ))
        .fetch_optional(&self.pool)
        .await?;

        let totals = sqlx::query(&format!(
            r#"
            SELECT 
                COUNT(*) as total,
                SUM(CASE WHEN is_most_profitable THEN 1 ELSE 0 END) as most_profitable,
                SUM(CASE WHEN NOT is_most_profitable THEN 1 ELSE 0 END) as not_most_profitable
            FROM {}
            "#,
            orders_table
        ))
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

    async fn get_aggregates_from_table(
        &self,
        table: &str,
        before: Option<u64>,
        after: Option<u64>,
        limit: u64,
        cursor: Option<u64>,
        sort_desc: bool,
    ) -> Result<Vec<EfficiencyAggregate>, DbError> {
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
}

#[cfg(test)]
mod tests {
    // These tests use sqlx::test and require DATABASE_URL and a running Postgres.
    use super::*;

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

    fn padded_request_id(n: u64) -> String {
        format!("{:0>width$}", n, width = U256_PADDING_WIDTH)
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn get_requests_for_efficiency_returns_batched_results(pool: sqlx::PgPool) {
        let db_url = get_db_url_from_pool(&pool).await;
        let db = EfficiencyDbImpl::new(&db_url, None, false).await.unwrap();

        let digest1 = format!("{:x}", B256::from([1u8; 32]));
        let digest2 = format!("{:x}", B256::from([2u8; 32]));
        let padded = padded_request_id(1);
        let zero_padded = format!("{:0>78}", 0u32);

        sqlx::query(
            r#"
            INSERT INTO request_status (
                request_digest, request_id, request_status, slashed_status, source, client_address,
                created_at, updated_at, min_price, max_price, lock_collateral,
                ramp_up_start, ramp_up_period, expires_at, lock_end,
                image_id, selector, predicate_type, predicate_data, input_type, input_data
            ) VALUES ($1, $2, 'submitted', 'N/A', 'onchain', '0x0000000000000000000000000000000000000000',
                1000, 1000, $3, $3, $3, 0, 1, 2000, 2000,
                'img', 'sel', 'type', 'pred', 'input', 'data')
            "#,
        )
        .bind(&digest1)
        .bind(&padded)
        .bind(&zero_padded)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::query(
            r#"
            INSERT INTO request_status (
                request_digest, request_id, request_status, slashed_status, source, client_address,
                created_at, updated_at, min_price, max_price, lock_collateral,
                ramp_up_start, ramp_up_period, expires_at, lock_end,
                image_id, selector, predicate_type, predicate_data, input_type, input_data
            ) VALUES ($1, $2, 'submitted', 'N/A', 'onchain', '0x0000000000000000000000000000000000000000',
                1500, 1500, $3, $3, $3, 0, 1, 2000, 2000,
                'img', 'sel', 'type', 'pred', 'input', 'data')
            "#,
        )
        .bind(&digest2)
        .bind(&padded)
        .bind(&zero_padded)
        .execute(&pool)
        .await
        .unwrap();

        let results = db.get_requests_for_efficiency(1000, 2000).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].created_at, 1000);
        assert_eq!(results[1].created_at, 1500);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_market_efficiency_orders_inserts_and_updates(pool: sqlx::PgPool) {
        let db_url = get_db_url_from_pool(&pool).await;
        let db = EfficiencyDbImpl::new(&db_url, None, false).await.unwrap();

        let digest = B256::from([3u8; 32]);
        let orders = vec![MarketEfficiencyOrder {
            request_digest: digest,
            request_id: U256::from(1),
            locked_at: 2000,
            lock_price: U256::from(100),
            program_cycles: U256::from(1000),
            lock_price_per_cycle: U256::from(10),
            num_orders_more_profitable: 0,
            num_orders_less_profitable: 2,
            num_orders_available_unfulfilled: 0,
            is_most_profitable: true,
            more_profitable_sample: None,
        }];

        db.upsert_market_efficiency_orders(&orders).await.unwrap();

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM market_efficiency_orders WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);

        let row: (String, i64, bool) = sqlx::query_as(
            "SELECT request_id, locked_at, is_most_profitable FROM market_efficiency_orders WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.1, 2000);
        assert!(row.2);

        let orders_updated = vec![MarketEfficiencyOrder {
            request_digest: digest,
            request_id: U256::from(1),
            locked_at: 2001,
            lock_price: U256::from(101),
            program_cycles: U256::from(1000),
            lock_price_per_cycle: U256::from(10),
            num_orders_more_profitable: 1,
            num_orders_less_profitable: 1,
            num_orders_available_unfulfilled: 0,
            is_most_profitable: false,
            more_profitable_sample: None,
        }];
        db.upsert_market_efficiency_orders(&orders_updated).await.unwrap();

        let count_after: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM market_efficiency_orders WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count_after.0, 1);

        let row_after: (i64, bool) = sqlx::query_as(
            "SELECT locked_at, is_most_profitable FROM market_efficiency_orders WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row_after.0, 2001);
        assert!(!row_after.1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_market_efficiency_orders_gas_adjusted(pool: sqlx::PgPool) {
        let db_url = get_db_url_from_pool(&pool).await;
        let db = EfficiencyDbImpl::new(&db_url, None, false).await.unwrap();

        let digest = B256::from([4u8; 32]);
        let orders = vec![MarketEfficiencyOrder {
            request_digest: digest,
            request_id: U256::from(2),
            locked_at: 3000,
            lock_price: U256::from(200),
            program_cycles: U256::from(2000),
            lock_price_per_cycle: U256::from(10),
            num_orders_more_profitable: 1,
            num_orders_less_profitable: 1,
            num_orders_available_unfulfilled: 0,
            is_most_profitable: false,
            more_profitable_sample: None,
        }];

        db.upsert_market_efficiency_orders_gas_adjusted(&orders).await.unwrap();

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM market_efficiency_orders_gas_adjusted WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn upsert_market_efficiency_orders_gas_adjusted_with_exclusions(pool: sqlx::PgPool) {
        let db_url = get_db_url_from_pool(&pool).await;
        let db = EfficiencyDbImpl::new(&db_url, None, false).await.unwrap();

        let digest = B256::from([5u8; 32]);
        let orders = vec![MarketEfficiencyOrder {
            request_digest: digest,
            request_id: U256::from(3),
            locked_at: 4000,
            lock_price: U256::from(300),
            program_cycles: U256::from(3000),
            lock_price_per_cycle: U256::from(10),
            num_orders_more_profitable: 0,
            num_orders_less_profitable: 2,
            num_orders_available_unfulfilled: 0,
            is_most_profitable: true,
            more_profitable_sample: None,
        }];

        db.upsert_market_efficiency_orders_gas_adjusted_with_exclusions(&orders).await.unwrap();

        let count: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM market_efficiency_orders_gas_adjusted_with_exclusions WHERE request_digest = $1",
        )
        .bind(format!("{:x}", digest))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count.0, 1);
    }
}
