use std::io::Read;

use alloy::primitives::Address;
use sqlx::postgres::{PgPool, PgPoolOptions};
use thiserror::Error;

/// Order DB Errors
#[derive(Error, Debug)]
pub enum OrderDbErr {
    #[error("Missing env var {0}")]
    MissingEnv(&'static str),

    #[error("Invalid DB_POOL_SIZE")]
    InvalidPoolSize(#[from] std::num::ParseIntError),

    #[error("Migrations failed")]
    MigrateErr(#[from] sqlx::migrate::MigrateError),

    #[error("sqlx error")]
    SqlErr(#[from] sqlx::Error),
}

struct OrderDb {
    pool: PgPool,
}

impl OrderDb {
    pub async fn from_env() -> Result<Self, OrderDbErr> {
        let conn_url =
            std::env::var("DB_CONN_URL").map_err(|_| OrderDbErr::MissingEnv("DB_CONN_URL"))?;
        let pool_size: u32 = std::env::var("DB_POOL_SIZE")
            .map_err(|_| OrderDbErr::MissingEnv("DB_POOL_SIZE"))?
            .parse()?;

        let pool = PgPoolOptions::new().max_connections(pool_size).connect(&conn_url).await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self { pool })
    }

    /// Check and optionally add a active broker
    ///
    /// If the broker address is currently active return true, if not
    /// add the broker to the active set
    pub async fn active_broker(&self, addr: Address) -> Result<bool, OrderDbErr> {
        let active_broker: Option<i64> =
            sqlx::query_scalar("SELECT COUNT(*) FROM brokers WHERE addr = $1")
                .bind(addr.as_slice())
                .fetch_optional(&self.pool)
                .await?;

        if active_broker.is_none() {
            sqlx::query("INSERT INTO brokers (addr) VALUES ($1)")
                .bind(addr.as_slice())
                .execute(&self.pool)
                .await?;
        }

        Ok(active_broker.is_some())
    }

    pub async fn deactivate_broker(&self, addr: Address) -> Result<(), OrderDbErr> {
        sqlx::query("DELETE FROM brokers WHERE addr = $1")
            .bind(addr.as_slice())
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
