// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{default::Default, str::FromStr, sync::Arc};

use alloy::primitives::{ruint::ParseError as RuintParseErr, B256, U256};
use async_trait::async_trait;
use chrono::Utc;
use risc0_zkvm::sha::Digest;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use thiserror::Error;

use crate::{Batch, BatchStatus, Node, Order, OrderStatus, ProofRequest};

#[derive(Error, Debug)]
pub enum DbError {
    #[error("Order key {0} not found in DB")]
    OrderNotFound(U256),

    #[error("Batch key {0} not found in DB")]
    BatchNotFound(usize),

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

pub struct AggregationProofs {
    pub order_id: U256,
    pub proof_id: String,
    pub expire_block: u64,
    pub fee: U256,
}

#[async_trait]
pub trait BrokerDb {
    async fn add_order(&self, id: U256, order: Order) -> Result<Option<Order>, DbError>;
    async fn order_exists(&self, id: U256) -> Result<bool, DbError>;
    async fn get_order(&self, id: U256) -> Result<Option<Order>, DbError>;
    async fn get_submission_order(
        &self,
        id: U256,
    ) -> Result<(ProofRequest, String, B256, Vec<Digest>, U256), DbError>;
    async fn get_order_for_pricing(&self) -> Result<Option<(U256, Order)>, DbError>;
    async fn get_active_pricing_orders(&self) -> Result<Vec<(U256, Order)>, DbError>;
    async fn set_order_lock(
        &self,
        id: U256,
        lock_block: u64,
        expire_block: u64,
    ) -> Result<(), DbError>;
    async fn set_proving_status(&self, id: U256, lock_price: U256) -> Result<(), DbError>;
    async fn set_order_failure(&self, id: U256, failure_str: String) -> Result<(), DbError>;
    async fn set_order_complete(&self, id: U256) -> Result<(), DbError>;
    async fn set_order_path(&self, id: U256, path: Vec<Digest>) -> Result<(), DbError>;
    async fn skip_order(&self, id: U256) -> Result<(), DbError>;
    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;
    async fn get_pending_lock_orders(&self, end_block: u64) -> Result<Vec<(U256, Order)>, DbError>;
    async fn get_orders_committed_to_fulfil_count(&self) -> Result<u64, DbError>;
    async fn get_proving_order(&self) -> Result<Option<(U256, Order)>, DbError>;
    async fn get_active_proofs(&self) -> Result<Vec<(U256, Order)>, DbError>;
    async fn set_order_proof_id(&self, order_id: U256, proof_id: &str) -> Result<(), DbError>;
    async fn set_image_input_ids(
        &self,
        id: U256,
        image_id: &str,
        input_id: &str,
    ) -> Result<(), DbError>;
    async fn set_aggregation_status(&self, id: U256) -> Result<(), DbError>;
    async fn get_aggregation_proofs(&self) -> Result<Vec<AggregationProofs>, DbError>;
    async fn complete_batch(
        &self,
        batch_id: usize,
        root: Digest,
        orders_root: Digest,
        g16_proof_id: String,
    ) -> Result<(), DbError>;
    async fn get_complete_batch(&self) -> Result<Option<(usize, Batch)>, DbError>;
    async fn set_batch_submitted(&self, batch_id: usize) -> Result<(), DbError>;
    async fn set_batch_failure(&self, batch_id: usize, err: String) -> Result<(), DbError>;
    async fn get_current_batch(&self) -> Result<usize, DbError>;
    async fn update_batch(
        &self,
        batch_id: usize,
        order_id: U256,
        expire_block: u64,
        fees: U256,
    ) -> Result<(), DbError>;
    async fn get_batch(&self, batch_id: usize) -> Result<Batch, DbError>;
    async fn set_batch_peaks(&self, batch_id: usize, peaks: Vec<Node>) -> Result<(), DbError>;
    async fn get_batch_peaks(&self, batch_id: usize) -> Result<Vec<Node>, DbError>;
    async fn get_batch_peak_count(&self, batch_id: usize) -> Result<usize, DbError>;

    #[cfg(test)]
    async fn add_batch(&self, batch_id: usize, batch: Batch) -> Result<(), DbError>;
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
    async fn add_order(&self, id: U256, order: Order) -> Result<Option<Order>, DbError> {
        sqlx::query("INSERT INTO orders (id, data) VALUES ($1, $2)")
            .bind(format!("{id:x}"))
            .bind(sqlx::types::Json(&order))
            .execute(&self.pool)
            .await?;
        Ok(Some(order))
    }

