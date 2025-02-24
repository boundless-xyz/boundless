// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::Address;
use async_stream::stream;
use boundless_market::order_stream_client::Order;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use sqlx::{
    postgres::{PgListener, PgPool, PgPoolOptions},
    types::chrono::{DateTime, Utc},
};
use std::pin::Pin;
use thiserror::Error as ThisError;

/// Order DB Errors
#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum OrderDbErr {
    #[error("Missing env var {0}")]
    MissingEnv(&'static str),

    #[error("Invalid DB_POOL_SIZE")]
    InvalidPoolSize(#[from] std::num::ParseIntError),

    #[error("Address not found: {0}")]
    AddrNotFound(Address),

    #[error("Migrations failed")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("sqlx error")]
    SqlErr(#[from] sqlx::Error),

    #[error("No rows effected when expected: {0}")]
    NoRows(&'static str),

    #[error("Json serialization error")]
    JsonErr(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, sqlx::FromRow, Debug)]
pub struct DbOrder {
    pub id: i64,
    #[sqlx(rename = "order_data", json)]
    pub order: Order,
    pub created_at: Option<DateTime<Utc>>,
}

pub struct OrderDb {
    pool: PgPool,
}

const ORDER_CHANNEL: &str = "new_orders";

pub type OrderStream = Pin<Box<dyn Stream<Item = Result<DbOrder, OrderDbErr>> + Send>>;

impl OrderDb {
    /// Constructs a [OrderDb] from an existing [PgPool]
    ///
    /// This method applies database migrations
    pub async fn from_pool(pool: PgPool) -> Result<Self, OrderDbErr> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Construct a new [OrderDb] from environment variables
    ///
    /// Reads the following env vars:
    /// * `DATABASE_URL` - postgresql connection string
    /// * `DB_POOL_SIZE` - size of postgresql connection pool for this process
    ///
    /// This method applies database migrations
    pub async fn from_env() -> Result<Self, OrderDbErr> {
        let conn_url =
            std::env::var("DATABASE_URL").map_err(|_| OrderDbErr::MissingEnv("DATABASE_URL"))?;
        let pool_size: u32 = std::env::var("DB_POOL_SIZE")
            .inspect_err(|_| tracing::warn!("No DB_POOL_SIZE set, defaulting to 5"))
            .unwrap_or("5".into())
            .parse()?;

        let pool = PgPoolOptions::new().max_connections(pool_size).connect(&conn_url).await?;

        Self::from_pool(pool).await
    }

    fn create_nonce() -> String {
        let rand_bytes: [u8; 16] = rand::random();
        hex::encode(rand_bytes.as_slice())
    }

    /// Add a new broker to the database
    ///
    /// Returning its new nonce (hex encoded)
    pub async fn add_broker(&self, addr: Address) -> Result<String, OrderDbErr> {
        let nonce = Self::create_nonce();
        let res = sqlx::query("INSERT INTO brokers (addr, nonce) VALUES ($1, $2)")
            .bind(addr.as_slice())
            .bind(&nonce)
            .execute(&self.pool)
            .await?;

        if res.rows_affected() != 1 {
            return Err(OrderDbErr::NoRows("broker address"));
        }

        Ok(nonce)
    }

    /// Mark the broker as updated by setting the update_at time
    ///
    /// Useful for any heartbeats or tracking liveness
    pub async fn broker_update(&self, addr: Address) -> Result<(), OrderDbErr> {
        let res = sqlx::query("UPDATE brokers SET updated_at = NOW() WHERE addr = $1")
            .bind(addr.as_slice())
            .execute(&self.pool)
            .await?;
        if res.rows_affected() == 0 {
            return Err(OrderDbErr::NoRows("disconnect broker"));
        }

        Ok(())
    }

    /// Fetches the current broker nonce
    ///
    /// Fetches a brokers nonce (hex encoded), returning a error if the broker is not found
    pub async fn get_nonce(&self, addr: Address) -> Result<String, OrderDbErr> {
        let nonce: Option<String> = sqlx::query_scalar("SELECT nonce FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .fetch_optional(&self.pool)
            .await?;

        let Some(nonce) = nonce else {
            return Err(OrderDbErr::AddrNotFound(addr));
        };

        Ok(nonce)
    }

    /// Updates the broker nonce
    ///
    /// Returning the updated nonce value, nonce hex encoded
    pub async fn set_nonce(&self, addr: Address) -> Result<String, OrderDbErr> {
        let nonce = Self::create_nonce();
        let res = sqlx::query("UPDATE brokers SET nonce = $1 WHERE addr = $2")
            .bind(&nonce)
            .bind(addr.as_slice())
            .execute(&self.pool)
            .await?;

        if res.rows_affected() == 0 {
            return Err(OrderDbErr::NoRows("Updating nonce failed to apply"));
        }

        Ok(nonce)
    }

    /// Add order to DB and notify listeners
    ///
    /// Adds a new order to the database, returning its db identifier, additionally notifies
    /// all listeners of the new order.
    pub async fn add_order(&self, order: Order) -> Result<i64, OrderDbErr> {
        let mut txn = self.pool.begin().await?;
        let row_res: Option<(i64, DateTime<Utc>)> = sqlx::query_as(
            "INSERT INTO orders (order_data, created_at) VALUES ($1, NOW()) RETURNING id, created_at",
        )
        .bind(sqlx::types::Json(order.clone()))
        .fetch_optional(&mut *txn)
        .await?;

        let Some(row) = row_res else {
            return Err(OrderDbErr::NoRows("new order"));
        };

        let id = row.0;
        let created_at = row.1;

        sqlx::query("SELECT pg_notify($1, $2::text)")
            .bind(ORDER_CHANNEL)
            .bind(sqlx::types::Json(DbOrder { id, created_at: Some(created_at), order }))
            .execute(&mut *txn)
            .await?;

        txn.commit().await?;

        Ok(id)
    }

    /// Deletes a order from the database
    #[cfg(test)]
    pub async fn delete_order(&self, id: i64) -> Result<(), OrderDbErr> {
        if sqlx::query("DELETE FROM orders WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await?
            .rows_affected()
            != 1
        {
            Err(OrderDbErr::NoRows("delete order"))
        } else {
            Ok(())
        }
    }

    /// List orders with pagination
    ///
    /// Lists all orders the the database with a size bound and start id. The index_id will be
    /// equal to the DB ID since they are sequential for listing all new orders after a specific ID
    pub async fn list_orders(&self, index_id: i64, size: i64) -> Result<Vec<DbOrder>, OrderDbErr> {
        let rows: Vec<DbOrder> = sqlx::query_as("SELECT * FROM orders WHERE id >= $1 LIMIT $2")
            .bind(index_id)
            .bind(size)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows)
    }

    /// Returns a stream of new orders from the DB
    ///
    /// listens to the new orders and emits them as a async Stream
    pub async fn order_stream(&self) -> Result<OrderStream, OrderDbErr> {
        let mut listener = PgListener::connect_with(&self.pool).await.unwrap();
        listener.listen(ORDER_CHANNEL).await?;

        Ok(Box::pin(stream! {
            while let Some(elm) = listener.try_recv().await? {
                let order: DbOrder = serde_json::from_str(elm.payload())?;
                yield Ok(order);
            }
        }))
    }

    /// Simple health check to test postgesql connectivity
    pub async fn health_check(&self) -> Result<(), OrderDbErr> {
        sqlx::query("SELECT COUNT(*) FROM orders LIMIT 1").execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use alloy::{
        primitives::{B256, U256},
        signers::local::LocalSigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProofRequest, Requirements,
    };
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::task::JoinHandle;

    use super::*;

    async fn create_order() -> Order {
        let signer = LocalSigner::random();
        let req = ProofRequest {
            id: U256::ZERO,
            requirements: Requirements {
                imageId: B256::ZERO,
                predicate: Predicate {
                    predicateType: PredicateType::PrefixMatch,
                    data: Default::default(),
                },
            },
            imageUrl: "test".to_string(),
            input: Input { inputType: InputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockStake: U256::from(0),
                lockTimeout: 1000,
            },
        };
        let signature = req.sign_request(&signer, Address::ZERO, 31337).await.unwrap();

        Order::new(req, signature)
    }

    #[sqlx::test]
    async fn add_broker(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();

        let addr = Address::ZERO;
        db.add_broker(addr).await.unwrap();

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test]
    async fn get_nonce(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;

        db.add_broker(addr).await.unwrap();

        let nonce = db.get_nonce(addr).await.unwrap();
        let db_nonce: String = sqlx::query_scalar("SELECT nonce FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();

        assert_eq!(nonce, db_nonce);
    }

    #[sqlx::test]
    #[should_panic(expected = "AddrNotFound(0x0000000000000000000000000000000000000000)")]
    async fn missing_nonce(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;
        let _nonce = db.get_nonce(addr).await.unwrap();
    }

    #[sqlx::test]
    async fn add_order(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order().await;
        let order_id = db.add_order(order).await.unwrap();
        assert_eq!(order_id, 1);
    }

    #[sqlx::test]
    async fn del_order(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order().await;
        let order_id = db.add_order(order).await.unwrap();
        db.delete_order(order_id).await.unwrap();
    }

    #[sqlx::test]
    async fn list_orders_simple(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order().await;
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(1, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_orders_page_forward(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order().await;
        let _order_id = db.add_order(order.clone()).await.unwrap();
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(2, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_after_del(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order().await;
        let order_id_1 = db.add_order(order.clone()).await.unwrap();
        let order_id_2 = db.add_order(order.clone()).await.unwrap();

        db.delete_order(order_id_1).await.unwrap();
        let orders = db.list_orders(order_id_2, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id_2);
    }

    #[sqlx::test]
    async fn order_stream(pool: PgPool) {
        let db = Arc::new(OrderDb::from_pool(pool).await.unwrap());

        let db_copy = db.clone();
        // Channel to signal stream is ready
        let (tx, rx) = tokio::sync::oneshot::channel();
        let task: JoinHandle<Result<DbOrder, OrderDbErr>> = tokio::spawn(async move {
            let mut new_orders = db_copy.order_stream().await.unwrap();
            tx.send(()).unwrap(); // Signal stream is ready
            let order = new_orders.next().await.unwrap().unwrap();
            Ok(order)
        });

        rx.await.unwrap(); // Wait for stream setup

        let order = create_order().await;
        let order_id = db.add_order(order).await.unwrap();
        let db_order = task.await.unwrap().unwrap();
        assert_eq!(db_order.id, order_id);
    }

    #[sqlx::test]
    async fn broker_update(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;

        db.add_broker(addr).await.unwrap();
        db.broker_update(addr).await.unwrap();

        let db_nonce: Option<sqlx::types::chrono::DateTime<sqlx::types::chrono::Utc>> =
            sqlx::query_scalar("SELECT updated_at FROM brokers WHERE addr = $1")
                .bind(addr.as_slice())
                .fetch_optional(&pool)
                .await
                .unwrap();

        assert!(db_nonce.is_some());
    }
}
