// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{str::FromStr, sync::Arc};

use alloy::primitives::U256;
use async_trait::async_trait;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions},
    Row,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum DbError {
    #[error("SQL error: {0}")]
    SqlErr(#[from] sqlx::Error),

    #[error("SQL Migration error: {0}")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("Invalid block number: {0}")]
    BadBlockNumb(String),

    #[error("Failed to set last block")]
    SetBlockFail,
}

#[async_trait]
pub trait SlasherDb {
    async fn add_order(
        &self,
        id: U256,
        expires_at: u64,
        lock_expires_at: u64,
    ) -> Result<(), DbError>;
    async fn get_order(&self, id: U256) -> Result<Option<(u64, u64)>, DbError>; // (expires_at, lock_expires_at)
    async fn remove_order(&self, id: U256) -> Result<(), DbError>;
    async fn order_exists(&self, id: U256) -> Result<bool, DbError>;
    async fn get_expired_orders(&self, current_timestamp: u64) -> Result<Vec<U256>, DbError>;

    async fn get_last_block(&self) -> Result<Option<u64>, DbError>;
    async fn set_last_block(&self, block_numb: u64) -> Result<(), DbError>;
}

pub type DbObj = Arc<dyn SlasherDb + Send + Sync>;

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
}

#[derive(sqlx::FromRow)]
struct DbOrder {
    id: String,
}

#[async_trait]
impl SlasherDb for SqliteDb {
    async fn add_order(
        &self,
        id: U256,
        expires_at: u64,
        lock_expires_at: u64,
    ) -> Result<(), DbError> {
        tracing::trace!("Adding order: 0x{:x}", id);
        // Only store the order if it has a valid expiration time.
        // If the expires_at is 0, the request is already slashed or fulfilled (or even not locked).
        if expires_at > 0 && lock_expires_at > 0 && !self.order_exists(id).await? {
            sqlx::query("INSERT INTO orders (id, expires_at, lock_expires_at) VALUES ($1, $2, $3)")
                .bind(format!("{id:x}"))
                .bind(expires_at as i64)
                .bind(lock_expires_at as i64)
                .execute(&self.pool)
                .await?;
        }
        Ok(())
    }

    async fn get_order(&self, id: U256) -> Result<Option<(u64, u64)>, DbError> {
        tracing::trace!("Getting order: 0x{:x}", id);
        let res = sqlx::query("SELECT expires_at, lock_expires_at FROM orders WHERE id = $1")
            .bind(format!("{id:x}"))
            .fetch_optional(&self.pool)
            .await?;

        if let Some(row) = res {
            let expires_at: i64 = row.try_get("expires_at")?;
            let lock_expires_at: i64 = row.try_get("lock_expires_at")?;
            Ok(Some((expires_at as u64, lock_expires_at as u64)))
        } else {
            Ok(None)
        }
    }

    async fn remove_order(&self, id: U256) -> Result<(), DbError> {
        tracing::trace!("Removing order: 0x{:x}", id);
        sqlx::query("DELETE FROM orders WHERE id = $1")
            .bind(format!("{id:x}"))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn order_exists(&self, id: U256) -> Result<bool, DbError> {
        let res = sqlx::query_scalar::<_, i64>("SELECT COUNT(1) FROM orders WHERE id = $1")
            .bind(format!("{id:x}"))
            .fetch_one(&self.pool)
            .await;

        match res {
            Ok(count) => Ok(count == 1),
            Err(sqlx::Error::RowNotFound) => Ok(false),
            Err(e) => Err(DbError::from(e)),
        }
    }

    async fn get_expired_orders(&self, current_timestamp: u64) -> Result<Vec<U256>, DbError> {
        let orders: Vec<DbOrder> = sqlx::query_as("SELECT id FROM orders WHERE $1 > expires_at")
            .bind(current_timestamp as i64)
            .fetch_all(&self.pool)
            .await?;

        Ok(orders
            .into_iter()
            .map(|x| U256::from_str_radix(&x.id, 16).map_err(|e| sqlx::Error::Decode(Box::new(e))))
            .collect::<Result<Vec<_>, sqlx::Error>>()?)
    }

    async fn get_last_block(&self) -> Result<Option<u64>, DbError> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    #[sqlx::test]
    async fn add_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        db.add_order(id, 10, 5).await.unwrap();

        // Adding the same order should not fail
        db.add_order(id, 10, 5).await.unwrap();

        // Adding an order slashed or fulfilled should not store it
        let id = U256::from(1);
        db.add_order(id, 0, 0).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn drop_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        db.add_order(id, 10, 5).await.unwrap();
        db.remove_order(id).await.unwrap();
        // Removing the same order should not fail
        db.remove_order(id).await.unwrap();
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
        db.add_order(id, 10, 5).await.unwrap();

        assert!(db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn get_expired_orders(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let expires_at = 10;
        db.add_order(id, expires_at, 5).await.unwrap();

        // Order should expires AFTER the `expires_at` block
        let expired = db.get_expired_orders(expires_at).await.unwrap();
        assert!(expired.is_empty());

        let db_order = db.get_expired_orders(expires_at + 1).await.unwrap();
        assert_eq!(id, db_order[0]);
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
    async fn get_existing_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::ZERO;
        let expires_at = 100;
        let lock_expires_at = 50;

        db.add_order(id, expires_at, lock_expires_at).await.unwrap();

        let result = db.get_order(id).await.unwrap();
        assert!(result.is_some());
        let (fetched_expires_at, fetched_lock_expires_at) = result.unwrap();
        assert_eq!(fetched_expires_at, expires_at);
        assert_eq!(fetched_lock_expires_at, lock_expires_at);
    }

    #[sqlx::test]
    async fn query_nonexistent_order(pool: SqlitePool) {
        let db: DbObj = Arc::new(SqliteDb::from(pool).await.unwrap());
        let id = U256::from(999);

        let result = db.get_order(id).await.unwrap();
        assert!(result.is_none());

        db.remove_order(id).await.unwrap();

        db.remove_order(id).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());
    }
}