    async fn order_exists(&self, id: U256) -> Result<bool, DbError> {
        let res: i64 = sqlx::query_scalar("SELECT COUNT(1) FROM orders WHERE id = $1")
            .bind(format!("{id:x}"))
            .fetch_one(&self.pool)
            .await?;

        Ok(res == 1)
    }

    async fn get_order(&self, id: U256) -> Result<Option<Order>, DbError> {
        let order: Option<DbOrder> = sqlx::query_as("SELECT * FROM orders WHERE id = $1 LIMIT 1")
            .bind(format!("{id:x}"))
            .fetch_optional(&self.pool)
            .await?;

        Ok(order.map(|x| x.data))
    }

    async fn get_submission_order(
        &self,
        id: U256,
    ) -> Result<(ProofRequest, String, B256, Vec<Digest>, U256), DbError> {
        let order = self.get_order(id).await?;
        if let Some(order) = order {
            Ok((
                order.request.clone(),
                order.proof_id.ok_or(DbError::MissingElm("proof_id"))?,
                order.request.requirements.imageId,
                order.path.ok_or(DbError::MissingElm("path"))?,
                order.lock_price.ok_or(DbError::MissingElm("lock_price"))?,
            ))
        } else {
            Err(DbError::OrderNotFound(id))
        }
    }

    async fn get_order_for_pricing(&self) -> Result<Option<(U256, Order)>, DbError> {
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
        .bind(OrderStatus::Pricing)
        .bind(Utc::now().timestamp())
        .bind(OrderStatus::New)
        .fetch_optional(&self.pool)
        .await?;

        let Some(order) = elm else {
            return Ok(None);
        };

        Ok(Some((U256::from_str_radix(&order.id, 16)?, order.data)))
    }

    async fn get_active_pricing_orders(&self) -> Result<Vec<(U256, Order)>, DbError> {
        let orders: Vec<DbOrder> =
            sqlx::query_as("SELECT * FROM orders WHERE data->>'status' = $1")
                .bind(OrderStatus::Pricing)
                .fetch_all(&self.pool)
                .await?;

        let orders: Result<Vec<_>, _> = orders
            .into_iter()
            .map(|elm| Ok((U256::from_str_radix(&elm.id, 16)?, elm.data)))
            .collect();

        orders
    }

