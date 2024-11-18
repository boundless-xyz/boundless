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

    #[error("Migrations failed")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("sqlx error")]
    SqlErr(#[from] sqlx::Error),

    #[error("No rows effected when expected: {0}")]
    NoRows(&'static str),

    #[error("Json serialization error")]
    JsonErr(#[from] serde_json::Error),
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct DbOrder {
    pub id: i64,
    #[sqlx(rename = "order_data", json)]
    pub order: Order,
}

pub struct OrderDb {
    pool: PgPool,
}

const ORDER_CHANNEL: &str = "new_orders";

pub type OrderStream = Pin<Box<dyn Stream<Item = Result<DbOrder, OrderDbErr>> + Send>>;

impl OrderDb {
    pub async fn from_pool(pool: PgPool) -> Result<Self, OrderDbErr> {
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

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

    /// Check and optionally add a active broker
    ///
    /// If the broker address is currently active return true, if not
    /// add the broker to the active set
    pub async fn active_broker(&self, addr: Address) -> Result<bool, OrderDbErr> {
        let broker_counts_res: Option<i64> =
            sqlx::query_scalar("SELECT COUNT(*) FROM brokers WHERE addr = $1")
                .bind(addr.as_slice())
                .fetch_optional(&self.pool)
                .await?;

        let Some(broker_count) = broker_counts_res else {
            return Err(OrderDbErr::NoRows("active_broker count"));
        };
        if broker_count == 0 {
            let res = sqlx::query("INSERT INTO brokers (addr) VALUES ($1)")
                .bind(addr.as_slice())
                .execute(&self.pool)
                .await?;

            if res.rows_affected() != 1 {
                return Err(OrderDbErr::NoRows("broker address"));
            }

            Ok(false)
        } else {
            Ok(true)
        }
    }

    /// Deactivate a broker
    ///
    /// If the broker currently exists it will deactivate it. It should not fail if
    /// the broker address does not exist.
    pub async fn deactivate_broker(&self, addr: Address) -> Result<(), OrderDbErr> {
        sqlx::query("DELETE FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .execute(&self.pool)
            .await?;
        Ok(())
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
    /// Lists all orders the the database with a size bound and offset. The offset can be
    /// equal to the DB ID since they are sequential for listing all new orders after a specific ID
    pub async fn list_orders(&self, size: i64, offset: i64) -> Result<Vec<DbOrder>, OrderDbErr> {
        let rows: Vec<DbOrder> =
            sqlx::query_as("SELECT * FROM orders ORDER BY id LIMIT $1 OFFSET $2")
                .bind(size)
                .bind(offset)
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
}

#[cfg(test)]
mod tests {
    use alloy::{
        primitives::{
            aliases::{U192, U96},
            B256,
        },
        signers::local::LocalSigner,
    };
    use boundless_market::contracts::{
        Input, InputType, Offer, Predicate, PredicateType, ProvingRequest, Requirements,
    };
    use futures_util::StreamExt;
    use std::sync::Arc;
    use tokio::task::JoinHandle;

    use super::*;

    fn create_order() -> Order {
        let signer = LocalSigner::random();
        let req = ProvingRequest {
            id: U192::from(0),
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
                minPrice: U96::from(0),
                maxPrice: U96::from(1),
                biddingStart: 0,
                timeout: 1000,
                rampUpPeriod: 1,
                lockinStake: U96::from(0),
            },
        };
        let signature = req.sign_request(&signer, Address::ZERO, 31337).unwrap();

        Order::new(req, signature)
    }

    #[sqlx::test]
    async fn active_broker(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();

        let addr = Address::ZERO;
        assert!(!db.active_broker(addr).await.unwrap());
        assert!(db.active_broker(addr).await.unwrap());
    }

    #[sqlx::test]
    async fn deactivate_broker(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let addr = Address::ZERO;

        assert!(!db.active_broker(addr).await.unwrap());
        db.deactivate_broker(addr).await.unwrap();
        assert!(!db.active_broker(addr).await.unwrap());
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

        let orders = db.list_orders(1, 0).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
    }

    #[sqlx::test]
    async fn list_orders_page_forward(pool: PgPool) {
        let db = OrderDb::from_pool(pool).await.unwrap();
        let order = create_order();
        let _order_id = db.add_order(order.clone()).await.unwrap();
        let order_id = db.add_order(order.clone()).await.unwrap();

        let orders = db.list_orders(1, 1).await.unwrap();
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].id, order_id);
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
