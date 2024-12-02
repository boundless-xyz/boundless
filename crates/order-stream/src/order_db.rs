// Copyright 2024 RISC Zero, Inc.
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

use alloy::primitives::Address;
use async_stream::stream;
use boundless_market::order_stream_client::Order;
use futures_util::Stream;
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgListener, PgPool, PgPoolOptions};
use std::pin::Pin;
use thiserror::Error as ThisError;

/// Order DB Errors
#[derive(ThisError, Debug)]
pub enum OrderDbErr {
    #[error("Missing env var {0}")]
    MissingEnv(&'static str),

    #[error("Invalid DB_POOL_SIZE")]
    InvalidPoolSize(#[from] std::num::ParseIntError),

    #[error("Max concurrent connections")]
    MaxConnections,

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
}

pub struct OrderDb {
    pool: PgPool,
}

const ORDER_CHANNEL: &str = "new_orders";
const MAX_BROKER_CONNECTIONS: i32 = 1;

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
        hex::encode(rand_bytes.to_vec())
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

    /// Connects a broker
    ///
    /// Increments a brokers connection count, and faults if over connection MAX
    pub async fn connect_broker(&self, addr: Address) -> Result<(), OrderDbErr> {
        let mut txn = self.pool.begin().await?;

        let connections: i32 =
            sqlx::query_scalar("SELECT connections FROM brokers WHERE addr = $1")
                .bind(addr.as_slice())
                .fetch_one(&mut *txn)
                .await?;

        if connections >= MAX_BROKER_CONNECTIONS {
            return Err(OrderDbErr::MaxConnections);
        }

        let res = sqlx::query(
            "UPDATE brokers SET connections = connections + 1 WHERE addr = $1 AND connections < $2",
        )
        .bind(addr.as_slice())
        .bind(MAX_BROKER_CONNECTIONS)
        .execute(&mut *txn)
        .await?;

        if res.rows_affected() == 0 {
            return Err(OrderDbErr::NoRows("connect broker"));
        }

        txn.commit().await?;

        Ok(())
    }

    /// Disconnects a broker, decreasing connection count
    pub async fn disconnect_broker(&self, addr: Address) -> Result<(), OrderDbErr> {
        let res = sqlx::query("UPDATE brokers SET connections = connections - 1 WHERE addr = $1")
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
        let id_res: Option<i64> =
            sqlx::query_scalar("INSERT INTO orders (order_data) VALUES ($1) RETURNING id")
                .bind(sqlx::types::Json(order.clone()))
                .fetch_optional(&mut *txn)
                .await?;

        let Some(id) = id_res else {
            return Err(OrderDbErr::NoRows("new order"));
        };

        sqlx::query("SELECT pg_notify($1, $2::text)")
            .bind(ORDER_CHANNEL)
            .bind(sqlx::types::Json(DbOrder { id, order }))
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
                println!("{}", elm.payload());
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

    fn create_order() -> Order {
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
                lockinStake: U256::from(0),
            },
        };
        let signature = req.sign_request(&signer, Address::ZERO, 31337).unwrap();

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
    async fn connect_broker(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;

        db.add_broker(addr).await.unwrap();
        db.connect_broker(addr).await.unwrap();
        let conns: i32 = sqlx::query_scalar("SELECT connections FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(conns, 1);
    }

    #[sqlx::test]
    #[should_panic(expected = "MaxConnections")]
    async fn connect_max(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;

        db.add_broker(addr).await.unwrap();
        db.connect_broker(addr).await.unwrap();
        db.connect_broker(addr).await.unwrap();
    }

    #[sqlx::test]
    async fn disconnect_broker(pool: PgPool) {
        let db = OrderDb::from_pool(pool.clone()).await.unwrap();
        let addr = Address::ZERO;

        db.add_broker(addr).await.unwrap();
        db.connect_broker(addr).await.unwrap();
        db.disconnect_broker(addr).await.unwrap();
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

        let order = create_order();
        let order_id = db.add_order(order).await.unwrap();
        assert_eq!(order_id, 1);
    }

    #[sqlx::test]
    async fn del_order(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order();
        let order_id = db.add_order(order).await.unwrap();
        db.delete_order(order_id).await.unwrap();
    }

    #[sqlx::test]
    async fn list_orders_simple(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order();
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(1, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_orders_page_forward(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order();
        let _order_id = db.add_order(order.clone()).await.unwrap();
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(2, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_after_del(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order();
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
        let task: JoinHandle<Result<DbOrder, OrderDbErr>> = tokio::spawn(async move {
            let mut new_orders = db_copy.order_stream().await.unwrap();
            let order = new_orders.next().await.unwrap().unwrap();
            Ok(order)
        });

        let order = create_order();
        let order_id = db.add_order(order).await.unwrap();
        let db_order = task.await.unwrap().unwrap();
        assert_eq!(db_order.id, order_id);
    }
}
