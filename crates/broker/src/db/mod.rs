// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{default::Default, str::FromStr, sync::Arc};

use alloy::primitives::{ruint::ParseError as RuintParseErr, Bytes, B256, U256};
use async_trait::async_trait;
use chrono::Utc;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use thiserror::Error;

use crate::{
    AggregationState, Batch, BatchStatus, FulfillmentType, Order, OrderStatus, ProofRequest,
};
use tracing::instrument;

#[cfg(test)]
mod fuzz_db;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Order key {0} not found in DB")]
    OrderNotFound(String),

    #[error("Batch key {0} not found in DB")]
    BatchNotFound(usize),

    #[error("Batch key {0} has no aggreagtion state")]
    BatchAggregationStateIsNone(usize),

    #[cfg(test)]
    #[error("Batch insert failed {0}")]
    BatchInsertFailure(usize),

    #[error("DB Missing column value: {0}")]
    MissingElm(&'static str),

    #[error("SQL error")]
    SqlErr(#[from] sqlx::Error),

    #[error("SQL Migration error")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("JSON serialization err")]
    JsonErr(#[from] serde_json::Error),

    #[error("Invalid order id")]
    InvalidOrderId(#[from] RuintParseErr),

    #[error("Invalid block number: {0}")]
    BadBlockNumb(String),

    #[error("Failed to set last block")]
    SetBlockFail,

    #[error("Invalid order id: {0} missing field: {1}")]
    InvalidOrder(String, &'static str),

    #[error("Invalid max connection env var value")]
    MaxConnEnvVar(#[from] std::num::ParseIntError),
}

/// Struct containing the information about an order used by the aggregation worker.
#[derive(Clone, Debug)]
pub struct AggregationOrder {
    pub order_id: String,
    pub proof_id: String,
    pub expiration: u64,
    pub fee: U256,
}

#[async_trait]
pub trait BrokerDb {
    async fn add_order(&self, order: Order) -> Result<Option<Order>, DbError>;
    async fn order_exists_with_request_id(&self, id: U256) -> Result<bool, DbError>;
    async fn get_order(&self, id: &str) -> Result<Option<Order>, DbError>;
    async fn get_submission_order(
        &self,
        id: &str,
    ) -> Result<(ProofRequest, Bytes, String, B256, U256, FulfillmentType), DbError>;
    async fn get_order_compressed_proof_id(&self, id: &str) -> Result<String, DbError>;
    async fn update_orders_for_pricing(&self, limit: u32) -> Result<Vec<Order>, DbError>;
    async fn get_active_pricing_orders(&self) -> Result<Vec<Order>, DbError>;
    async fn set_order_lock(
        &self,
        id: &str,
        lock_timestamp: u64,
        expire_timestamp: u64,
    ) -> Result<(), DbError>;
    async fn set_order_fulfill_after_lock_expire(
        &self,
        id: &str,
        lock_expire_timestamp: u64,
        expire_timestamp: u64,
    ) -> Result<(), DbError>;
    async fn set_proving_status_lock_and_fulfill_orders(
        &self,
        id: &str,
        lock_price: U256,
    ) -> Result<(), DbError>;
    async fn get_fulfill_after_lock_expire_orders(
        &self,
        end_timestamp: u64,
    ) -> Result<Vec<Order>, DbError>;
    async fn set_proving_status_fulfill_after_lock_expire_orders(
        &self,
        id: &str,
    ) -> Result<(), DbError>;
    async fn set_order_failure(&self, id: &str, failure_str: String) -> Result<(), DbError>;
    async fn set_order_complete(&self, id: &str) -> Result<(), DbError>;
    #[allow(dead_code)]
    async fn set_order_status(&self, id: &str, status: OrderStatus) -> Result<(), DbError>;
    async fn skip_order(&self, id: &str) -> Result<(), DbError>;
    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;
    async fn get_pending_lock_orders(&self, end_timestamp: u64) -> Result<Vec<Order>, DbError>;
    /// Get all orders that are committed to be fulfilled, including those that have been selected
    /// to lock or are locked and are not yet fulfilled.
    async fn get_committed_orders(&self) -> Result<Vec<Order>, DbError>;
    /// Get a count of orders that are committed to be fulfilled, including those that have been
    /// selected to lock or are locked and are not yet fulfilled.
    async fn get_committed_orders_count(&self) -> Result<u32, DbError>;
    async fn get_proving_order(&self) -> Result<Option<Order>, DbError>;
    async fn get_active_proofs(&self) -> Result<Vec<Order>, DbError>;
    async fn set_order_proof_id(&self, order_id: &str, proof_id: &str) -> Result<(), DbError>;
    async fn set_order_compressed_proof_id(
        &self,
        order_id: &str,
        proof_id: &str,
    ) -> Result<(), DbError>;
    async fn set_image_input_ids(
        &self,
        id: &str,
        image_id: &str,
        input_id: &str,
    ) -> Result<(), DbError>;
    async fn set_aggregation_status(&self, id: &str, status: OrderStatus) -> Result<(), DbError>;
    async fn get_aggregation_proofs(&self) -> Result<Vec<AggregationOrder>, DbError>;
    async fn get_groth16_proofs(&self) -> Result<Vec<AggregationOrder>, DbError>;
    async fn complete_batch(&self, batch_id: usize, g16_proof_id: &str) -> Result<(), DbError>;
    async fn get_complete_batch(&self) -> Result<Option<(usize, Batch)>, DbError>;
    async fn set_batch_submitted(&self, batch_id: usize) -> Result<(), DbError>;
    async fn set_batch_failure(&self, batch_id: usize, err: String) -> Result<(), DbError>;
    async fn get_current_batch(&self) -> Result<usize, DbError>;
    async fn set_request_fulfilled(
        &self,
        request_id: &str,
        block_timestamp: u64,
        block_number: u64,
    ) -> Result<(), DbError>;
    async fn is_request_fulfilled(&self, request_id: &str) -> Result<bool, DbError>;
    /// Update a batch with the results of an aggregation step.
    ///
    /// Sets the aggreagtion state, and adds the given orders to the batch, updating the batch fees
    /// and deadline. During finalization, the assessor_proof_id is recorded as well.
    async fn update_batch(
        &self,
        batch_id: usize,
        aggreagtion_state: &AggregationState,
        orders: &[AggregationOrder],
        assessor_proof_id: Option<String>,
    ) -> Result<(), DbError>;
    async fn get_batch(&self, batch_id: usize) -> Result<Batch, DbError>;

    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError>;
    #[cfg(test)]
    async fn set_batch_status(&self, batch_id: usize, status: BatchStatus) -> Result<(), DbError>;
}

pub type DbObj = Arc<dyn BrokerDb + Send + Sync>;

const SQL_BLOCK_KEY: i64 = 0;

pub struct SqliteDb {
    pool: SqlitePool,
}

impl SqliteDb {
    pub async fn new(conn_str: &str) -> Result<Self, DbError> {
        let opts = SqliteConnectOptions::from_str(conn_str)?
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .create_if_missing(true)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            // set timeouts to None for sqlite in-memory:
            // https://github.com/launchbadge/sqlx/issues/1647
            .max_lifetime(None)
            .idle_timeout(None)
            .min_connections(1)
            // Limit the DB to a single connection to prevent database-locked
            // this does effectively make the DB single threaded but it should be
            // a non-issue with the low DB contention
            .max_connections(1);

        let pool = pool.connect_with(opts).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    #[cfg(test)]
    pub async fn from(pool: SqlitePool) -> Result<Self, DbError> {
        Ok(Self { pool })
    }

    async fn new_batch(&self) -> Result<usize, DbError> {
        let batch = Batch { start_time: Utc::now(), ..Default::default() };

        let res: i64 = sqlx::query_scalar("INSERT INTO batches (data) VALUES ($1) RETURNING id")
            .bind(sqlx::types::Json(&batch))
            .fetch_one(&self.pool)
            .await?;

        Ok(res as usize)
    }
}

#[derive(sqlx::FromRow)]
struct DbOrder {
    id: String,
    #[sqlx(json)]
    data: Order,
}

#[derive(sqlx::FromRow)]
struct DbBatch {
    id: i64,
    #[sqlx(json)]
    data: Batch,
}

#[async_trait]
impl BrokerDb for SqliteDb {
    #[instrument(level = "trace", skip_all, fields(id = %format!("{order}")))]
    async fn add_order(&self, order: Order) -> Result<Option<Order>, DbError> {
        // TODO(austin): https://github.com/boundless-xyz/boundless/issues/162
        sqlx::query("INSERT INTO orders (id, data) VALUES ($1, $2)")
            .bind(order.id())
            .bind(sqlx::types::Json(&order))
            .execute(&self.pool)
            .await?;
        Ok(Some(order))
    }

    #[instrument(level = "trace", skip_all, fields(request_id = %format!("{request_id:x}")))]
    async fn order_exists_with_request_id(&self, request_id: U256) -> Result<bool, DbError> {
        let res: i64 = sqlx::query_scalar("SELECT COUNT(1) FROM orders WHERE id LIKE $1")
            .bind(format!("0x{request_id:x}%"))
            .fetch_one(&self.pool)
            .await?;
        Ok(res > 0)
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn get_order(&self, id: &str) -> Result<Option<Order>, DbError> {
        let order: Option<DbOrder> = sqlx::query_as("SELECT * FROM orders WHERE id = $1 LIMIT 1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(order.map(|x| x.data))
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn get_submission_order(
        &self,
        id: &str,
    ) -> Result<(ProofRequest, Bytes, String, B256, U256, FulfillmentType), DbError> {
        let order = self.get_order(id).await?;
        if let Some(order) = order {
            Ok((
                order.request.clone(),
                order.client_sig.clone(),
                order.proof_id.ok_or(DbError::MissingElm("proof_id"))?,
                order.request.requirements.imageId,
                order.lock_price.ok_or(DbError::MissingElm("lock_price"))?,
                order.fulfillment_type,
            ))
        } else {
            Err(DbError::OrderNotFound(id.to_string()))
        }
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn get_order_compressed_proof_id(&self, id: &str) -> Result<String, DbError> {
        let order = self.get_order(id).await?;
        if let Some(order) = order {
            Ok(order.compressed_proof_id.ok_or(DbError::MissingElm("compressed_proof_id"))?)
        } else {
            Err(DbError::OrderNotFound(id.to_string()))
        }
    }

    // Pick orders that are either new or were locked by others, and updates their status to either
    // Pricing or PricingLockedByOther
    #[instrument(level = "trace", skip_all, fields(limit = %limit))]
    async fn update_orders_for_pricing(&self, limit: u32) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            r#"
            WITH orders_to_update AS (
                SELECT id, data->>'status' as current_status
                FROM orders
                WHERE (data->>'status' = $1)
                LIMIT $2
            )
            UPDATE orders
            SET data = 
                    json_set(
                    json_set(data, 
                    '$.status', $3), 
                    '$.updated_at', $4
            )
            WHERE id IN (SELECT id FROM orders_to_update)
            RETURNING *
            "#,
        )
        .bind(OrderStatus::New)
        .bind(i64::from(limit))
        .bind(OrderStatus::Pricing)
        .bind(Utc::now().timestamp())
        .fetch_all(&self.pool)
        .await?;

        let result: Result<Vec<_>, _> = orders.into_iter().map(|order| Ok(order.data)).collect();

        result
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_active_pricing_orders(&self) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> =
            sqlx::query_as("SELECT * FROM orders WHERE data->>'status' = $1")
                .bind(OrderStatus::Pricing)
                .fetch_all(&self.pool)
                .await?;

        let orders: Result<Vec<_>, _> = orders.into_iter().map(|elm| Ok(elm.data)).collect();

        orders
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_lock(
        &self,
        id: &str,
        lock_timestamp: u64,
        expire_timestamp: u64,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.target_timestamp', $2),
                       '$.expire_timestamp', $3),
                       '$.updated_at', $4)
            WHERE
                id = $5"#,
        )
        .bind(OrderStatus::FulfillAfterLocking)
        // TODO: can we work out how to correctly
        // use bind + a json field with out string formatting
        // the sql query?
        .bind(
            i64::try_from(lock_timestamp)
                .map_err(|_| DbError::BadBlockNumb(lock_timestamp.to_string()))?,
        )
        .bind(
            i64::try_from(expire_timestamp)
                .map_err(|_| DbError::BadBlockNumb(expire_timestamp.to_string()))?,
        )
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_fulfill_after_lock_expire(
        &self,
        id: &str,
        lock_expire_timestamp: u64,
        expire_timestamp: u64,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.target_timestamp', $2),
                       '$.expire_timestamp', $3),
                       '$.updated_at', $4)
            WHERE
                id = $5"#,
        )
        .bind(OrderStatus::FulfillAfterLockExpire)
        .bind(
            i64::try_from(lock_expire_timestamp)
                .map_err(|_| DbError::BadBlockNumb(lock_expire_timestamp.to_string()))?,
        )
        .bind(
            i64::try_from(expire_timestamp)
                .map_err(|_| DbError::BadBlockNumb(expire_timestamp.to_string()))?,
        )
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip(self, id), fields(id = %format!("{id}")))]
    // Gets orders that we intend to lock and fulfill, and updates their status to PendingProving.
    async fn set_proving_status_lock_and_fulfill_orders(
        &self,
        id: &str,
        lock_price: U256,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2),
                       '$.lock_price', $3)
            WHERE
                id = $4"#,
        )
        .bind(OrderStatus::PendingProving)
        .bind(Utc::now().timestamp())
        .bind(lock_price.to_string())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    async fn get_fulfill_after_lock_expire_orders(
        &self,
        end_timestamp: u64,
    ) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            "SELECT * FROM orders WHERE data->>'status' = $1 AND data->>'target_timestamp' <= $2",
        )
        .bind(OrderStatus::FulfillAfterLockExpire)
        .bind(end_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        let result: Result<Vec<_>, _> = orders.into_iter().map(|order| Ok(order.data)).collect();

        result
    }

    #[instrument(level = "trace", skip_all)]
    // Gets orders that we intend to fulfill after their lock expires, and updates their status to PendingProving.
    async fn set_proving_status_fulfill_after_lock_expire_orders(
        &self,
        id: &str,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2),
                       '$.lock_price', $3)
            WHERE
                id = $4
            "#,
        )
        .bind(OrderStatus::PendingProving)
        .bind(Utc::now().timestamp())
        .bind(U256::ZERO.to_string())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_failure(&self, id: &str, failure_str: String) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2),
                       '$.error_msg', $3)
            WHERE
                id = $4"#,
        )
        .bind(OrderStatus::Failed)
        .bind(Utc::now().timestamp())
        .bind(failure_str)
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_complete(&self, id: &str) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(OrderStatus::Done)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id} {status:?}")))]
    async fn set_order_status(&self, id: &str, status: OrderStatus) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(status)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn skip_order(&self, id: &str) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(OrderStatus::Skipped)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_last_block(&self) -> Result<Option<u64>, DbError> {
        // TODO: query_as, seems to not work correctly here
        let res = sqlx::query("SELECT block FROM last_block WHERE id = $1")
            .bind(SQL_BLOCK_KEY)
            .fetch_optional(&self.pool)
            .await?;

        let Some(row) = res else {
            return Ok(None);
        };

        let block_str: String = row.try_get("block")?;

        Ok(Some(block_str.parse().map_err(|_err| DbError::BadBlockNumb(block_str))?))
    }

    #[instrument(level = "trace", skip(self))]
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError> {
        let res = sqlx::query("REPLACE INTO last_block (id, block) VALUES ($1, $2)")
            .bind(SQL_BLOCK_KEY)
            .bind(block_numb.to_string())
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::SetBlockFail);
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_pending_lock_orders(&self, end_timestamp: u64) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            "SELECT * FROM orders WHERE data->>'status' = $1 AND data->>'target_timestamp' <= $2",
        )
        .bind(OrderStatus::FulfillAfterLocking)
        .bind(end_timestamp as i64)
        .fetch_all(&self.pool)
        .await?;

        // Break if any order-id's are invalid and raise
        orders.into_iter().map(|elm| Ok(elm.data)).collect()
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_committed_orders_count(&self) -> Result<u32, DbError> {
        // Note we do not include orders that are pending proving after lock expire,
        // as we are unsure if we will commit to proving them or not until the lock expires.
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orders WHERE data->>'status' IN ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(OrderStatus::FulfillAfterLocking)
        .bind(OrderStatus::PendingProving)
        .bind(OrderStatus::Proving)
        .bind(OrderStatus::PendingAgg)
        .bind(OrderStatus::Aggregating)
        .bind(OrderStatus::SkipAggregation)
        .bind(OrderStatus::PendingSubmission)
        .fetch_one(&self.pool)
        .await?;

        Ok(u32::try_from(count).expect("count should never be negative"))
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_committed_orders(&self) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            "SELECT * FROM orders WHERE data->>'status' IN ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(OrderStatus::FulfillAfterLocking)
        .bind(OrderStatus::PendingProving)
        .bind(OrderStatus::Proving)
        .bind(OrderStatus::PendingAgg)
        .bind(OrderStatus::Aggregating)
        .bind(OrderStatus::SkipAggregation)
        .bind(OrderStatus::PendingSubmission)
        .fetch_all(&self.pool)
        .await?;

        // Break if any order-id's are invalid and raise
        orders.into_iter().map(|elm| Ok(elm.data)).collect()
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_proving_order(&self) -> Result<Option<Order>, DbError> {
        let elm: Option<DbOrder> = sqlx::query_as(
            r#"
            UPDATE orders
            SET data = json_set(json_set(data, '$.status', $1), '$.update_at', $2)
            WHERE id =
                (SELECT id
                FROM orders
                WHERE data->>'status' = $3
                LIMIT 1)
            RETURNING *
            "#,
        )
        .bind(OrderStatus::Proving)
        .bind(Utc::now().timestamp())
        .bind(OrderStatus::PendingProving)
        .fetch_optional(&self.pool)
        .await?;

        let Some(order) = elm else {
            return Ok(None);
        };

        Ok(Some(order.data))
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_active_proofs(&self) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> =
            sqlx::query_as("SELECT * FROM orders WHERE data->>'status' = $1")
                .bind(OrderStatus::Proving)
                .fetch_all(&self.pool)
                .await?;

        orders.into_iter().map(|elm| Ok(elm.data)).collect()
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_proof_id(&self, id: &str, proof_id: &str) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.proof_id', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(proof_id)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_compressed_proof_id(
        &self,
        id: &str,
        compressed_proof_id: &str,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.compressed_proof_id', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(compressed_proof_id)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_image_input_ids(
        &self,
        id: &str,
        image_id: &str,
        input_id: &str,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(data,
                       '$.image_id', $1),
                       '$.input_id', $2),
                       '$.updated_at', $3)
            WHERE
                id = $4"#,
        )
        .bind(image_id)
        .bind(input_id)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_aggregation_status(&self, id: &str, status: OrderStatus) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2)
            WHERE
                id = $3"#,
        )
        .bind(status)
        .bind(Utc::now().timestamp())
        .bind(id)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id.to_string()));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_aggregation_proofs(&self) -> Result<Vec<AggregationOrder>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.update_at', $2)
            WHERE
                data->>'status' IN ($3, $4)
            RETURNING *
            "#,
        )
        .bind(OrderStatus::Aggregating)
        .bind(Utc::now().timestamp())
        .bind(OrderStatus::PendingAgg)
        .bind(OrderStatus::Aggregating)
        .fetch_all(&self.pool)
        .await?;

        let mut agg_orders = vec![];
        for order in orders.into_iter() {
            agg_orders.push(AggregationOrder {
                order_id: order.id.clone(),
                // TODO(austin): https://github.com/boundless-xyz/boundless/issues/300
                proof_id: order
                    .data
                    .proof_id
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "proof_id"))?,
                expiration: order
                    .data
                    .expire_timestamp
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "expire_timestamp"))?,
                fee: order
                    .data
                    .lock_price
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "lock_price"))?,
            })
        }

        Ok(agg_orders)
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_groth16_proofs(&self) -> Result<Vec<AggregationOrder>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.update_at', $2)
            WHERE
                data->>'status' == $3
            RETURNING *
            "#,
        )
        .bind(OrderStatus::SkipAggregation)
        .bind(Utc::now().timestamp())
        .bind(OrderStatus::SkipAggregation)
        .fetch_all(&self.pool)
        .await?;

        let mut agg_orders = vec![];
        for order in orders.into_iter() {
            agg_orders.push(AggregationOrder {
                order_id: order.id.clone(),
                // TODO(austin): https://github.com/boundless-xyz/boundless/issues/300
                proof_id: order
                    .data
                    .proof_id
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "proof_id"))?,
                expiration: order
                    .data
                    .expire_timestamp
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "expire_timestamp"))?,
                fee: order.data.lock_price.ok_or(DbError::InvalidOrder(order.id, "lock_price"))?,
            })
        }

        Ok(agg_orders)
    }

    #[instrument(level = "trace", skip_all)]
    async fn complete_batch(&self, batch_id: usize, g16_proof_id: &str) -> Result<(), DbError> {
        let batch = self.get_batch(batch_id).await?;
        if batch.aggregation_state.is_none() {
            return Err(DbError::BatchAggregationStateIsNone(batch_id));
        }

        let res = sqlx::query(
            r#"
            UPDATE batches
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.aggregation_state.groth16_proof_id', $2)
            WHERE
                id = $3"#,
        )
        .bind(BatchStatus::Complete)
        .bind(g16_proof_id)
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_complete_batch(&self) -> Result<Option<(usize, Batch)>, DbError> {
        let elm: Option<DbBatch> = sqlx::query_as(
            r#"
            UPDATE batches
            SET
                data = json_set(data, '$.status', $1)
            WHERE id =
                (SELECT id
                FROM batches
                WHERE data->>'status' = $2
                LIMIT 1)
            RETURNING *
            "#,
        )
        .bind(BatchStatus::PendingSubmission)
        .bind(BatchStatus::Complete)
        .fetch_optional(&self.pool)
        .await?;

        let Some(db_batch) = elm else {
            return Ok(None);
        };

        Ok(Some((db_batch.id as usize, db_batch.data)))
    }

    #[instrument(level = "trace", skip_all)]
    async fn set_batch_submitted(&self, batch_id: usize) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE batches
            SET
                data = json_set(data, '$.status', $1)
            WHERE
                id = $2"#,
        )
        .bind(BatchStatus::Submitted)
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn set_batch_failure(&self, batch_id: usize, err: String) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE batches
            SET
                data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.error_msg', $2)
            WHERE
                id = $3"#,
        )
        .bind(BatchStatus::Failed)
        .bind(err)
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }

    #[instrument(level = "trace", skip_all)]
    async fn get_current_batch(&self) -> Result<usize, DbError> {
        let batch_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM batches").fetch_one(&self.pool).await?;

        if batch_count == 0 {
            self.new_batch().await
        } else {
            let cur_batch: Option<DbBatch> =
                sqlx::query_as("SELECT * FROM batches WHERE data->>'status' IN ($1, $2) LIMIT 1")
                    .bind(BatchStatus::Aggregating)
                    .bind(BatchStatus::PendingCompression)
                    .fetch_optional(&self.pool)
                    .await?;

            if let Some(batch) = cur_batch {
                Ok(batch.id as usize)
            } else {
                self.new_batch().await
            }
        }
    }

    #[instrument(level = "trace", skip(self, aggreagtion_state, orders, assessor_proof_id))]
    async fn update_batch(
        &self,
        batch_id: usize,
        aggreagtion_state: &AggregationState,
        orders: &[AggregationOrder],
        assessor_proof_id: Option<String>,
    ) -> Result<(), DbError> {
        let mut txn = self.pool.begin().await?;

        let rows = sqlx::query(r#"SELECT data->>'fees' as fees, data->>'deadline' as deadline FROM batches WHERE id = $1"#)
            .bind(batch_id as i64)
            .fetch_optional(&mut *txn)
            .await?;

        let Some(rows) = rows else {
            return Err(DbError::BatchNotFound(batch_id));
        };

        let db_fees: String = rows.try_get("fees")?;
        let db_deadline: Option<i64> = rows.try_get("deadline")?;

        let new_deadline = orders
            .iter()
            .fold(db_deadline, |min, order| {
                Some(i64::min(min.unwrap_or(i64::MAX), order.expiration as i64))
            })
            .unwrap_or(i64::MAX);

        let db_fees = U256::from_str(&db_fees)?;
        let new_fees = orders.iter().fold(db_fees, |sum, order| sum + order.fee);

        // Update the batch fees, deadline, and aggregation state.
        let res = sqlx::query(
            r#"
            UPDATE batches
            SET
                data = json_set(
                       json_set(
                       json_set(data,
                       '$.deadline', $1),
                       '$.fees', $2),
                       '$.aggregation_state', json($3))
            WHERE
                id = $4"#,
        )
        .bind(new_deadline)
        .bind(format!("0x{new_fees:x}"))
        .bind(sqlx::types::Json(aggreagtion_state))
        .bind(batch_id as i64)
        .execute(&mut *txn)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        // Insert all the new orders.
        for order in orders {
            let res = sqlx::query(
                r#"
                UPDATE batches
                SET
                    data = json_set(data, '$.orders', json_insert(data->>'orders', '$[#]', $1))
                WHERE
                    id = $2"#,
            )
            .bind(order.order_id.clone())
            .bind(batch_id as i64)
            .execute(&mut *txn)
            .await?;

            if res.rows_affected() == 0 {
                return Err(DbError::BatchNotFound(batch_id));
            }

            let res = sqlx::query(
                r#"
                UPDATE orders
                SET data = json_set(
                           json_set(data,
                           '$.status', $1),
                           '$.updated_at', $2)
                WHERE
                    id = $3"#,
            )
            .bind(OrderStatus::PendingSubmission)
            .bind(Utc::now().timestamp())
            .bind(order.order_id.clone())
            .execute(&mut *txn)
            .await?;

            if res.rows_affected() == 0 {
                return Err(DbError::OrderNotFound(order.order_id.clone()));
            }
        }

        if let Some(assessor_proof_id) = assessor_proof_id {
            let res = sqlx::query(
                r#"
                UPDATE batches
                SET
                    data = json_set(
                           json_set(data,
                           '$.status', $1),
                           '$.assessor_proof_id', json($2))
                WHERE
                    id = $3"#,
            )
            .bind(BatchStatus::PendingCompression)
            .bind(sqlx::types::Json(assessor_proof_id))
            .bind(batch_id as i64)
            .execute(&mut *txn)
            .await?;

            if res.rows_affected() == 0 {
                return Err(DbError::BatchNotFound(batch_id));
            }
        }

        txn.commit().await?;

        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_batch(&self, batch_id: usize) -> Result<Batch, DbError> {
        let batch: Option<DbBatch> = sqlx::query_as("SELECT * FROM batches WHERE id = $1")
            .bind(batch_id as i64)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(batch) = batch {
            Ok(batch.data)
        } else {
            Err(DbError::BatchNotFound(batch_id))
        }
    }

    async fn set_request_fulfilled(
        &self,
        request_id: &str,
        block_timestamp: u64,
        block_number: u64,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            INSERT INTO fulfilled_orders (id, block_timestamp, block_number) VALUES ($1, $2, $3)"#,
        )
        .bind(request_id)
        .bind(block_timestamp as i64)
        .bind(block_number as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn is_request_fulfilled(&self, request_id: &str) -> Result<bool, DbError> {
        let res = sqlx::query(r#"SELECT * FROM fulfilled_orders WHERE id = $1"#)
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(res.is_some())
    }

    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError> {
        let res = sqlx::query("INSERT INTO batches (id, data) VALUES ($1, $2)")
            .bind(batch_id as i64)
            .bind(sqlx::types::Json(batch))
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchInsertFailure(batch_id));
        }

        Ok(())
    }

    #[cfg(test)]
    async fn set_batch_status(&self, batch_id: usize, status: BatchStatus) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
                UPDATE batches
                SET
                    data = json_set(data,
                           '$.status', $1)
                WHERE
                    id = $2"#,
        )
        .bind(status)
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProofRequest;
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, RequestId, Requirements,
    };
    use risc0_aggregation::GuestState;
    use risc0_zkvm::sha::Digest;

    fn create_order() -> Order {
        Order {
            status: OrderStatus::New,
            updated_at: Utc::now(),
            target_timestamp: None,
            request: ProofRequest::new(
                RequestId::new(Address::ZERO, 1),
                Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                "http://risczero.com",
                Input { inputType: InputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    biddingStart: 0,
                    timeout: 100,
                    lockTimeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            compressed_proof_id: None,
            expire_timestamp: None,
            client_sig: Bytes::new(),
            lock_price: None,
            fulfillment_type: FulfillmentType::LockAndFulfill,
            error_msg: None,
        }
    }

    #[sqlx::test]
    async fn add_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order).await.unwrap();
    }

    #[sqlx::test]
    async fn order_not_exists(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        assert!(!db.order_exists_with_request_id(U256::ZERO).await.unwrap());
    }

    #[sqlx::test]
    async fn order_exists(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        assert!(db.order_exists_with_request_id(order.request.id).await.unwrap());
    }

    #[sqlx::test]
    async fn get_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();

        assert_eq!(order.request, db_order.request);
    }

    #[sqlx::test]
    async fn get_submission_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order = create_order();
        order.proof_id = Some("test".to_string());
        order.lock_price = Some(U256::from(10));
        db.add_order(order.clone()).await.unwrap();

        let submit_order: (ProofRequest, Bytes, String, B256, U256, FulfillmentType) =
            db.get_submission_order(&order.id()).await.unwrap();
        assert_eq!(submit_order.0, order.request);
        assert_eq!(submit_order.1, order.client_sig);
        assert_eq!(submit_order.2, order.proof_id.unwrap());
        assert_eq!(submit_order.3, order.request.requirements.imageId);
        assert_eq!(submit_order.4, order.lock_price.unwrap());
        assert_eq!(submit_order.5, order.fulfillment_type);
    }

    #[sqlx::test]
    async fn update_orders_for_pricing(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let mut order = create_order();
        db.add_order(order.clone()).await.unwrap();
        order.request.id = id + U256::from(2);
        db.add_order(order.clone()).await.unwrap();
        order.request.id = id + U256::from(3);
        db.add_order(order.clone()).await.unwrap();

        let price_order = db.update_orders_for_pricing(1).await.unwrap();
        assert_eq!(price_order.len(), 1);
        assert_eq!(price_order[0].status, OrderStatus::Pricing);
        assert_ne!(price_order[0].updated_at, order.updated_at);

        // Request the next two orders, which should skip the first
        let price_order = db.update_orders_for_pricing(2).await.unwrap();
        assert_eq!(price_order.len(), 2);
        assert_eq!(price_order[0].status, OrderStatus::Pricing);
        assert_eq!(price_order[1].status, OrderStatus::Pricing);
    }

    /// Create a db with two orders that were locked by others
    async fn init_db_locked_by_others(pool: SqlitePool) -> (DbObj, Order, Order) {
        let db = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order1 = create_order();
        order1.status = OrderStatus::New;
        order1.request.offer.lockTimeout = 100;
        order1.request.offer.timeout = 200;
        order1.request.id = U256::from(1);
        db.add_order(order1.clone()).await.unwrap();

        let mut order2 = create_order();
        order2.status = OrderStatus::New;
        order2.request.offer.lockTimeout = 150;
        order2.request.offer.timeout = 250;
        order2.request.id = U256::from(2);
        db.add_order(order2.clone()).await.unwrap();
        (db, order1, order2)
    }

    #[sqlx::test]
    async fn update_orders_for_pricing_updates(pool: SqlitePool) {
        // // both still locked
        let (db, order1, order2) = init_db_locked_by_others(pool).await;
        let result = db.update_orders_for_pricing(2).await.unwrap();

        // Check order status is updated
        assert_eq!(result.len(), 2);
        assert_eq!(db.get_order(&order1.id()).await.unwrap().unwrap().status, OrderStatus::Pricing);
        assert_eq!(db.get_order(&order2.id()).await.unwrap().unwrap().status, OrderStatus::Pricing);
    }

    #[sqlx::test]
    async fn get_active_pricing_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let mut order = create_order();
        order.request.id = U256::from(1);
        order.status = OrderStatus::Pricing;
        db.add_order(order.clone()).await.unwrap();

        let mut order2 = create_order();
        order2.status = OrderStatus::Pricing;
        order2.request.id = U256::from(2);
        db.add_order(order2.clone()).await.unwrap();

        let orders = db.get_active_pricing_orders().await.unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].id(), order.id());
        assert_eq!(orders[0].status, OrderStatus::Pricing);
        assert_eq!(orders[1].id(), order2.id());
        assert_eq!(orders[1].status, OrderStatus::Pricing);
    }

    #[sqlx::test]
    async fn set_order_lock(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order = create_order();
        order.request.id = U256::ZERO;
        db.add_order(order.clone()).await.unwrap();

        let lock_timestamp = 10;
        let expire_timestamp = 20;
        db.set_order_lock(&order.id(), lock_timestamp, expire_timestamp).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::FulfillAfterLocking);
        assert_eq!(db_order.target_timestamp, Some(lock_timestamp));
        assert_eq!(db_order.expire_timestamp, Some(expire_timestamp));
    }

    #[sqlx::test]
    #[should_panic(expected = "OrderNotFound(\"10\")")]
    async fn set_order_lock_fail(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();
        let bad_id = U256::from(10);
        db.set_order_lock(&bad_id.to_string(), 1, 1).await.unwrap();
    }

    async fn set_order_fulfill_after_lock_expire(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();
        let lock_timestamp = 10;
        let expire_timestamp = 20;
        db.set_order_fulfill_after_lock_expire(&order.id(), lock_timestamp, expire_timestamp)
            .await
            .unwrap();
        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::FulfillAfterLockExpire);
        assert_eq!(db_order.target_timestamp, Some(lock_timestamp));
        assert_eq!(db_order.expire_timestamp, Some(expire_timestamp));
    }

    #[sqlx::test]
    #[should_panic(expected = "OrderNotFound(\"10\")")]
    async fn set_order_fulfill_after_lock_expire_fail(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();
        let bad_id = U256::from(10);
        db.set_order_fulfill_after_lock_expire(&bad_id.to_string(), 1, 1).await.unwrap();
    }

    #[sqlx::test]
    async fn set_proving_status(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        let lock_price = U256::from(20);

        db.set_proving_status_lock_and_fulfill_orders(&order.id(), lock_price).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingProving);
        assert_eq!(db_order.lock_price, Some(lock_price));
    }

    #[sqlx::test]
    async fn get_fulfill_after_lock_expire_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        // Create orders with different target timestamps
        let mut order1 = create_order();
        order1.status = OrderStatus::FulfillAfterLockExpire;
        order1.request.id = U256::from(1);
        order1.target_timestamp = Some(10);
        db.add_order(order1.clone()).await.unwrap();

        let mut order2 = create_order();
        order2.request.id = U256::from(2);
        order2.status = OrderStatus::FulfillAfterLockExpire;
        order2.target_timestamp = Some(20);
        db.add_order(order2.clone()).await.unwrap();

        // Test with timestamp before both orders
        let orders = db.get_fulfill_after_lock_expire_orders(5).await.unwrap();
        assert_eq!(orders.len(), 0);

        // Test with timestamp between orders
        let orders = db.get_fulfill_after_lock_expire_orders(15).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id(), order1.id());

        // Test with timestamp after both orders
        let orders = db.get_fulfill_after_lock_expire_orders(25).await.unwrap();
        assert_eq!(orders.len(), 2);
        assert_eq!(orders[0].id(), order1.id());
        assert_eq!(orders[1].id(), order2.id());
    }

    #[sqlx::test]
    async fn set_proving_status_fulfill_after_lock_expire_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order = create_order();
        order.status = OrderStatus::Pricing;
        db.add_order(order.clone()).await.unwrap();

        // Order should not be updated if we query before the target timestamp.
        db.set_proving_status_fulfill_after_lock_expire_orders(&order.id()).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingProving);
        assert_eq!(db_order.lock_price, Some(U256::ZERO));
    }

    #[sqlx::test]
    async fn set_order_failure(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        let failure_str = "TEST_FAIL";
        db.set_order_failure(&order.id(), failure_str.into()).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some(failure_str.into()));
    }

    #[sqlx::test]
    async fn set_order_complete(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        db.set_order_complete(&order.id()).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Done);
    }

    #[sqlx::test]
    async fn skip_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(order.clone()).await.unwrap();

        db.skip_order(&order.id()).await.unwrap();
        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);
    }

    #[sqlx::test]
    async fn set_get_block(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let mut block_numb = 20;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);

        block_numb = 21;
        db.set_last_block(block_numb).await.unwrap();

        let db_block = db.get_last_block().await.unwrap().unwrap();
        assert_eq!(block_numb, db_block);
    }

    #[sqlx::test]
    async fn get_pending_lock_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let target_timestamp = 20;
        let good_end = 25;
        let bad_end = 15;
        let mut order1 = create_order();
        order1.status = OrderStatus::FulfillAfterLocking;
        order1.target_timestamp = Some(target_timestamp);
        order1.request.id = id;
        db.add_order(order1.clone()).await.unwrap();
        let mut order2 = create_order();
        order2.request.id = id + U256::from(1);
        order2.target_timestamp = Some(good_end + 1);
        db.add_order(order2.clone()).await.unwrap();

        let res = db.get_pending_lock_orders(good_end).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].id(), order1.id());
        assert_eq!(res[0].target_timestamp, Some(target_timestamp));

        let res = db.get_pending_lock_orders(bad_end).await.unwrap();
        assert_eq!(res.len(), 0);
    }

    #[sqlx::test]
    async fn get_proving_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::PendingProving;
        order.request.id = id;
        db.add_order(order.clone()).await.unwrap();

        let db_order = db.get_proving_order().await.unwrap();
        let db_order = db_order.unwrap();
        assert_eq!(db_order.id(), order.id());
        assert_eq!(db_order.status, OrderStatus::Proving);
    }

    #[sqlx::test]
    async fn set_order_proof_id(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.request.id = id;
        db.add_order(order.clone()).await.unwrap();

        let proof_id = "test";
        db.set_order_proof_id(&order.id(), proof_id).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.proof_id, Some(proof_id.into()));
    }

    #[sqlx::test]
    async fn get_active_proofs(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::Done;
        order.request.id = id;
        db.add_order(order.clone()).await.unwrap();

        let id_2 = U256::from(1);
        let mut order = create_order();
        order.status = OrderStatus::Proving;
        order.request.id = id_2;
        db.add_order(order.clone()).await.unwrap();

        let proving_orders = db.get_active_proofs().await.unwrap();
        assert_eq!(proving_orders.len(), 1);
        assert_eq!(proving_orders[0].id(), order.id());
    }

    #[sqlx::test]
    async fn set_image_input_ids(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::PendingProving;
        order.request.id = id;
        db.add_order(order.clone()).await.unwrap();

        let image_id = "test_img";
        let input_id = "test_input";
        db.set_image_input_ids(&order.id(), image_id, input_id).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();

        assert_eq!(db_order.image_id, Some(image_id.into()));
        assert_eq!(db_order.input_id, Some(input_id.into()));
    }

    #[sqlx::test]
    async fn set_aggregation_status(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.request.id = id;
        db.add_order(order.clone()).await.unwrap();

        db.set_aggregation_status(&order.id(), OrderStatus::PendingAgg).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();

        assert_eq!(db_order.status, OrderStatus::PendingAgg);
    }

    #[sqlx::test]
    async fn get_aggregation_proofs(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let mut orders = [
            Order {
                status: OrderStatus::New,
                proof_id: Some("test_id3".to_string()),
                expire_timestamp: Some(10),
                lock_price: Some(U256::from(10u64)),
                ..create_order()
            },
            Order {
                status: OrderStatus::PendingAgg,
                proof_id: Some("test_id1".to_string()),
                expire_timestamp: Some(10),
                lock_price: Some(U256::from(10u64)),
                ..create_order()
            },
            Order {
                status: OrderStatus::Aggregating,
                proof_id: Some("test_id2".to_string()),
                expire_timestamp: Some(10),
                lock_price: Some(U256::from(10u64)),
                ..create_order()
            },
            Order {
                status: OrderStatus::PendingSubmission,
                proof_id: Some("test_id4".to_string()),
                expire_timestamp: Some(10),
                lock_price: Some(U256::from(10u64)),
                ..create_order()
            },
        ];
        for (i, order) in orders.iter_mut().enumerate() {
            order.request.id = U256::from(i);
            db.add_order(order.clone()).await.unwrap();
        }

        let agg_proofs = db.get_aggregation_proofs().await.unwrap();

        assert_eq!(agg_proofs.len(), 2);

        let agg_proof = &agg_proofs[0];
        assert_eq!(agg_proof.order_id, orders[1].id());
        assert_eq!(agg_proof.proof_id, "test_id1");
        assert_eq!(agg_proof.expiration, 10);
        assert_eq!(agg_proof.fee, U256::from(10u64));

        let agg_proof = &agg_proofs[1];
        assert_eq!(agg_proof.order_id, orders[2].id());
        assert_eq!(agg_proof.proof_id, "test_id2");
        assert_eq!(agg_proof.expiration, 10);
        assert_eq!(agg_proof.fee, U256::from(10u64));

        let db_order = db.get_order(&agg_proofs[0].order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Aggregating);
        let db_order = db.get_order(&agg_proofs[1].order_id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Aggregating);
    }

    #[sqlx::test]
    async fn get_current_batch(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = db.get_current_batch().await.unwrap();
        assert_eq!(batch_id, 1);

        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::Aggregating);

        let batch_id = db.get_current_batch().await.unwrap();
        assert_eq!(batch_id, 1);

        db.set_batch_status(1, BatchStatus::PendingCompression).await.unwrap();

        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::PendingCompression);

        let batch_id = db.get_current_batch().await.unwrap();
        assert_eq!(batch_id, 1);
    }

    #[sqlx::test]
    async fn add_batch(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = 1;
        let batch = Batch { start_time: Utc::now(), ..Default::default() };
        db.add_batch(batch_id, batch.clone()).await.unwrap();

        let batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(batch.status, BatchStatus::Aggregating);
    }

    #[sqlx::test]
    async fn complete_batch(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = 1;
        let batch = Batch {
            aggregation_state: Some(AggregationState {
                guest_state: GuestState::initial([1u32; 8]),
                claim_digests: vec![],
                groth16_proof_id: None,
                proof_id: "a".to_string(),
            }),
            ..Default::default()
        };
        db.add_batch(batch_id, batch).await.unwrap();

        let g16_proof_id = "Testg16";
        db.complete_batch(batch_id, g16_proof_id.into()).await.unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.status, BatchStatus::Complete);
        assert_eq!(db_batch.aggregation_state.unwrap().groth16_proof_id.unwrap(), g16_proof_id);
    }

    #[sqlx::test]
    async fn get_complete_batch(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = 1;
        let batch =
            Batch { start_time: Utc::now(), status: BatchStatus::Complete, ..Default::default() };

        db.add_batch(batch_id, batch.clone()).await.unwrap();

        let (db_batch_id, db_batch) = db.get_complete_batch().await.unwrap().unwrap();
        assert_eq!(db_batch_id, batch_id);
        assert_eq!(db_batch.status, BatchStatus::PendingSubmission);
    }

    #[sqlx::test]
    async fn set_batch_submitted(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = db.get_current_batch().await.unwrap();
        db.set_batch_submitted(batch_id).await.unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.status, BatchStatus::Submitted);
    }

    #[sqlx::test]
    async fn set_batch_failure(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = db.get_current_batch().await.unwrap();
        let err_msg = "test_err";
        db.set_batch_failure(batch_id, err_msg.into()).await.unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.status, BatchStatus::Failed);
        assert_eq!(db_batch.error_msg, Some(err_msg.into()));
    }

    #[sqlx::test]
    async fn update_batch(pool: SqlitePool) {
        // Create a persistent DB for manual testing:
        //
        // let db_url = "sqlite:///tmp/test.db";
        // if !Sqlite::database_exists(db_url).await.unwrap() {
        //     Sqlite::create_database(db_url).await.unwrap()
        // }
        // let tmp_pool = SqlitePool::connect("sqlite:///tmp/test.db").await.unwrap();
        // sqlx::migrate!("./migrations").run(&tmp_pool).await.unwrap();

        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order1 = create_order();
        order1.request.id = U256::from(11);
        db.add_order(order1.clone()).await.unwrap();
        let mut order2 = create_order();
        order2.request.id = U256::from(12);
        db.add_order(order2.clone()).await.unwrap();

        let batch_id = 1;
        let agg_proofs = [
            AggregationOrder {
                proof_id: "a".to_string(),
                order_id: order1.id(),
                expiration: 20,
                fee: U256::from(5),
            },
            AggregationOrder {
                proof_id: "b".to_string(),
                order_id: order2.id(),
                expiration: 25,
                fee: U256::from(10),
            },
        ];
        let claim_digests = vec![[1u32; 8].into(), [2u32; 8].into()];
        let mut guest_state = GuestState::initial([3u32; 8]);
        guest_state.mmr.extend(&claim_digests);
        let agg_state = AggregationState {
            guest_state,
            proof_id: "c".to_string(),
            claim_digests: claim_digests.clone(),
            groth16_proof_id: None,
        };

        let base_fees = U256::from(10);
        let batch = Batch {
            start_time: Utc::now(),
            deadline: Some(100),
            fees: base_fees,
            ..Default::default()
        };

        db.add_batch(batch_id, batch.clone()).await.unwrap();
        db.update_batch(batch_id, &agg_state, &agg_proofs, Some("proof_id".to_string()))
            .await
            .unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.status, BatchStatus::PendingCompression);
        assert_eq!(db_batch.orders, vec![order1.id(), order2.id()]);
        assert_eq!(db_batch.deadline, Some(20));
        assert_eq!(db_batch.fees, U256::from(25));
        assert!(db_batch.aggregation_state.is_some());
        let agg_state = db_batch.aggregation_state.unwrap();
        assert_eq!(agg_state.groth16_proof_id.as_ref(), None);
        assert!(!agg_state.guest_state.is_initial());
        assert_eq!(&agg_state.proof_id, "c");
        assert_eq!(&agg_state.claim_digests, &claim_digests);
    }

    #[sqlx::test]
    async fn set_and_check_request_fulfilled(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let request_id = "test_request_123";
        let block_timestamp = 1000;
        let block_number = 42;

        // Initially should not be fulfilled
        assert!(!db.is_request_fulfilled(request_id).await.unwrap());

        // Set as fulfilled
        db.set_request_fulfilled(request_id, block_timestamp, block_number).await.unwrap();

        // Should now be fulfilled
        assert!(db.is_request_fulfilled(request_id).await.unwrap());

        // Different request should still not be fulfilled
        assert!(!db.is_request_fulfilled("different_request").await.unwrap());
    }
}