    async fn set_order_lock(
        &self,
        id: U256,
        lock_block: u64,
        expire_block: u64,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.target_block', $2),
                       '$.expire_block', $3),
                       '$.updated_at', $4)
            WHERE
                id = $5"#,
        )
        .bind(OrderStatus::Locking)
        // TODO: can we work out how to correctly
        // use bind + a json field with out string formatting
        // the sql query?
        .bind(lock_block as i64)
        .bind(expire_block as i64)
        .bind(Utc::now().timestamp())
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_proving_status(&self, id: U256, lock_price: U256) -> Result<(), DbError> {
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
        .bind(OrderStatus::Locked)
        .bind(Utc::now().timestamp())
        .bind(lock_price.to_string())
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_order_failure(&self, id: U256, failure_str: String) -> Result<(), DbError> {
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
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_order_complete(&self, id: U256) -> Result<(), DbError> {
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
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_order_path(&self, id: U256, path: Vec<Digest>) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.updated_at', $2),
                       '$.path', json($3))
            WHERE
                id = $4"#,
        )
        .bind(OrderStatus::PendingSubmission)
        .bind(Utc::now().timestamp())
        .bind(sqlx::types::Json(path))
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn skip_order(&self, id: U256) -> Result<(), DbError> {
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
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

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

    async fn get_pending_lock_orders(&self, end_block: u64) -> Result<Vec<(U256, Order)>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            "SELECT * FROM orders WHERE data->>'status' = $1 AND data->>'target_block' <= $2",
        )
        .bind(OrderStatus::Locking)
        .bind(end_block as i64)
        .fetch_all(&self.pool)
        .await?;

        // Break if any order-id's are invalid and raise
        let orders: Result<Vec<_>, _> = orders
            .into_iter()
            .map(|elm| Ok((U256::from_str_radix(&elm.id, 16)?, elm.data)))
            .collect();

        orders
    }

    async fn get_orders_committed_to_fulfil_count(&self) -> Result<u64, DbError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orders WHERE data->>'status' >= $1 AND data->>'status' <= $2",
        )
        .bind(OrderStatus::Locking)
        .bind(OrderStatus::PendingSubmission)
        .fetch_one(&self.pool)
        .await?;

        Ok(count as u64)
    }

    async fn get_proving_order(&self) -> Result<Option<(U256, Order)>, DbError> {
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
        .bind(OrderStatus::Locked)
        .fetch_optional(&self.pool)
        .await?;

        let Some(order) = elm else {
            return Ok(None);
        };

        Ok(Some((U256::from_str_radix(&order.id, 16)?, order.data)))
    }

    async fn get_active_proofs(&self) -> Result<Vec<(U256, Order)>, DbError> {
        let orders: Vec<DbOrder> =
            sqlx::query_as("SELECT * FROM orders WHERE data->>'status' = $1")
                .bind(OrderStatus::Proving)
                .fetch_all(&self.pool)
                .await?;

        let orders: Result<Vec<_>, _> = orders
            .into_iter()
            .map(|elm| Ok((U256::from_str_radix(&elm.id, 16)?, elm.data)))
            .collect();

        orders
    }

    async fn set_order_proof_id(&self, id: U256, proof_id: &str) -> Result<(), DbError> {
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
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_image_input_ids(
        &self,
        id: U256,
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
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn set_aggregation_status(&self, id: U256) -> Result<(), DbError> {
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
        .bind(OrderStatus::PendingAgg)
        .bind(Utc::now().timestamp())
        .bind(format!("{id:x}"))
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::OrderNotFound(id));
        }

        Ok(())
    }

    async fn get_aggregation_proofs(&self) -> Result<Vec<AggregationProofs>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as(
            r#"
            UPDATE orders
            SET data = json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.update_at', $2)
            WHERE
                data->>'status' = $3
            RETURNING *
            "#,
        )
        .bind(OrderStatus::Aggregating)
        .bind(Utc::now().timestamp())
        .bind(OrderStatus::PendingAgg)
        .fetch_all(&self.pool)
        .await?;

        let mut agg_orders = vec![];
        for order in orders.into_iter() {
            agg_orders.push(AggregationProofs {
                order_id: U256::from_str_radix(&order.id, 16)?,
                proof_id: order
                    .data
                    .proof_id
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "proof_id"))?,
                expire_block: order
                    .data
                    .expire_block
                    .ok_or(DbError::InvalidOrder(order.id.clone(), "expire_block"))?,
                fee: order.data.lock_price.ok_or(DbError::InvalidOrder(order.id, "lock_price"))?,
            })
        }

        Ok(agg_orders)
    }

    async fn complete_batch(
        &self,
        batch_id: usize,
        root: Digest,
        orders_root: Digest,
        g16_proof_id: String,
    ) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE batches
            SET data = json_set(
                       json_set(
                       json_set(
                       json_set(data,
                       '$.status', $1),
                       '$.root', json($2)),
                       '$.orders_root', json($3)),
                       '$.groth16_proof_id', $4)
            WHERE
                id = $5"#,
        )
        .bind(BatchStatus::Complete)
        .bind(sqlx::types::Json(root))
        .bind(sqlx::types::Json(orders_root))
        .bind(g16_proof_id)
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }

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

    async fn get_current_batch(&self) -> Result<usize, DbError> {
        let batch_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM batches").fetch_one(&self.pool).await?;

        if batch_count == 0 {
            self.new_batch().await
        } else {
            let cur_batch: Option<DbBatch> =
                sqlx::query_as("SELECT * FROM batches WHERE data->>'status' = $1 LIMIT 1")
                    .bind(BatchStatus::Aggregating)
                    .fetch_optional(&self.pool)
                    .await?;

            if let Some(batch) = cur_batch {
                Ok(batch.id as usize)
            } else {
                self.new_batch().await
            }
        }
    }

    async fn update_batch(
        &self,
        batch_id: usize,
        order_id: U256,
        expire_block: u64,
        fees: U256,
    ) -> Result<(), DbError> {
        let mut txn = self.pool.begin().await?;

        let rows = sqlx::query(r#"SELECT data->>'fees' as fees, data->>'block_deadline' as deadline FROM batches WHERE id = $1"#)
            .bind(batch_id as i64)
            .fetch_optional(&mut *txn)
            .await?;

        let Some(rows) = rows else {
            return Err(DbError::BatchNotFound(batch_id));
        };

        let db_fees: String = rows.try_get("fees")?;
        let db_deadline_res: Option<i64> = rows.try_get("deadline")?;

        let new_deadline: i64 = if let Some(db_deadline) = db_deadline_res {
            if (expire_block as i64) < db_deadline {
                expire_block as i64
            } else {
                db_deadline
            }
        } else {
            expire_block as i64
        };

        let db_fees = U256::from_str(&db_fees)?;
        let new_fees = db_fees + fees;

        let res = sqlx::query(
            r#"
            UPDATE batches
            SET
                data = json_set(
                       json_set(
                       json_set(data,
                       '$.orders', json_insert(data->>'orders', '$[#]', $1)),
                       '$.block_deadline', $2),
                       '$.fees', $3)
            WHERE
                id = $4"#,
        )
        .bind(format!("0x{order_id:x}"))
        .bind(new_deadline)
        .bind(format!("0x{new_fees:x}"))
        .bind(batch_id as i64)
        .execute(&mut *txn)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        txn.commit().await?;

        Ok(())
    }

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

    async fn set_batch_peaks(&self, batch_id: usize, peaks: Vec<Node>) -> Result<(), DbError> {
        let res = sqlx::query(
            r#"
            UPDATE batches
            SET
                data = json_set(data, '$.peaks', json($1))
            WHERE
                id = $2"#,
        )
        .bind(sqlx::types::Json(peaks))
        .bind(batch_id as i64)
        .execute(&self.pool)
        .await?;

        if res.rows_affected() == 0 {
            return Err(DbError::BatchNotFound(batch_id));
        }

        Ok(())
    }

    async fn get_batch_peaks(&self, batch_id: usize) -> Result<Vec<Node>, DbError> {
        let res = sqlx::query("SELECT data->>'peaks' as peaks FROM batches WHERE id = $1")
            .bind(batch_id as i64)
            .fetch_optional(&self.pool)
            .await?;

        let Some(rows) = res else {
            return Err(DbError::BatchNotFound(batch_id));
        };
        let peaks: sqlx::types::Json<Vec<Node>> = rows.try_get("peaks")?;

        Ok(peaks.0)
    }

    async fn get_batch_peak_count(&self, batch_id: usize) -> Result<usize, DbError> {
        let count: Option<i64> = sqlx::query_scalar(
            "SELECT json_array_length(data->>'peaks') FROM batches WHERE id = $1",
        )
        .bind(batch_id as i64)
        .fetch_optional(&self.pool)
        .await?;
        let count = count.unwrap_or(0);

        Ok(count as usize)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProofRequest;
    use alloy::primitives::{Address, Bytes, U256};
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, Requirements,
    };
    use risc0_zkvm::sha::DIGEST_WORDS;

    fn create_order() -> Order {
        Order {
            status: OrderStatus::New,
            updated_at: Utc::now(),
            target_block: None,
            request: ProofRequest::new(
                1,
                &Address::ZERO,
                Requirements {
                    imageId: Default::default(),
                    predicate: Predicate {
                        predicateType: PredicateType::DigestMatch,
                        data: B256::ZERO.into(),
                    },
                },
                "http://risczero.com",
                Input { inputType: InputType::Inline, data: "".into() },
                Offer {
                    minPrice: U256::from(1),
                    maxPrice: U256::from(2),
                    biddingStart: 0,
                    timeout: 100,
                    rampUpPeriod: 1,
                    lockStake: U256::from(0),
                },
            ),
            image_id: None,
            input_id: None,
            proof_id: None,
            expire_block: None,
            path: None,
            client_sig: Bytes::new(),
            lock_price: None,
            error_msg: None,
        }
    }

    #[sqlx::test]
    async fn add_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order).await.unwrap();
    }

    #[sqlx::test]
    async fn order_not_exists(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        assert!(!db.order_exists(U256::ZERO).await.unwrap());
    }

    #[sqlx::test]
    async fn order_exists(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order).await.unwrap();

        assert!(db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn get_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();

        assert_eq!(order.request, db_order.request);
    }

    #[sqlx::test]
    async fn get_submission_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let mut order = create_order();
        order.path = Some(vec![Digest::default()]);
        order.proof_id = Some("test".to_string());
        order.lock_price = Some(U256::from(10));
        db.add_order(id, order.clone()).await.unwrap();

        let submit_order = db.get_submission_order(id).await.unwrap();
        assert_eq!(submit_order.0, order.request);
        assert_eq!(submit_order.1, order.proof_id.unwrap());
        assert_eq!(submit_order.2, order.request.requirements.imageId);
        assert_eq!(submit_order.3, order.path.unwrap());
        assert_eq!(submit_order.4, order.lock_price.unwrap());
    }

    #[sqlx::test]
    async fn get_order_for_pricing(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let price_order = db.get_order_for_pricing().await.unwrap();
        let price_order = price_order.unwrap();
        assert_eq!(price_order.0, id);
        assert_eq!(price_order.1.status, OrderStatus::Pricing);
        assert_ne!(price_order.1.updated_at, order.updated_at);
    }

    #[sqlx::test]
    async fn get_active_pricing_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::Pricing;
        db.add_order(id, order.clone()).await.unwrap();

        let orders = db.get_active_pricing_orders().await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].0, id);
        assert_eq!(orders[0].1.status, OrderStatus::Pricing);
    }

    #[sqlx::test]
    async fn set_order_lock(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let lock_block = 10;
        let expire_block = 20;
        db.set_order_lock(id, lock_block, expire_block).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locking);
        assert_eq!(db_order.target_block, Some(lock_block));
        assert_eq!(db_order.expire_block, Some(expire_block));
    }

    #[sqlx::test]
    #[should_panic(expected = "OrderNotFound(1)")]
    async fn set_order_lock_fail(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();
        let bad_id = U256::from(1);
        db.set_order_lock(bad_id, 1, 1).await.unwrap();
    }

    #[sqlx::test]
    async fn set_proving_status(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let lock_price = U256::from(20);

        db.set_proving_status(id, lock_price).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Locked);
        assert_eq!(db_order.lock_price, Some(lock_price));
    }

    #[sqlx::test]
    async fn set_order_failure(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let failure_str = "TEST_FAIL";
        db.set_order_failure(id, failure_str.into()).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Failed);
        assert_eq!(db_order.error_msg, Some(failure_str.into()));
    }

    #[sqlx::test]
    async fn set_order_complete(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        db.set_order_complete(id).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::Done);
    }

    #[sqlx::test]
    async fn set_order_path(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let path = vec![Digest::new([1; DIGEST_WORDS])];
        db.set_order_path(id, path.clone()).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.status, OrderStatus::PendingSubmission);
        assert_eq!(db_order.path, Some(path));
    }

    #[sqlx::test]
    async fn skip_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        db.skip_order(id).await.unwrap();
        let db_order = db.get_order(id).await.unwrap().unwrap();
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
        let target_block = 20;
        let good_end = 25;
        let bad_end = 15;
        let mut order = create_order();
        order.status = OrderStatus::Locking;
        order.target_block = Some(target_block);
        db.add_order(id, order.clone()).await.unwrap();
        order.target_block = Some(good_end + 1);
        db.add_order(id + U256::from(1), order.clone()).await.unwrap();

        let res = db.get_pending_lock_orders(good_end).await.unwrap();

        assert_eq!(res.len(), 1);
        assert_eq!(res[0].0, id);
        assert_eq!(res[0].1.target_block, Some(target_block));

        let res = db.get_pending_lock_orders(bad_end).await.unwrap();
        assert_eq!(res.len(), 0);
    }

    #[sqlx::test]
    async fn get_proving_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::Locked;
        db.add_order(id, order.clone()).await.unwrap();

        let db_order = db.get_proving_order().await.unwrap();
        let db_order = db_order.unwrap();
        assert_eq!(db_order.0, id);
        assert_eq!(db_order.1.status, OrderStatus::Proving);
    }

    #[sqlx::test]
    async fn set_order_proof_id(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        let proof_id = "test";
        db.set_order_proof_id(id, proof_id).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();
        assert_eq!(db_order.proof_id, Some(proof_id.into()));
    }

    #[sqlx::test]
    async fn get_active_proofs(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::Done;
        db.add_order(id, order.clone()).await.unwrap();

        let id_2 = U256::from(1);
        let mut order = create_order();
        order.status = OrderStatus::Proving;
        db.add_order(id_2, order.clone()).await.unwrap();

        let proving_orders = db.get_active_proofs().await.unwrap();
        assert_eq!(proving_orders.len(), 1);
        assert_eq!(proving_orders[0].0, id_2);
    }

    #[sqlx::test]
    async fn set_image_input_ids(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let mut order = create_order();
        order.status = OrderStatus::Locked;
        db.add_order(id, order.clone()).await.unwrap();

        let image_id = "test_img";
        let input_id = "test_input";
        db.set_image_input_ids(id, image_id, input_id).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();

        assert_eq!(db_order.image_id, Some(image_id.into()));
        assert_eq!(db_order.input_id, Some(input_id.into()));
    }

    #[sqlx::test]
    async fn set_aggregation_status(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let order = create_order();
        db.add_order(id, order.clone()).await.unwrap();

        db.set_aggregation_status(id).await.unwrap();

        let db_order = db.get_order(id).await.unwrap().unwrap();

        assert_eq!(db_order.status, OrderStatus::PendingAgg);
    }

    #[sqlx::test]
    async fn get_aggregation_proofs(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let id = U256::ZERO;
        let proof_id = "test_id";
        let expire_block = 10;
        let fee = U256::from(10);
        let mut order = create_order();
        order.status = OrderStatus::PendingAgg;
        order.proof_id = Some(proof_id.into());
        order.expire_block = Some(expire_block);
        order.lock_price = Some(fee);
        db.add_order(id, order.clone()).await.unwrap();

        let agg_proofs = db.get_aggregation_proofs().await.unwrap();

        assert_eq!(agg_proofs.len(), 1);
        let agg_proof = &agg_proofs[0];

        assert_eq!(agg_proof.order_id, id);
        assert_eq!(agg_proof.proof_id, proof_id);
        assert_eq!(agg_proof.expire_block, expire_block);
        assert_eq!(agg_proof.fee, fee);

        let db_order = db.get_order(id).await.unwrap().unwrap();
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

        let batch_id = db.get_current_batch().await.unwrap();

        let root = Digest::new([1; DIGEST_WORDS]);
        let orders_root = Digest::new([2; DIGEST_WORDS]);
        let g16_proof_id = "Testg16";
        db.complete_batch(batch_id, root, orders_root, g16_proof_id.into()).await.unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.status, BatchStatus::Complete);
        assert_eq!(db_batch.root, Some(root));
        assert_eq!(db_batch.orders_root, Some(orders_root));
        assert_eq!(db_batch.groth16_proof_id, g16_proof_id);
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

        let batch_id = 1;
        let order_id = U256::from(1);
        let expire_block = 20;

        let base_fees = U256::from(10);
        let new_fees = U256::from(5);
        let batch = Batch {
            start_time: Utc::now(),
            block_deadline: Some(100),
            fees: base_fees,
            ..Default::default()
        };

        db.add_batch(batch_id, batch.clone()).await.unwrap();
        db.update_batch(batch_id, order_id, expire_block, new_fees).await.unwrap();

        let db_batch = db.get_batch(batch_id).await.unwrap();
        assert_eq!(db_batch.orders, vec![order_id]);
        assert_eq!(db_batch.block_deadline, Some(expire_block));
        assert_eq!(db_batch.fees, base_fees + new_fees);
    }

    #[sqlx::test]
    async fn set_batch_peaks(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let batch_id = 1;
        let batch = Batch { start_time: Utc::now(), ..Default::default() };
        db.add_batch(batch_id, batch.clone()).await.unwrap();

        let proof_id = "test";
        let order_id = U256::from(1);
        let root = Digest::new([1; DIGEST_WORDS]);
        db.set_batch_peaks(
            batch_id,
            vec![Node::Singleton { proof_id: proof_id.into(), order_id, root }],
        )
        .await
        .unwrap();
    }

    #[sqlx::test]
    async fn get_batch_peaks(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = 1;
        let batch = Batch { start_time: Utc::now(), ..Default::default() };
        db.add_batch(batch_id, batch.clone()).await.unwrap();

        let proof_id = "test";
        let order_id = U256::from(1);
        let root = Digest::new([1; DIGEST_WORDS]);
        let peaks = vec![Node::Singleton { proof_id: proof_id.into(), order_id, root }];
        db.set_batch_peaks(batch_id, peaks.clone()).await.unwrap();

        let db_peaks = db.get_batch_peaks(batch_id).await.unwrap();
        assert_eq!(db_peaks, peaks);
    }

    #[sqlx::test]
    async fn get_batch_peak_count(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());

        let batch_id = 1;
        let batch = Batch { start_time: Utc::now(), ..Default::default() };
        db.add_batch(batch_id, batch.clone()).await.unwrap();
        assert_eq!(db.get_batch_peak_count(batch_id).await.unwrap(), 0);

        let proof_id = "test";
        let order_id = U256::from(1);
        let root = Digest::new([1; DIGEST_WORDS]);
        let peaks = vec![Node::Singleton { proof_id: proof_id.into(), order_id, root }];
        db.set_batch_peaks(batch_id, peaks.clone()).await.unwrap();

        assert_eq!(db.get_batch_peak_count(batch_id).await.unwrap(), 1);
    }
}
