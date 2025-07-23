// Copyright 2025 RISC Zero, Inc.
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

use std::str::FromStr;

use alloy::primitives::Address;
use anyhow::Result;
use sqlx::{
    any::{install_default_drivers, AnyConnectOptions, AnyPoolOptions},
    AnyPool, Row,
};

/// The `monitoring` struct provides functionality to monitor and query the indexer database.
pub struct Monitor {
    /// The database connection pool.
    pub db: AnyPool,
}

impl Monitor {
    /// Creates a new instance of the Monitor.
    pub async fn new(conn: &str) -> Result<Self> {
        install_default_drivers();
        let opts = AnyConnectOptions::from_str(conn)?;
        let pool = AnyPoolOptions::new().max_connections(5).connect_with(opts).await?;

        let db = pool;
        Ok(Self { db })
    }

    /// Set the last run timestamp in the database.
    pub async fn set_last_run(&self, last_run: i64) -> Result<()> {
        let res = sqlx::query(
            "INSERT INTO metric_state (id, last_run) VALUES ($1, $2)
         ON CONFLICT (id) DO UPDATE SET last_run = EXCLUDED.last_run",
        )
        .bind(true)
        .bind(last_run)
        .execute(&self.db)
        .await?;

        if res.rows_affected() == 0 {
            anyhow::bail!("Failed to set last run");
        }

        Ok(())
    }

