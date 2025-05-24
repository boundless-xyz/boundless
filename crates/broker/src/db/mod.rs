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
    errors::{impl_coded_debug, CodedError},
    AggregationState, Batch, BatchStatus, FulfillmentType, Order, OrderRequest, OrderStatus,
    ProofRequest,
};
use tracing::instrument;

#[cfg(test)]
mod fuzz_db;

#[derive(Error)]
pub enum DbError {
    #[error("{code} Order key {0} not found in DB", code = self.code())]
    OrderNotFound(String),

    #[error("{code} Batch key {0} not found in DB", code = self.code())]
    BatchNotFound(usize),

    #[error("{code} Batch key {0} has no aggreagtion state", code = self.code())]
    BatchAggregationStateIsNone(usize),

    #[cfg(test)]
    #[error("{code} Batch insert failed {0}", code = self.code())]
    BatchInsertFailure(usize),

    #[error("{code} DB Missing column value: {0}", code = self.code())]
    MissingElm(&'static str),

    #[error("{code} SQL error {0}", code = self.code())]
    SqlErr(sqlx::Error),

    #[error("{code} SQL Pool timed out {0}", code = self.code())]
    SqlPoolTimedOut(sqlx::Error),

    #[error("{code} SQL Database locked {0}", code = self.code())]
    SqlDatabaseLocked(anyhow::Error),

    #[error("{code} SQL Unique violation {0}", code = self.code())]
    SqlUniqueViolation(sqlx::Error),

    #[error("{code} SQL Migration error", code = self.code())]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("{code} JSON serialization error", code = self.code())]
    JsonErr(#[from] serde_json::Error),

    #[error("{code} Invalid order id", code = self.code())]
    InvalidOrderId(#[from] RuintParseErr),

    #[error("{code} Invalid order id: {0} missing field: {1}", code = self.code())]
    InvalidOrder(String, &'static str),

    #[error("{code} Invalid max connection env var value", code = self.code())]
    MaxConnEnvVar(#[from] std::num::ParseIntError),
}

impl_coded_debug!(DbError);

impl CodedError for DbError {
    fn code(&self) -> &str {
        match self {
            DbError::SqlDatabaseLocked(_) => "[B-DB-001]",
            DbError::SqlPoolTimedOut(_) => "[B-DB-002]",
            DbError::SqlUniqueViolation(_) => "[B-DB-003]",
            _ => "[B-DB-500]",
        }
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        if let sqlx::Error::Database(ref db_err) = e {
            let msg = db_err.message().to_string();
            if msg.contains("database is locked") {
                return DbError::SqlDatabaseLocked(anyhow::anyhow!(msg));
            }
            if db_err.is_unique_violation() {
                return DbError::SqlUniqueViolation(e);
            }
        }
        match e {
            sqlx::Error::PoolTimedOut => DbError::SqlPoolTimedOut(e),
            _ => DbError::SqlErr(e),
        }
    }
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
    async fn insert_skipped_request(&self, order_request: &OrderRequest) -> Result<(), DbError>;
    async fn insert_accepted_request(
        &self,
        order_request: &OrderRequest,
        lock_price: U256,
    ) -> Result<Order, DbError>;
    async fn get_order(&self, id: &str) -> Result<Option<Order>, DbError>;
    async fn get_submission_order(
        &self,
        id: &str,
    ) -> Result<(ProofRequest, Bytes, String, B256, U256, FulfillmentType), DbError>;
    async fn get_order_compressed_proof_id(&self, id: &str) -> Result<String, DbError>;
    async fn set_order_failure(&self, id: &str, failure_str: &'static str) -> Result<(), DbError>;
    async fn set_order_complete(&self, id: &str) -> Result<(), DbError>;
    /// Get all orders that are committed to be prove and be fulfilled.
    async fn get_committed_orders(&self) -> Result<Vec<Order>, DbError>;
    async fn get_proving_order(&self) -> Result<Option<Order>, DbError>;
    async fn get_active_proofs(&self) -> Result<Vec<Order>, DbError>;
    async fn set_order_proof_id(&self, order_id: &str, proof_id: &str) -> Result<(), DbError>;
    async fn set_order_compressed_proof_id(
        &self,
        order_id: &str,
        proof_id: &str,
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
        request_id: U256,
        block_number: u64,
    ) -> Result<(), DbError>;
    // Checks the fulfillment table for the given request_id
    async fn is_request_fulfilled(&self, request_id: U256) -> Result<bool, DbError>;
    async fn set_request_locked(
        &self,
        request_id: U256,
        locker: &str,
        block_number: u64,
    ) -> Result<(), DbError>;
    // Checks the locked table for the given request_id
    async fn is_request_locked(&self, request_id: U256) -> Result<bool, DbError>;
    // Checks the locked table for the given request_id
    async fn get_request_locked(&self, request_id: U256) -> Result<Option<(String, u64)>, DbError>;
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
    async fn add_order(&self, order: &Order) -> Result<(), DbError>;
    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError>;
    #[cfg(test)]
    async fn set_batch_status(&self, batch_id: usize, status: BatchStatus) -> Result<(), DbError>;
}

pub type DbObj = Arc<dyn BrokerDb + Send + Sync>;

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

    async fn insert_order(&self, order: &Order) -> Result<(), DbError> {
        sqlx::query("INSERT INTO orders (id, data) VALUES ($1, $2)")
            .bind(order.id())
            .bind(sqlx::types::Json(&order))
            .execute(&self.pool)
            .await?;
        Ok(())
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

#[derive(sqlx::FromRow)]
struct DbLockedRequest {
    #[allow(dead_code)]
    id: String,
    locker: String,
    block_number: u64,
}

#[async_trait]
impl BrokerDb for SqliteDb {
    #[cfg(test)]
    #[instrument(level = "trace", skip_all, fields(id = %format!("{order}")))]
    async fn add_order(&self, order: &Order) -> Result<(), DbError> {
        self.insert_order(order).await
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{}", order_request.id())))]
    async fn insert_skipped_request(&self, order_request: &OrderRequest) -> Result<(), DbError> {
        self.insert_order(&order_request.to_skipped_order()).await
    }

    #[instrument(level = "trace", skip_all, fields(id = %format!("{}", order_request.id())))]
    async fn insert_accepted_request(
        &self,
        order_request: &OrderRequest,
        lock_price: U256,
    ) -> Result<Order, DbError> {
        let order = order_request.to_proving_order(lock_price);
        self.insert_order(&order).await?;
        Ok(order)
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

    #[instrument(level = "trace", skip_all, fields(id = %format!("{id}")))]
    async fn set_order_failure(&self, id: &str, failure_str: &'static str) -> Result<(), DbError> {
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

    #[instrument(level = "trace", skip_all)]
    async fn get_committed_orders(&self) -> Result<Vec<Order>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            "SELECT * FROM orders WHERE data->>'status' IN ($1, $2, $3, $4, $5, $6)",
        )
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

    #[instrument(level = "trace", skip(self))]
    async fn set_request_fulfilled(
        &self,
        request_id: U256,
        block_number: u64,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"
            INSERT INTO fulfilled_requests (id, block_number) VALUES ($1, $2)"#,
        )
        .bind(format!("0x{:x}", request_id))
        .bind(block_number as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn is_request_fulfilled(&self, request_id: U256) -> Result<bool, DbError> {
        let res = sqlx::query(r#"SELECT * FROM fulfilled_requests WHERE id = $1"#)
            .bind(format!("0x{:x}", request_id))
            .fetch_optional(&self.pool)
            .await?;

        Ok(res.is_some())
    }

    #[instrument(level = "trace", skip(self))]
    async fn set_request_locked(
        &self,
        request_id: U256,
        locker: &str,
        block_number: u64,
    ) -> Result<(), DbError> {
        sqlx::query(
            r#"INSERT INTO locked_requests (id, locker, block_number) VALUES ($1, $2, $3)"#,
        )
        .bind(format!("0x{:x}", request_id))
        .bind(locker)
        .bind(block_number as i64)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    #[instrument(level = "trace", skip(self))]
    async fn is_request_locked(&self, request_id: U256) -> Result<bool, DbError> {
        let res = sqlx::query(r#"SELECT * FROM locked_requests WHERE id = $1"#)
            .bind(format!("0x{:x}", request_id))
            .fetch_optional(&self.pool)
            .await?;

        Ok(res.is_some())
    }

    #[instrument(level = "trace", skip(self))]
    async fn get_request_locked(&self, request_id: U256) -> Result<Option<(String, u64)>, DbError> {
        let res: Option<DbLockedRequest> =
            sqlx::query_as(r#"SELECT * FROM locked_requests WHERE id = $1"#)
                .bind(format!("0x{:x}", request_id))
                .fetch_optional(&self.pool)
                .await?;

        Ok(res.map(|r| (r.locker, r.block_number)))
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
        Offer, Predicate, PredicateType, RequestId, RequestInput, RequestInputType, Requirements,
    };
    use risc0_aggregation::GuestState;
    use risc0_zkvm::sha::Digest;

    fn create_order_request() -> OrderRequest {
        OrderRequest::new(
            ProofRequest::new(
                RequestId::new(Address::ZERO, 1),
                Requirements::new(
                    Digest::ZERO,
                    Predicate {
                        predicateType: PredicateType::PrefixMatch,
                        data: Default::default(),
                    },
                ),
                "http://risczero.com",
                RequestInput { inputType: RequestInputType::Inline, data: "".into() },
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
            Bytes::new(),
            FulfillmentType::LockAndFulfill,
            Address::ZERO,
            1,
        )
    }

    fn create_order() -> Order {
        create_order_request().to_proving_order(Default::default())
    }

    #[sqlx::test]
    async fn add_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order_request();
        db.insert_accepted_request(&order, U256::ZERO).await.unwrap();
    }

    #[sqlx::test]
    async fn get_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(&order).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();

        assert_eq!(order.request, db_order.request);
    }

    #[sqlx::test]
    async fn get_submission_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let mut order = create_order();
        order.proof_id = Some("test".to_string());
        order.lock_price = Some(U256::from(10));
        db.add_order(&order).await.unwrap();

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
    async fn set_order_failure(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(&order).await.unwrap();

        let failure_str = "TEST_FAIL";
        db.set_order_failure(&order.id(), failure_str).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some(failure_str.into()));
    }

    #[sqlx::test]
    async fn set_order_complete(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order();
        db.add_order(&order).await.unwrap();

        db.set_order_complete(&order.id()).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Done);
    }

    #[sqlx::test]
    async fn skip_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let order = create_order_request();

        db.insert_skipped_request(&order).await.unwrap();
        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Skipped);
    }

    #[sqlx::test]
    async fn get_proving_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::PendingProving;
        order.request.id = id;
        db.add_order(&order).await.unwrap();

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
        db.add_order(&order).await.unwrap();

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
        db.add_order(&order).await.unwrap();

        let id_2 = U256::from(1);
        let mut order = create_order();
        order.status = OrderStatus::Proving;
        order.request.id = id_2;
        db.add_order(&order).await.unwrap();

        let proving_orders = db.get_active_proofs().await.unwrap();
        assert_eq!(proving_orders.len(), 1);
        assert_eq!(proving_orders[0].id(), order.id());
    }

    #[sqlx::test]
    async fn set_aggregation_status(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.request.id = id;
        db.add_order(&order).await.unwrap();

        db.set_aggregation_status(&order.id(), OrderStatus::PendingAgg).await.unwrap();

        let db_order = db.get_order(&order.id()).await.unwrap().unwrap();

        assert_eq!(db_order.status, OrderStatus::PendingAgg);
    }

    #[sqlx::test]
    async fn get_aggregation_proofs(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let mut orders = [
            Order {
                status: OrderStatus::PendingProving,
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
            db.add_order(order).await.unwrap();
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
        db.complete_batch(batch_id, g16_proof_id).await.unwrap();

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
        db.add_order(&order1).await.unwrap();
        let mut order2 = create_order();
        order2.request.id = U256::from(12);
        db.add_order(&order2).await.unwrap();

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

        let request_id = U256::from(123);
        let block_number = 42;

        // Initially should not be fulfilled
        assert!(!db.is_request_fulfilled(request_id).await.unwrap());

        // Set as fulfilled
        db.set_request_fulfilled(request_id, block_number).await.unwrap();

        // Should now be fulfilled
        assert!(db.is_request_fulfilled(request_id).await.unwrap());

        // Different request should still not be fulfilled
        assert!(!db.is_request_fulfilled(U256::from(413)).await.unwrap());
    }

    #[sqlx::test]
    async fn set_and_check_request_locked(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let request_id = U256::from(123);
        let locker = "test_locker";
        let block_number = 42;
        // Initially should not be locked
        assert!(!db.is_request_locked(request_id).await.unwrap());

        // Set as locked
        db.set_request_locked(request_id, locker, block_number).await.unwrap();

        // Should now be locked
        assert!(db.is_request_locked(request_id).await.unwrap());

        // Different request should still not be locked
        assert!(!db.is_request_locked(U256::from(413)).await.unwrap());
    }
}
