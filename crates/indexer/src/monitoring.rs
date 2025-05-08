// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::str::FromStr;

use crate::db::AnyDb;
use alloy::primitives::Address;
use anyhow::Result;
use sqlx::{
    any::{install_default_drivers, AnyConnectOptions, AnyPoolOptions},
    Row,
};

/// The `monitoring` struct provides functionality to monitor and query the indexer database.
pub struct Monitor {
    /// The database connection pool.
    pub db: AnyDb,
}

impl Monitor {
    /// Creates a new instance of the Monitor.
    pub async fn new(conn: &str) -> Result<Self> {
        install_default_drivers();
        let opts = AnyConnectOptions::from_str(conn)?;
        let pool = AnyPoolOptions::new().max_connections(5).connect_with(opts).await?;

        let db = AnyDb { pool };
        Ok(Self { db })
    }

    /// Fetches requests that expired within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_requests_expired(&self, from: i64, to: i64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT pr.request_id
            FROM proof_requests pr
            LEFT JOIN request_fulfilled_events rfe
              ON pr.request_digest = rfe.request_digest
            WHERE
              rfe.request_digest IS NULL
              AND pr.expires_at >= $1
              AND pr.expires_at < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_all(self.db.pool())
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Fetches requests that expired within the given range from a specific client address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_requests_expired_from(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT pr.request_id
            FROM proof_requests pr
            LEFT JOIN request_fulfilled_events rfe
              ON pr.request_digest = rfe.request_digest
            WHERE
              rfe.request_digest IS NULL
              AND pr.expires_at >= $1
              AND pr.expires_at < $2
              AND pr.client_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", address))
        .fetch_all(self.db.pool())
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Fetch the number of requests that have been submitted within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_requests_number(&self, from: i64, to: i64) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM proof_requests
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of submitted requests.
    pub async fn total_requests(&self) -> Result<i64> {
        let row =
            sqlx::query("SELECT COUNT(*) FROM proof_requests").fetch_one(self.db.pool()).await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of requests that have been submitted within the given range from a specific client address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_requests_number_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM proof_requests
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            AND client_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", address))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of submitted requests from a specific client address.
    pub async fn total_requests_from_client(&self, address: Address) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM proof_requests WHERE client_address = $1")
            .bind(format!("{:x}", address))
            .fetch_one(self.db.pool())
            .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of fulfilled requests within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_fulfillments_number(&self, from: i64, to: i64) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM fulfillments
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of fulfilled requests.
    pub async fn total_fulfillments(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM request_fulfilled_events")
            .fetch_one(self.db.pool())
            .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of fulfilled requests within the given range from a specific client address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_fulfillments_number_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_fulfilled_events rfe
            JOIN proof_requests pr
              ON rfe.request_digest = pr.request_digest
            WHERE pr.block_timestamp >= $1
            AND pr.block_timestamp < $2
            AND pr.client_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", address))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of fulfilled requests from a specific client address.
    pub async fn total_fulfillments_from_client(&self, address: Address) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_fulfilled_events rfe
            JOIN proof_requests pr
              ON rfe.request_digest = pr.request_digest
            WHERE pr.client_address = $1
            "#,
        )
        .bind(format!("{:x}", address))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of fulfilled requests within the given range by a specific prover address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_fulfillments_number_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_fulfilled_events rfe
            JOIN fulfillments f
              ON rfe.request_digest = f.request_digest
            WHERE f.block_timestamp >= $1
            AND f.block_timestamp < $2
            AND f.prover_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of fulfilled requests by a specific prover address.
    ///
    /// prover: The prover address to filter requests by.
    pub async fn total_fulfillments_by_prover(&self, prover: Address) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_fulfilled_events rfe
            JOIN fulfillments f
              ON rfe.request_digest = f.request_digest
            WHERE f.prover_address = $1
            "#,
        )
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of locked requests by a prover within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_locked_number_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_locked_events
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            AND prover_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of locked requests by a specific prover address.
    ///
    /// prover: The prover address to filter requests by.
    pub async fn total_locked_by_prover(&self, prover: Address) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM request_locked_events
            WHERE prover_address = $1
            "#,
        )
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of slashed requests within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_slashed_number(&self, from: i64, to: i64) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM prover_slashed_events
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of slashed requests.
    pub async fn total_slashed(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM prover_slashed_events")
            .fetch_one(self.db.pool())
            .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the number of slashed requests within the given range by a specific prover address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_slashed_number_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM prover_slashed_events
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            AND prover_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Total number of slashed requests by a specific prover address.
    ///
    /// prover: The prover address to filter requests by.
    pub async fn total_slashed_by_prover(&self, prover: Address) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM prover_slashed_events
            WHERE prover_address = $1
            "#,
        )
        .bind(format!("{:x}", prover))
        .fetch_one(self.db.pool())
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the success rate of fulfilled requests by a prover within the given range.
    ///
    /// The success rate is calculated as the number of fulfilled requests divided by the number of locked requests.
    /// If the number of locked requests is zero, the success rate returned is `None`.
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_success_rate_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<Option<f64>> {
        let fulfilled = self.fetch_fulfillments_number_by_prover(from, to, prover).await?;
        let locked: i64 = self.fetch_locked_number_by_prover(from, to, prover).await?;

        if locked == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled as f64 / locked as f64))
    }

    /// Total success rate of fulfilled requests by a specific prover address.
    ///
    /// The success rate is calculated as the number of fulfilled requests divided by the number of locked requests.
    /// If the number of locked requests is zero, the success rate returned is `None`.
    /// prover: The prover address to filter requests by.
    pub async fn total_success_rate_by_prover(&self, prover: Address) -> Result<Option<f64>> {
        let fulfilled = self.total_fulfillments_by_prover(prover).await?;
        let locked: i64 = self.total_locked_by_prover(prover).await?;

        if locked == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled as f64 / locked as f64))
    }

    /// Fetch the success rate of fulfilled requests from a specific client address within the given range.
    ///
    /// The success rate is calculated as the number of fulfilled requests divided by the number of submitted requests.
    /// If the number of submitted requests is zero, the success rate returned is `None`.
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_success_rate_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<Option<f64>> {
        let fulfilled = self.fetch_fulfillments_number_from_client(from, to, address).await?;
        let submitted: i64 = self.fetch_requests_number_from_client(from, to, address).await?;

        if submitted == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled as f64 / submitted as f64))
    }

    /// Total success rate of fulfilled requests from a specific client address.
    ///
    /// The success rate is calculated as the number of fulfilled requests divided by the number of submitted requests.
    /// If the number of submitted requests is zero, the success rate returned is `None`.
    /// address: The client address to filter requests by.
    pub async fn total_success_rate_from_client(&self, address: Address) -> Result<Option<f64>> {
        let fulfilled = self.total_fulfillments_from_client(address).await?;
        let submitted: i64 = self.total_requests_from_client(address).await?;

        if submitted == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled as f64 / submitted as f64))
    }
}
