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

use std::{sync::Arc, time::SystemTime};

use crate::db::market::u256_to_padded_string;
use crate::db::{
    market::{CycleCount, IndexerDb, TxMetadata},
    DbError, DbObj, MarketDb,
};
use alloy::primitives::{Address, B256, U256};
use boundless_market::contracts::ProofRequest;
use sqlx::PgPool;

pub struct TestDb {
    pub db: Arc<MarketDb>,
    pub db_url: String,
    pub pool: PgPool,
}

impl TestDb {
    pub async fn from_pool(db_url: String, pool: PgPool) -> Result<Self, DbError> {
        let db = Arc::new(MarketDb::new(&db_url, None, true).await?);
        Ok(Self { db, db_url, pool })
    }

    pub fn get_db(&self) -> DbObj {
        self.db.clone()
    }

    pub async fn cleanup(&self) -> Result<(), DbError> {
        Ok(())
    }

    /// Helper for tests to manually insert cycle counts for non-hardcoded requestors.
    /// This allows testing cycle aggregations and lock_price_per_cycle percentiles.
    /// Uses the timestamp of the last processed block plus 10 seconds for updated_at.
    /// Preserves existing created_at if the row already exists.
    /// Returns the updated_at timestamp that was used, so callers can ensure mining happens beyond it.
    pub async fn insert_test_cycle_counts(
        &self,
        request_digest: B256,
        program_cycles: U256,
        total_cycles: U256,
    ) -> Result<u64, DbError> {
        // Get the last processed block number
        let last_block = self.db.get_last_block().await?.expect(
            "No last block found in database - indexer must have processed at least one block",
        );

        // Get the timestamp for the last processed block, and add 10 seconds to it
        let mut updated_at = self
            .db
            .get_block_timestamp(last_block)
            .await?
            .expect("Last block timestamp not found in database");
        updated_at += 10;

        // Check if cycle_count already exists and preserve its created_at
        let digests = std::collections::HashSet::from([request_digest]);
        let existing_counts = self.db.get_cycle_counts(&digests).await?;
        let created_at = existing_counts
            .first()
            .map(|cc| cc.created_at)
            .unwrap_or(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs());

        let cycle_count = CycleCount {
            request_digest,
            cycle_status: "COMPLETED".to_string(),
            program_cycles: Some(program_cycles),
            total_cycles: Some(total_cycles),
            created_at,
            updated_at,
        };

        tracing::debug!(
            "Inserting cycle count for request digest: {:x}, created_at: {}, updated_at: {}",
            request_digest,
            created_at,
            updated_at
        );

        sqlx::query(
            "INSERT INTO cycle_counts (request_digest, cycle_status, program_cycles, total_cycles, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6)
             ON CONFLICT (request_digest) DO UPDATE SET
             cycle_status = EXCLUDED.cycle_status,
             program_cycles = EXCLUDED.program_cycles,
             total_cycles = EXCLUDED.total_cycles,
             created_at = EXCLUDED.created_at,
             updated_at = EXCLUDED.updated_at"
        )
        .bind(format!("{:x}", request_digest))
        .bind(&cycle_count.cycle_status)
        .bind(cycle_count.program_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
        .bind(cycle_count.total_cycles.as_ref().map(|c| u256_to_padded_string(*c)))
        .bind(created_at as i64)
        .bind(updated_at as i64)
        .execute(&self.pool)
        .await?;

        Ok(updated_at)
    }

    /// Helper for tests to set up proof requests and cycle counts together.
    /// This is useful for testing cycle count state transitions.
    pub async fn setup_requests_and_cycles(
        &self,
        digests: &[B256],
        requests: &[ProofRequest],
        statuses: &[&str],
    ) {
        let metadata = TxMetadata::new(B256::ZERO, Address::ZERO, 100, 1234567890, 0);
        let timestamp =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();

        self.db
            .add_proof_requests(
                &digests
                    .iter()
                    .zip(requests.iter())
                    .map(|(d, r)| {
                        (*d, r.clone(), metadata, "onchain".to_string(), metadata.block_timestamp)
                    })
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();

        let cycle_counts: Vec<CycleCount> = digests
            .iter()
            .zip(statuses.iter())
            .map(|(d, s)| CycleCount {
                request_digest: *d,
                cycle_status: s.to_string(),
                program_cycles: if *s == "COMPLETED" { Some(U256::from(1000)) } else { None },
                total_cycles: if *s == "COMPLETED" { Some(U256::from(1015)) } else { None },
                created_at: timestamp,
                updated_at: timestamp,
            })
            .collect();

        self.db.add_cycle_counts(&cycle_counts).await.unwrap();
    }
}
