// Copyright 2025 Boundless Foundation, Inc.
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

use crate::db::{
    market::{CycleCount, IndexerDb},
    DbError, DbObj, MarketDb,
};
use alloy::primitives::{B256, U256};
use sqlx::any::install_default_drivers;
use sqlx::AnyPool;
use tempfile::NamedTempFile;

pub struct TestDb {
    pub db: Arc<MarketDb>,
    pub db_url: String,
    pub pool: AnyPool,
    pub _temp_file: Option<NamedTempFile>,
}

impl TestDb {
    pub async fn new() -> Result<Self, DbError> {
        install_default_drivers();

        // Lets you run the DB tests against PostgreSQL, via setting INDEXER_DATABASE_URL
        // This is only supported for testing with --test-threads=1
        // This should only be used for sanity checking queries are working correctly with postgres.
        //
        // RUST_LOG="info" INDEXER_DATABASE_URL="postgres://postgres:password@localhost:5433/postgres" RISC0_DEV_MODE=1 cargo test -p boundless-indexer --lib -- --nocapture --test-threads=1
        if let Ok(db_url) = std::env::var("INDEXER_DATABASE_URL") {
            if db_url.starts_with("postgres") {
                let pool = AnyPool::connect(&db_url).await?;
                let db = Arc::new(MarketDb::new(&db_url, None, false).await?);
                let test_db = Self { db, db_url, pool, _temp_file: None };
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
        let db = Arc::new(MarketDb::new(&db_url, None, false).await?);

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
                "cycle_counts",
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

    /// Helper for tests to manually insert cycle counts for non-hardcoded requestors.
    /// This allows testing cycle aggregations and lock_price_per_cycle percentiles.
    pub async fn insert_test_cycle_counts(
        &self,
        request_digest: B256,
        program_cycles: U256,
        total_cycles: U256,
    ) -> Result<(), DbError> {
        let current_timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        let cycle_count = CycleCount {
            request_digest,
            cycle_status: "COMPLETED".to_string(),
            program_cycles: Some(program_cycles),
            total_cycles: Some(total_cycles),
            created_at: current_timestamp,
            updated_at: current_timestamp,
        };

        self.db.add_cycle_counts(&[cycle_count]).await
    }
}
