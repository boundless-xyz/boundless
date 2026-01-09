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

use alloy::primitives::U256;
use async_trait::async_trait;
use sqlx::{
    postgres::{PgPool, PgPoolOptions},
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

pub struct PgDb {
    pool: PgPool,
}

impl PgDb {
    /// Constructs a [PgDb] from an existing [PgPool]
    ///
    /// This method applies database migrations
    pub async fn from_pool(pool: PgPool) -> Result<Self, DbError> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Construct a new [PgDb] from a connection string
    pub async fn new(conn_str: &str) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new().max_connections(5).connect(conn_str).await?;

        Self::from_pool(pool).await
    }
}

#[derive(sqlx::FromRow)]
struct DbOrder {
    id: String,
}

#[async_trait]
impl SlasherDb for PgDb {
    async fn add_order(
        &self,
        id: U256,
        expires_at: u64,
        lock_expires_at: u64,
    ) -> Result<(), DbError> {
        tracing::trace!("Adding order: 0x{:x}", id);
        // Only store the order if it has a valid expiration time.
        // If the expires_at is 0, the request is already slashed or fulfilled (or even not locked).
        // Use ON CONFLICT DO NOTHING to atomically handle duplicates (prevents race conditions)
        if expires_at > 0 && lock_expires_at > 0 {
            sqlx::query("INSERT INTO orders (id, expires_at, lock_expires_at) VALUES ($1, $2, $3) ON CONFLICT (id) DO NOTHING")
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
        let res = sqlx::query(
            "INSERT INTO last_block (id, block) VALUES ($1, $2) ON CONFLICT (id) DO UPDATE SET block = $2"
        )
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
    use sqlx::postgres::PgPool;

    async fn setup_test_db(pool: PgPool) -> DbObj {
        // sqlx::test provides a PgPool connected to a temporary test database
        Arc::new(PgDb::from_pool(pool).await.unwrap())
    }

    #[sqlx::test]
    async fn add_order(pool: PgPool) {
        let db = setup_test_db(pool).await;
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
    async fn drop_order(pool: PgPool) {
        let db = setup_test_db(pool).await;
        let id = U256::ZERO;
        db.add_order(id, 10, 5).await.unwrap();
        db.remove_order(id).await.unwrap();
        // Removing the same order should not fail
        db.remove_order(id).await.unwrap();
    }

    #[sqlx::test]
    async fn order_not_exists(pool: PgPool) {
        let db = setup_test_db(pool).await;
        assert!(!db.order_exists(U256::ZERO).await.unwrap());
    }

    #[sqlx::test]
    async fn order_exists(pool: PgPool) {
        let db = setup_test_db(pool).await;
        let id = U256::ZERO;
        db.add_order(id, 10, 5).await.unwrap();

        assert!(db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn get_expired_orders(pool: PgPool) {
        let db = setup_test_db(pool).await;
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
    async fn set_get_block(pool: PgPool) {
        let db = setup_test_db(pool).await;

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
    async fn get_existing_order(pool: PgPool) {
        let db = setup_test_db(pool).await;
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
    async fn query_nonexistent_order(pool: PgPool) {
        let db = setup_test_db(pool).await;
        let id = U256::from(999);

        let result = db.get_order(id).await.unwrap();
        assert!(result.is_none());

        db.remove_order(id).await.unwrap();

        db.remove_order(id).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn get_last_block_none(pool: PgPool) {
        let db = setup_test_db(pool).await;

        // Initially no block should be set
        let result = db.get_last_block().await.unwrap();
        assert!(result.is_none());
    }

    #[sqlx::test]
    async fn set_last_block_conflict_update(pool: PgPool) {
        let db = setup_test_db(pool).await;

        // Set initial block
        db.set_last_block(100).await.unwrap();
        assert_eq!(db.get_last_block().await.unwrap().unwrap(), 100);

        // Update with ON CONFLICT - should update existing row
        db.set_last_block(200).await.unwrap();
        assert_eq!(db.get_last_block().await.unwrap().unwrap(), 200);

        // Update again
        db.set_last_block(300).await.unwrap();
        assert_eq!(db.get_last_block().await.unwrap().unwrap(), 300);
    }

    #[sqlx::test]
    async fn get_expired_orders_multiple(pool: PgPool) {
        let db = setup_test_db(pool).await;

        let id1 = U256::from(1);
        let id2 = U256::from(2);
        let id3 = U256::from(3);

        // Add orders with different expiration times
        db.add_order(id1, 10, 5).await.unwrap(); // expires at 10
        db.add_order(id2, 20, 15).await.unwrap(); // expires at 20
        db.add_order(id3, 30, 25).await.unwrap(); // expires at 30

        // No expired orders at timestamp 5
        let expired = db.get_expired_orders(5).await.unwrap();
        assert!(expired.is_empty());

        // Only id1 expired at timestamp 15
        let expired = db.get_expired_orders(15).await.unwrap();
        assert_eq!(expired.len(), 1);
        assert!(expired.contains(&id1));

        // id1 and id2 expired at timestamp 25
        let expired = db.get_expired_orders(25).await.unwrap();
        assert_eq!(expired.len(), 2);
        assert!(expired.contains(&id1));
        assert!(expired.contains(&id2));

        // All expired at timestamp 35
        let expired = db.get_expired_orders(35).await.unwrap();
        assert_eq!(expired.len(), 3);
        assert!(expired.contains(&id1));
        assert!(expired.contains(&id2));
        assert!(expired.contains(&id3));
    }

    #[sqlx::test]
    async fn add_order_duplicate_prevention(pool: PgPool) {
        let db = setup_test_db(pool).await;
        let id = U256::from(42);

        // Add order first time
        db.add_order(id, 100, 50).await.unwrap();
        assert!(db.order_exists(id).await.unwrap());

        // Try to add same order again - should not create duplicate
        db.add_order(id, 100, 50).await.unwrap();

        // Verify only one order exists
        let result = db.get_order(id).await.unwrap();
        assert!(result.is_some());
        let (expires_at, lock_expires_at) = result.unwrap();
        assert_eq!(expires_at, 100);
        assert_eq!(lock_expires_at, 50);

        // Verify order_exists still returns true (not duplicated)
        assert!(db.order_exists(id).await.unwrap());
    }

    #[sqlx::test]
    async fn add_order_with_zero_expires(pool: PgPool) {
        let db = setup_test_db(pool).await;
        let id = U256::from(99);

        // Try to add order with expires_at = 0 - should not be stored
        db.add_order(id, 0, 50).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());

        // Try to add order with lock_expires_at = 0 - should not be stored
        db.add_order(id, 100, 0).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());

        // Try to add order with both = 0 - should not be stored
        db.add_order(id, 0, 0).await.unwrap();
        assert!(!db.order_exists(id).await.unwrap());
    }
}
