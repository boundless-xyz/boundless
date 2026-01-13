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
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Order DB Errors
#[derive(ThisError, Debug)]
#[non_exhaustive]
pub enum OrderDbErr {
    #[error("Missing env var {0}")]
    MissingEnv(&'static str),

    #[error("Invalid DB_POOL_SIZE {0}")]
    InvalidPoolSize(#[from] std::num::ParseIntError),

    #[error("Address not found: {0}")]
    AddrNotFound(Address),

    #[error("Migrations failed {0}")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("sqlx error {0}")]
    SqlErr(#[from] sqlx::Error),

    #[error("No rows effected when expected: {0}")]
    NoRows(&'static str),

    #[error("Json serialization error {0}")]
    JsonErr(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, sqlx::FromRow, Debug, ToSchema)]
pub struct DbOrder {
    pub id: i64,
    #[sqlx(rename = "order_data", json)]
    pub order: Order,
    #[schema(value_type = Option<String>)]
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
            "INSERT INTO orders (request_id, request_digest, order_data, created_at) VALUES ($1, $2, $3, NOW()) RETURNING id, created_at",
        )
        .bind(order.request.id.to_string())
        .bind(order.request_digest.to_string())
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

    /// Find orders by request ID
    ///
    /// Returns a list of orders that match the request ID
    pub async fn find_orders_by_request_id(
        &self,
        request_id: String,
    ) -> Result<Vec<DbOrder>, OrderDbErr> {
        let rows: Vec<DbOrder> = sqlx::query_as("SELECT * FROM orders WHERE request_id = $1")
            .bind(request_id)
            .fetch_all(&self.pool)
            .await?;

        Ok(rows)
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

    /// List orders sorted by creation time descending
    ///
    /// Lists orders sorted by creation time descending (most recent first) with pagination.
    /// Returns orders created after the given timestamp, up to the specified limit.
    pub async fn list_orders_by_creation_desc(
        &self,
        after_timestamp: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<DbOrder>, OrderDbErr> {
        let rows: Vec<DbOrder> =
            match after_timestamp {
                Some(ts) => sqlx::query_as(
                    "SELECT * FROM orders WHERE created_at > $1 ORDER BY created_at DESC LIMIT $2",
                )
                .bind(ts)
                .bind(limit)
                .fetch_all(&self.pool)
                .await?,
                None => {
                    sqlx::query_as("SELECT * FROM orders ORDER BY created_at DESC LIMIT $1")
                        .bind(limit)
                        .fetch_all(&self.pool)
                        .await?
                }
            };

        Ok(rows)
    }

    /// List orders with cursor-based pagination and flexible filtering (v2)
    ///
    /// Provides cursor-based pagination using (created_at, id) tuples for stable ordering.
    /// Supports filtering by timestamp ranges (before/after) and bidirectional sorting.
    pub async fn list_orders_v2(
        &self,
        cursor: Option<(DateTime<Utc>, i64)>,
        limit: i64,
        sort: SortDirection,
        before: Option<DateTime<Utc>>,
        after: Option<DateTime<Utc>>,
    ) -> Result<Vec<DbOrder>, OrderDbErr> {
        let mut conditions = Vec::new();
        let mut bind_count = 0;

        let cursor_condition = match (cursor, sort) {
            (Some((_ts, _id)), SortDirection::Asc) => {
                bind_count += 2;
                Some(format!("(created_at, id) > (${}, ${})", bind_count - 1, bind_count))
            }
            (Some((_ts, _id)), SortDirection::Desc) => {
                bind_count += 2;
                Some(format!("(created_at, id) < (${}, ${})", bind_count - 1, bind_count))
            }
            (None, _) => None,
        };

        if let Some(cond) = cursor_condition {
            conditions.push(cond);
        }

        if after.is_some() {
            bind_count += 1;
            conditions.push(format!("created_at > ${}", bind_count));
        }

        if before.is_some() {
            bind_count += 1;
            conditions.push(format!("created_at < ${}", bind_count));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        let order_clause = match sort {
            SortDirection::Asc => "ORDER BY created_at ASC, id ASC",
            SortDirection::Desc => "ORDER BY created_at DESC, id DESC",
        };

        bind_count += 1;
        let query_str =
            format!("SELECT * FROM orders {} {} LIMIT ${}", where_clause, order_clause, bind_count);

        let mut query = sqlx::query_as::<_, DbOrder>(&query_str);

        if let Some((ts, id)) = cursor {
            query = query.bind(ts).bind(id);
        }

        if let Some(ts) = after {
            query = query.bind(ts);
        }

        if let Some(ts) = before {
            query = query.bind(ts);
        }

        query = query.bind(limit);

        let rows = query.fetch_all(&self.pool).await?;

        Ok(rows)
    }

    /// Returns a stream of new orders from the DB
    ///
    /// Listens to the new orders and emits them as an async Stream.
    /// Uses recv() which awaits notifications indefinitely until the connection
    /// is closed or an error occurs.
    pub async fn order_stream(&self) -> Result<OrderStream, OrderDbErr> {
        let mut listener = PgListener::connect_with(&self.pool).await.unwrap();
        listener.listen(ORDER_CHANNEL).await?;

        Ok(Box::pin(stream! {
            while let Ok(notification) = listener.recv().await {
                let order: DbOrder = serde_json::from_str(notification.payload())?;
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
        primitives::{Bytes, U256},
        signers::local::LocalSigner,
        sol_types::SolStruct,
    };
    use boundless_market::contracts::{
        eip712_domain, Offer, Predicate, ProofRequest, RequestInput, RequestInputType, Requirements,
    };
    use futures_util::StreamExt;
    use risc0_zkvm::sha::Digest;
    use std::sync::Arc;
    use tokio::task::JoinHandle;

    use super::*;

    async fn create_order(id: U256) -> Order {
        let signer = LocalSigner::random();
        let req = ProofRequest {
            id,
            requirements: Requirements::new(Predicate::prefix_match(
                Digest::ZERO,
                Bytes::default(),
            )),
            imageUrl: "test".to_string(),
            input: RequestInput { inputType: RequestInputType::Url, data: Default::default() },
            offer: Offer {
                minPrice: U256::from(0),
                maxPrice: U256::from(1),
                rampUpStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockCollateral: U256::from(0),
                lockTimeout: 1000,
            },
        };
        let signature = req.sign_request(&signer, Address::ZERO, 31337).await.unwrap();
        let domain = eip712_domain(Address::ZERO, 31337);
        let request_digest = req.eip712_signing_hash(&domain.alloy_struct());

        Order::new(req, request_digest, signature)
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

        let order = create_order(U256::from(1)).await;
        let order_id = db.add_order(order).await.unwrap();
        assert_eq!(order_id, 1);
    }

    #[sqlx::test]
    async fn del_order(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order(U256::from(1)).await;
        let order_id = db.add_order(order).await.unwrap();
        db.delete_order(order_id).await.unwrap();
    }

    #[sqlx::test]
    async fn list_orders_simple(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order = create_order(U256::from(1)).await;
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(1, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_orders_page_forward(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order(U256::from(1)).await;
        let order2 = create_order(U256::from(2)).await;
        let _order_id = db.add_order(order).await.unwrap();
        let order_id = db.add_order(order2).await.unwrap();

        let orders = db.list_orders(2, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_after_del(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order(U256::from(1)).await;
        let order2 = create_order(U256::from(2)).await;
        let order_id_1 = db.add_order(order).await.unwrap();
        let order_id_2 = db.add_order(order2).await.unwrap();

        db.delete_order(order_id_1).await.unwrap();
        let orders = db.list_orders(order_id_2, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id_2);
    }

    #[sqlx::test]
    async fn list_orders_by_creation_desc_test(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order1 = create_order(U256::from(1)).await;
        let id1 = db.add_order(order1).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order2 = create_order(U256::from(2)).await;
        let id2 = db.add_order(order2).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order3 = create_order(U256::from(3)).await;
        let id3 = db.add_order(order3).await.unwrap();

        let orders = db.list_orders_by_creation_desc(None, 10).await.unwrap();
        assert_eq!(orders.len(), 3);
        assert_eq!(orders[0].id, id3);
        assert_eq!(orders[1].id, id2);
        assert_eq!(orders[2].id, id1);

        let orders_limited = db.list_orders_by_creation_desc(None, 2).await.unwrap();
        assert_eq!(orders_limited.len(), 2);
        assert_eq!(orders_limited[0].id, id3);
        assert_eq!(orders_limited[1].id, id2);

        let after_timestamp = orders[1].created_at;
        let filtered = db.list_orders_by_creation_desc(after_timestamp, 10).await.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, id3);
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

        let order = create_order(U256::from(1)).await;
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

    #[sqlx::test]
    async fn list_orders_v2_basic(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order1 = create_order(U256::from(1)).await;
        let order2 = create_order(U256::from(2)).await;
        let order3 = create_order(U256::from(3)).await;

        let id1 = db.add_order(order1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let id2 = db.add_order(order2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        let id3 = db.add_order(order3).await.unwrap();

        let orders = db.list_orders_v2(None, 10, SortDirection::Desc, None, None).await.unwrap();
        assert_eq!(orders.len(), 3);
        assert_eq!(orders[0].id, id3);
        assert_eq!(orders[1].id, id2);
        assert_eq!(orders[2].id, id1);

        let orders_asc = db.list_orders_v2(None, 10, SortDirection::Asc, None, None).await.unwrap();
        assert_eq!(orders_asc.len(), 3);
        assert_eq!(orders_asc[0].id, id1);
        assert_eq!(orders_asc[1].id, id2);
        assert_eq!(orders_asc[2].id, id3);
    }

    #[sqlx::test]
    async fn list_orders_v2_cursor_pagination(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order1 = create_order(U256::from(1)).await;
        let order2 = create_order(U256::from(2)).await;
        let order3 = create_order(U256::from(3)).await;

        db.add_order(order1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db.add_order(order2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        db.add_order(order3).await.unwrap();

        let first_page = db.list_orders_v2(None, 2, SortDirection::Desc, None, None).await.unwrap();
        assert_eq!(first_page.len(), 2);

        let cursor_ts = first_page[1].created_at.unwrap();
        let cursor_id = first_page[1].id;

        let second_page = db
            .list_orders_v2(Some((cursor_ts, cursor_id)), 2, SortDirection::Desc, None, None)
            .await
            .unwrap();
        assert_eq!(second_page.len(), 1);
        assert_ne!(second_page[0].id, first_page[0].id);
        assert_ne!(second_page[0].id, first_page[1].id);
    }

    #[sqlx::test]
    async fn list_orders_v2_timestamp_filters(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order1 = create_order(U256::from(1)).await;
        db.add_order(order1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let middle_time = Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order2 = create_order(U256::from(2)).await;
        db.add_order(order2).await.unwrap();

        let after_orders = db
            .list_orders_v2(None, 10, SortDirection::Desc, None, Some(middle_time))
            .await
            .unwrap();
        assert_eq!(after_orders.len(), 1);
        assert_eq!(after_orders[0].order.request.id, U256::from(2));

        let before_orders = db
            .list_orders_v2(None, 10, SortDirection::Desc, Some(middle_time), None)
            .await
            .unwrap();
        assert_eq!(before_orders.len(), 1);
        assert_eq!(before_orders[0].order.request.id, U256::from(1));
    }

    #[sqlx::test]
    async fn list_orders_v2_range_query(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let order1 = create_order(U256::from(1)).await;
        db.add_order(order1).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let start_time = Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order2 = create_order(U256::from(2)).await;
        db.add_order(order2).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order3 = create_order(U256::from(3)).await;
        db.add_order(order3).await.unwrap();
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let end_time = Utc::now();
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let order4 = create_order(U256::from(4)).await;
        db.add_order(order4).await.unwrap();

        let range_orders = db
            .list_orders_v2(None, 10, SortDirection::Asc, Some(end_time), Some(start_time))
            .await
            .unwrap();
        assert_eq!(range_orders.len(), 2);
        assert_eq!(range_orders[0].order.request.id, U256::from(2));
        assert_eq!(range_orders[1].order.request.id, U256::from(3));
    }
}
