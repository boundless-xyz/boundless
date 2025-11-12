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

use std::sync::Arc;

use crate::db::{AnyDb, DbError, DbObj};
use sqlx::any::install_default_drivers;
use sqlx::AnyPool;
use tempfile::NamedTempFile;

pub struct TestDb {
    pub db: Arc<AnyDb>,
    pub db_url: String,
    pub pool: AnyPool,
    _temp_file: Option<NamedTempFile>,
}

impl TestDb {
    pub async fn new() -> Result<Self, DbError> {
        install_default_drivers();

        // Check for PostgreSQL via INDEXER_DATABASE_URL
        // This is only supported for testing with --test-threads=1
        // And used as a way to sanity check queries are working correctly with postgres
        if let Ok(db_url) = std::env::var("INDEXER_DATABASE_URL") {
            if db_url.starts_with("postgres") {
                let pool = AnyPool::connect(&db_url).await?;
                let db = Arc::new(AnyDb::new(&db_url).await?);
                let test_db = Self {
                    db,
                    db_url,
                    pool,
                    _temp_file: None
                };
                // Clean up any leftover data from previous test runs
                test_db.cleanup().await?;
                tracing::info!("Testing with Postgres. Must only run with --test-threads=1");
                return Ok(test_db);
            }
        }

        // Default: SQLite with temp file
        let temp_file = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite:{}", temp_file.path().display());
        let pool = AnyPool::connect(&db_url).await?;
        let db = Arc::new(AnyDb::new(&db_url).await?);

        Ok(Self { db, db_url: db_url.clone(), pool, _temp_file: Some(temp_file) })
    }

    pub fn get_db(&self) -> DbObj {
        self.db.clone()
    }

    pub async fn cleanup(&self) -> Result<(), DbError> {
        // Only needed for PostgreSQL (SQLite uses temp files that are auto-cleaned)
        if self.db_url.starts_with("postgres") {
            let tables = vec![
                "proof_requests",
                "transactions",
                "request_submitted_events",
                "request_locked_events",
                "proof_delivered_events",
                "request_fulfilled_events",
                "prover_slashed_events",
                "callback_failed_events",
                "assessor_receipts",
                "fulfillments",
                "request_status",
                "hourly_market_summary",
                "daily_market_summary",
                "weekly_market_summary",
                "monthly_market_summary",
                "blocks",
                "order_stream_state",
            ];

            for table in tables {
                // Ignore errors if table doesn't exist (may not have run migrations yet)
                let _ = sqlx::query(&format!("TRUNCATE TABLE {} CASCADE", table))
                    .execute(&self.pool)
                    .await;
            }
        }
        Ok(())
    }
}