    /// Fetches the last run timestamp from the database.
    pub async fn get_last_run(&self) -> Result<i64> {
        let row = sqlx::query("SELECT last_run FROM metric_state WHERE id = $1")
            .bind(true)
            .fetch_one(&self.db)
            .await?;

        Ok(row.get::<i64, _>("last_run"))
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
        .fetch_all(&self.db)
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
        .bind(format!("{address:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Fetches all requests that expired from a specific client address.
    ///
    /// address: The client address to filter requests by.
    pub async fn fetch_total_requests_expired_from(&self, address: Address) -> Result<i64> {
        let row = sqlx::query(
            r#"
            SELECT COUNT(*)
            FROM proof_requests pr
            LEFT JOIN request_fulfilled_events rfe
              ON pr.request_digest = rfe.request_digest
            WHERE
              rfe.request_digest IS NULL
              AND pr.client_address = $1
            "#,
        )
        .bind(format!("{address:x}"))
        .fetch_one(&self.db)
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the requests that have been submitted within the given range.
    /// NOTE: This function queries the `proof_requests` table, not the `request_submitted_events` table,
    ///       because the `request_submitted_events` table is not updated when a request is submitted offchain.
    ///       This is therefore an approximation of when requests were submitted.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_requests(&self, from: i64, to: i64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT request_id
            FROM proof_requests
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Total number of submitted requests.
    pub async fn total_requests(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM proof_requests").fetch_one(&self.db).await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the requests that have been submitted within the given range from a specific client address.
    /// NOTE: This function queries the `proof_requests` table, not the `request_submitted_events` table,
    ///       because the `request_submitted_events` table is not updated when a request is submitted offchain.
    ///       This is therefore an approximation of when requests were submitted.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_requests_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT request_id
            FROM proof_requests
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            AND client_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{address:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Total number of submitted requests from a specific client address.
    pub async fn total_requests_from_client(&self, address: Address) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM proof_requests WHERE client_address = $1")
            .bind(format!("{address:x}"))
            .fetch_one(&self.db)
            .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the fulfilled requests within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_fulfillments(&self, from: i64, to: i64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT pr.request_id
            FROM fulfillments f
            JOIN proof_requests pr ON f.request_digest = pr.request_digest
            WHERE f.block_timestamp >= $1
            AND f.block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Total number of fulfilled requests.
    pub async fn total_fulfillments(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) FROM request_fulfilled_events")
            .fetch_one(&self.db)
            .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the fulfilled requests within the given range from a specific client address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_fulfillments_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT rfe.request_id
            FROM request_fulfilled_events rfe
            JOIN proof_requests pr
              ON rfe.request_digest = pr.request_digest
            WHERE rfe.block_timestamp >= $1
            AND rfe.block_timestamp < $2
            AND pr.client_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{address:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
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
        .bind(format!("{address:x}"))
        .fetch_one(&self.db)
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the fulfilled requests within the given range by a specific prover address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_fulfillments_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT f.request_id
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
        .bind(format!("{prover:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
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
        .bind(format!("{prover:x}"))
        .fetch_one(&self.db)
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the locked requests by a prover within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_locked_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT pr.request_id
            FROM request_locked_events rle
            JOIN proof_requests pr
              ON rle.request_digest = pr.request_digest
            WHERE rle.block_timestamp >= $1
            AND rle.block_timestamp < $2
            AND rle.prover_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{prover:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
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
        .bind(format!("{prover:x}"))
        .fetch_one(&self.db)
        .await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the slashed requests within the given range.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    pub async fn fetch_slashed(&self, from: i64, to: i64) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT request_id
            FROM prover_slashed_events
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            "#,
        )
        .bind(from)
        .bind(to)
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
    }

    /// Total number of slashed requests.
    pub async fn total_slashed(&self) -> Result<i64> {
        let row =
            sqlx::query("SELECT COUNT(*) FROM prover_slashed_events").fetch_one(&self.db).await?;

        Ok(row.get::<i64, _>(0))
    }

    /// Fetch the slashed requests within the given range by a specific prover address.
    ///
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// prover: The prover address to filter requests by.
    pub async fn fetch_slashed_by_prover(
        &self,
        from: i64,
        to: i64,
        prover: Address,
    ) -> Result<Vec<String>> {
        let rows = sqlx::query(
            r#"
            SELECT request_id
            FROM prover_slashed_events
            WHERE block_timestamp >= $1
            AND block_timestamp < $2
            AND prover_address = $3
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(format!("{prover:x}"))
        .fetch_all(&self.db)
        .await?;

        Ok(rows.into_iter().map(|row| row.get::<String, _>("request_id")).collect())
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
        .bind(format!("{prover:x}"))
        .fetch_one(&self.db)
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
        let fulfilled = self.fetch_fulfillments_by_prover(from, to, prover).await?;
        let locked = self.fetch_locked_by_prover(from, to, prover).await?;

        if locked.is_empty() {
            return Ok(None);
        }

        Ok(Some(fulfilled.len() as f64 / locked.len() as f64))
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
    /// The success rate is calculated as the number of fulfilled requests divided by the number of fulfilled + expired requests.
    /// If the number of fulfilled + expired requests is zero, the success rate returned is `None`.
    /// from: timestamp in seconds.
    /// to: timestamp in seconds.
    /// address: The client address to filter requests by.
    pub async fn fetch_success_rate_from_client(
        &self,
        from: i64,
        to: i64,
        address: Address,
    ) -> Result<Option<f64>> {
        let fulfilled = self.fetch_fulfillments_from_client(from, to, address).await?;
        let expired = self.fetch_requests_expired_from(from, to, address).await?;

        if (fulfilled.len() + expired.len()) == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled.len() as f64 / (fulfilled.len() + expired.len()) as f64))
    }

    /// Total success rate of fulfilled requests from a specific client address.
    ///
    /// The success rate is calculated as the number of fulfilled requests divided by the number of fulfilled + expired requests.
    /// If the number of fulfilled + expired requests is zero, the success rate returned is `None`.
    /// address: The client address to filter requests by.
    pub async fn total_success_rate_from_client(&self, address: Address) -> Result<Option<f64>> {
        let fulfilled = self.total_fulfillments_from_client(address).await?;
        let expired: i64 = self.fetch_total_requests_expired_from(address).await?;

        if (fulfilled + expired) == 0 {
            return Ok(None);
        }

        Ok(Some(fulfilled as f64 / (fulfilled + expired) as f64))
    }
}
