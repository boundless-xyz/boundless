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

//! Event database operations.
//!
//! This module provides the `EventsDb` trait which extends `IndexerDb` with
//! methods for inserting blockchain events into the database.

use super::{market::IndexerDb, DbError, TxMetadata};
use alloy::primitives::{Address, B256, U256};
use async_trait::async_trait;
use sqlx::Row;

// Batch insert chunk sizes for event inserts
// Conservative sizes to avoid large statements and parameter limits
const REQUEST_SUBMITTED_EVENT_BATCH_SIZE: usize = 500;
const REQUEST_LOCKED_EVENT_BATCH_SIZE: usize = 500;
const PROOF_DELIVERED_EVENT_BATCH_SIZE: usize = 500;
const REQUEST_FULFILLED_EVENT_BATCH_SIZE: usize = 500;

/// Extension trait for event database operations.
///
/// This trait provides methods for inserting blockchain events into the database.
/// It requires `IndexerDb` for pool access and transaction handling.
#[async_trait]
pub trait EventsDb: IndexerDb {
    async fn add_request_submitted_events(
        &self,
        events: &[(B256, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert all unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = {
            let mut seen = std::collections::HashSet::new();
            events
                .iter()
                .filter_map(
                    |(_, _, metadata)| {
                        if seen.insert(metadata.tx_hash) {
                            Some(*metadata)
                        } else {
                            None
                        }
                    },
                )
                .collect()
        };

        self.add_txs(&unique_txs).await?;

        // Batch insert the events (commit per chunk)
        for chunk in events.chunks(REQUEST_SUBMITTED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO request_submitted_events (
                    request_digest,
                    request_id,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_request_locked_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert all unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = {
            let mut seen = std::collections::HashSet::new();
            events
                .iter()
                .filter_map(
                    |(_, _, _, metadata)| {
                        if seen.insert(metadata.tx_hash) {
                            Some(*metadata)
                        } else {
                            None
                        }
                    },
                )
                .collect()
        };

        self.add_txs(&unique_txs).await?;

        // Batch insert the events (commit per chunk)
        for chunk in events.chunks(REQUEST_LOCKED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO request_locked_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, prover_address, metadata) in chunk {
                // Use the exact same formatting as the individual insert
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_proof_delivered_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert proof delivered events in chunks (commit per chunk)
        for chunk in events.chunks(PROOF_DELIVERED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO proof_delivered_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_digest, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, prover_address, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_request_fulfilled_events(
        &self,
        events: &[(B256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions (before starting our transaction)
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert request fulfilled events in chunks (commit per chunk)
        for chunk in events.chunks(REQUEST_FULFILLED_EVENT_BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO request_fulfilled_events (
                    request_digest,
                    request_id,
                    prover_address,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_digest) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_digest, request_id, prover_address, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_digest:x}"))
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{prover_address:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_prover_slashed_events(
        &self,
        events: &[(U256, U256, U256, Address, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert prover slashed events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 500;
        for chunk in events.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            // First, fetch prover addresses for all request IDs in this chunk
            let request_ids: Vec<String> =
                chunk.iter().map(|(request_id, _, _, _, _)| format!("{request_id:x}")).collect();
            let prover_map: std::collections::HashMap<String, String> = {
                let mut map = std::collections::HashMap::new();
                // Query in batches to avoid parameter limits
                for request_id_batch in request_ids.chunks(500) {
                    let placeholders: Vec<String> =
                        (1..=request_id_batch.len()).map(|i| format!("${}", i)).collect();
                    let query_str = format!(
                        "SELECT request_id, prover_address FROM request_locked_events WHERE request_id IN ({})",
                        placeholders.join(", ")
                    );
                    let mut query = sqlx::query(&query_str);
                    for request_id in request_id_batch {
                        query = query.bind(request_id);
                    }
                    let rows = query.fetch_all(self.pool()).await?;
                    for row in rows {
                        let request_id: String = row.try_get("request_id")?;
                        let prover_address: String = row.try_get("prover_address")?;
                        map.insert(request_id, prover_address);
                    }
                }
                map
            };

            let mut query = String::from(
                "INSERT INTO prover_slashed_events (
                    request_id, 
                    prover_address,
                    burn_value,
                    transfer_value,
                    collateral_recipient,
                    tx_hash, 
                    block_number, 
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6,
                    params_count + 7,
                    params_count + 8
                ));
                params_count += 8;
            }
            query.push_str(" ON CONFLICT (request_id) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_id, burn_value, transfer_value, collateral_recipient, metadata) in chunk {
                let request_id_str = format!("{request_id:x}");
                let prover_address =
                    prover_map.get(&request_id_str).cloned().unwrap_or_else(|| {
                        tracing::warn!(
                            "Missing request locked event for slashed event for request id: {:x}",
                            request_id
                        );
                        format!("{:x}", Address::ZERO)
                    });
                query_builder = query_builder
                    .bind(request_id_str)
                    .bind(prover_address)
                    .bind(burn_value.to_string())
                    .bind(transfer_value.to_string())
                    .bind(format!("{collateral_recipient:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if deposits.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = deposits
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert deposit events in chunks (commit per chunk)
        for chunk in deposits.chunks(1000) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO deposit_events (
                    account,
                    value,
                    tx_hash,
                    block_number,
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if withdrawals.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = withdrawals
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert withdrawal events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in withdrawals.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO withdrawal_events (
                    account,
                    value,
                    tx_hash, 
                    block_number, 
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_collateral_deposit_events(
        &self,
        deposits: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if deposits.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = deposits
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert collateral deposit events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in deposits.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO collateral_deposit_events (
                    account,
                    value,
                    tx_hash, 
                    block_number, 
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_collateral_withdrawal_events(
        &self,
        withdrawals: &[(Address, U256, TxMetadata)],
    ) -> Result<(), DbError> {
        if withdrawals.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = withdrawals
            .iter()
            .map(|(_, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert collateral withdrawal events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 1000;
        for chunk in withdrawals.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO collateral_withdrawal_events (
                    account,
                    value,
                    tx_hash, 
                    block_number, 
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5
                ));
                params_count += 5;
            }
            query.push_str(" ON CONFLICT (account, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (account, value, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{account:x}"))
                    .bind(value.to_string())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn add_callback_failed_events(
        &self,
        events: &[(U256, Address, Vec<u8>, TxMetadata)],
    ) -> Result<(), DbError> {
        if events.is_empty() {
            return Ok(());
        }

        // First, batch insert unique transactions
        let unique_txs: Vec<TxMetadata> = events
            .iter()
            .map(|(_, _, _, metadata)| *metadata)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();

        self.add_txs(&unique_txs).await?;

        // Batch insert callback failed events in chunks (commit per chunk)
        const BATCH_SIZE: usize = 500;
        for chunk in events.chunks(BATCH_SIZE) {
            if chunk.is_empty() {
                continue;
            }

            let mut tx = self.pool().begin().await?;

            let mut query = String::from(
                "INSERT INTO callback_failed_events (
                    request_id,
                    callback_address,
                    error_data,
                    tx_hash, 
                    block_number, 
                    block_timestamp
                ) VALUES ",
            );

            let mut params_count = 0;
            for i in 0..chunk.len() {
                if i > 0 {
                    query.push_str(", ");
                }
                query.push_str(&format!(
                    "(${}, ${}, ${}, ${}, ${}, ${})",
                    params_count + 1,
                    params_count + 2,
                    params_count + 3,
                    params_count + 4,
                    params_count + 5,
                    params_count + 6
                ));
                params_count += 6;
            }
            query.push_str(" ON CONFLICT (request_id, tx_hash) DO NOTHING");

            let mut query_builder = sqlx::query(&query);
            for (request_id, callback_address, error_data, metadata) in chunk {
                query_builder = query_builder
                    .bind(format!("{request_id:x}"))
                    .bind(format!("{callback_address:x}"))
                    .bind(error_data.clone())
                    .bind(format!("{:x}", metadata.tx_hash))
                    .bind(metadata.block_number as i64)
                    .bind(metadata.block_timestamp as i64);
            }

            query_builder.execute(&mut *tx).await?;
            tx.commit().await?;
        }

        Ok(())
    }
}

// Blanket implementation: any type that implements IndexerDb automatically implements EventsDb
impl<T: IndexerDb + Send + Sync> EventsDb for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestDb;
    use alloy::primitives::{Address, B256, U256};

    async fn get_db_url_from_pool(pool: &sqlx::PgPool) -> String {
        let base_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for sqlx::test");
        let db_name: String = sqlx::query_scalar("SELECT current_database()")
            .fetch_one(pool)
            .await
            .expect("failed to query current_database()");

        if let Some(last_slash) = base_url.rfind('/') {
            format!("{}/{}", &base_url[..last_slash], db_name)
        } else {
            format!("{}/{}", base_url, db_name)
        }
    }

    async fn test_db(pool: sqlx::PgPool) -> TestDb {
        let db_url = get_db_url_from_pool(&pool).await;
        TestDb::from_pool(db_url, pool).await.unwrap()
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_request_submitted_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_request_submitted_events(&[]).await.unwrap();

        // Create test events with different request digests
        let mut events = Vec::new();
        for i in 0..15 {
            let request_digest = B256::from([i as u8; 32]);
            let request_id = U256::from(i);
            let metadata = TxMetadata::new(
                B256::from([(i + 100) as u8; 32]), // Different tx_hash for each
                Address::from([i as u8; 20]),
                1000 + i as u64,
                1234567890 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, metadata));
        }

        // Add events in batch
        db.add_request_submitted_events(&events).await.unwrap();

        // Verify all events were added correctly
        for (request_digest, request_id, metadata) in &events {
            let result =
                sqlx::query("SELECT * FROM request_submitted_events WHERE request_digest = $1")
                    .bind(format!("{request_digest:x}"))
                    .fetch_one(&test_db.pool)
                    .await
                    .unwrap();

            assert_eq!(result.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(result.get::<String, _>("tx_hash"), format!("{:x}", metadata.tx_hash));
            assert_eq!(result.get::<i64, _>("block_number"), metadata.block_number as i64);
            assert_eq!(result.get::<i64, _>("block_timestamp"), metadata.block_timestamp as i64);
        }

        // Test idempotency - adding same events again should not fail
        db.add_request_submitted_events(&events).await.unwrap();

        // Verify we still have exactly 15 events (not duplicated)
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_submitted_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 15);

        // Test with large batch to verify chunking works
        let mut large_batch = Vec::new();
        for i in 100..1200 {
            // 1100 events, will require 2 chunks
            let _request_digest = B256::from([(i % 256) as u8; 32]);
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i / 256) as u8;
            digest_bytes[1] = (i % 256) as u8;
            let unique_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let metadata = TxMetadata::new(
                B256::from([(i % 256) as u8; 32]),
                Address::from([(i % 256) as u8; 20]),
                2000 + i as u64,
                2234567890 + i as u64,
                (i % 100) as u64,
            );
            large_batch.push((unique_digest, request_id, metadata));
        }

        db.add_request_submitted_events(&large_batch).await.unwrap();

        // Verify a sample from the large batch
        let sample_index = 500;
        let (sample_digest, sample_id, _sample_metadata) = &large_batch[sample_index];
        let result =
            sqlx::query("SELECT * FROM request_submitted_events WHERE request_digest = $1")
                .bind(format!("{sample_digest:x}"))
                .fetch_one(&test_db.pool)
                .await
                .unwrap();
        assert_eq!(result.get::<String, _>("request_id"), format!("{sample_id:x}"));
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_request_locked_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_request_locked_events(&[]).await.unwrap();

        // Create test data - more than REQUEST_LOCKED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1500 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xFF; // Add a marker byte to ensure uniqueness

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                1000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_request_locked_events(&events).await.unwrap();

        // Verify events were added correctly
        for (i, (request_digest, request_id, prover, metadata)) in events.iter().enumerate() {
            let result = sqlx::query(
                "SELECT * FROM request_locked_events WHERE request_digest = $1 AND tx_hash = $2",
            )
            .bind(format!("{request_digest:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Event {} should exist", i);
            let row = result.unwrap();

            let db_request_id = row.get::<String, _>("request_id");
            let expected_request_id = format!("{request_id:x}");
            assert_eq!(db_request_id, expected_request_id, "Request ID mismatch at index {}", i);
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_locked_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_request_locked_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_locked_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1500);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_proof_delivered_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_proof_delivered_events(&[]).await.unwrap();

        // Create test data - more than PROOF_DELIVERED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1200 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xEE; // Different marker byte from the other test

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                2000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_proof_delivered_events(&events).await.unwrap();

        // Verify events were added correctly
        for (i, (request_digest, request_id, prover, metadata)) in events.iter().enumerate() {
            let result = sqlx::query(
                "SELECT * FROM proof_delivered_events WHERE request_digest = $1 AND tx_hash = $2",
            )
            .bind(format!("{request_digest:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some(), "Event {} should exist", i);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_delivered_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_proof_delivered_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM proof_delivered_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1200);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_request_fulfilled_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_request_fulfilled_events(&[]).await.unwrap();

        // Create test data - more than REQUEST_FULFILLED_EVENT_BATCH_SIZE to test chunking
        let mut events = Vec::new();
        for i in 0..1000 {
            // Create unique request_digest using multiple bytes to avoid collisions
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = (i % 256) as u8;
            digest_bytes[1] = ((i / 256) % 256) as u8;
            digest_bytes[2] = ((i / 65536) % 256) as u8;
            let request_digest = B256::from(digest_bytes);

            let request_id = U256::from(i);
            let prover = Address::from([(i % 256) as u8; 20]);

            // Create unique tx_hash to avoid collisions
            let mut tx_hash_bytes = [0u8; 32];
            tx_hash_bytes[0] = ((i + 1) % 256) as u8;
            tx_hash_bytes[1] = (((i + 1) / 256) % 256) as u8;
            tx_hash_bytes[2] = (((i + 1) / 65536) % 256) as u8;
            tx_hash_bytes[3] = 0xDD; // Different marker byte from the other tests

            let metadata = TxMetadata::new(
                B256::from(tx_hash_bytes),
                Address::from([100; 20]),
                3000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_digest, request_id, prover, metadata));
        }

        // Add events in batch
        db.add_request_fulfilled_events(&events).await.unwrap();

        // Verify events were added correctly - note: only check first few due to ON CONFLICT
        // Since request_fulfilled_events has ON CONFLICT (request_digest) DO NOTHING,
        // only the first occurrence of each request_digest will be inserted
        let mut seen_digests = std::collections::HashSet::new();
        for (request_digest, request_id, prover, metadata) in &events {
            if !seen_digests.insert(*request_digest) {
                // Skip duplicates
                continue;
            }

            let result =
                sqlx::query("SELECT * FROM request_fulfilled_events WHERE request_digest = $1")
                    .bind(format!("{request_digest:x}"))
                    .fetch_optional(&test_db.pool)
                    .await
                    .unwrap();

            assert!(result.is_some(), "Event with digest {:x} should exist", request_digest);
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("request_id"), format!("{request_id:x}"));
            assert_eq!(row.get::<String, _>("prover_address"), format!("{prover:x}"));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify count - should be 1000 unique events (all are unique now)
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_fulfilled_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1000);

        // Test idempotency - adding same events should not fail and not duplicate
        db.add_request_fulfilled_events(&events).await.unwrap();

        let count_result = sqlx::query("SELECT COUNT(*) as count FROM request_fulfilled_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 1000); // Still 1000 unique events
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_prover_slashed_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_prover_slashed_events(&[]).await.unwrap();

        // First, we need to add locked events since add_prover_slashed_events looks up prover addresses
        let mut locked_events = Vec::new();
        for i in 0..10 {
            let mut digest_bytes = [0u8; 32];
            digest_bytes[0] = i as u8;
            let request_digest = B256::from(digest_bytes);
            let request_id = U256::from(i);
            let prover = Address::from([(i + 50) as u8; 20]);

            let metadata = TxMetadata::new(
                B256::from([(i + 100) as u8; 32]),
                Address::from([100; 20]),
                1000 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            locked_events.push((request_digest, request_id, prover, metadata));
        }
        db.add_request_locked_events(&locked_events).await.unwrap();

        // Now create slashed events for the same request IDs
        let mut slashed_events = Vec::new();
        for i in 0..10 {
            let request_id = U256::from(i);
            let burn_value = U256::from(100 + i);
            let transfer_value = U256::from(50 + i);
            let collateral_recipient = Address::from([(i + 200) as u8; 20]);

            let metadata = TxMetadata::new(
                B256::from([(i + 150) as u8; 32]),
                Address::from([100; 20]),
                2000 + i as u64,
                1600000000 + 1000 + i as u64,
                i as u64,
            );
            slashed_events.push((
                request_id,
                burn_value,
                transfer_value,
                collateral_recipient,
                metadata,
            ));
        }

        // Add slashed events
        db.add_prover_slashed_events(&slashed_events).await.unwrap();

        // Verify events were added correctly with prover addresses from locked events
        for (i, (request_id, burn_value, transfer_value, collateral_recipient, metadata)) in
            slashed_events.iter().enumerate()
        {
            let result = sqlx::query("SELECT * FROM prover_slashed_events WHERE request_id = $1")
                .bind(format!("{request_id:x}"))
                .fetch_optional(&test_db.pool)
                .await
                .unwrap();

            assert!(result.is_some(), "Slashed event {} should exist", i);
            let row = result.unwrap();

            // Verify prover address was looked up from locked events
            let expected_prover = Address::from([(i + 50) as u8; 20]);
            assert_eq!(row.get::<String, _>("prover_address"), format!("{expected_prover:x}"));
            assert_eq!(row.get::<String, _>("burn_value"), burn_value.to_string());
            assert_eq!(row.get::<String, _>("transfer_value"), transfer_value.to_string());
            assert_eq!(
                row.get::<String, _>("collateral_recipient"),
                format!("{collateral_recipient:x}")
            );
            assert_eq!(row.get::<String, _>("tx_hash"), format!("{:x}", metadata.tx_hash));
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM prover_slashed_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);

        // Test idempotency
        db.add_prover_slashed_events(&slashed_events).await.unwrap();
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM prover_slashed_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_deposit_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty deposits - should not fail
        db.add_deposit_events(&[]).await.unwrap();

        // Create test deposits
        let mut deposits = Vec::new();
        for i in 0..10 {
            let account = Address::from([i as u8; 20]);
            let value = U256::from(1000 + i);
            let metadata = TxMetadata::new(
                B256::from([i as u8; 32]),
                Address::from([100; 20]),
                100 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            deposits.push((account, value, metadata));
        }

        // Add deposits in batch
        db.add_deposit_events(&deposits).await.unwrap();

        // Verify deposits were added correctly
        for (account, value, metadata) in &deposits {
            let result =
                sqlx::query("SELECT * FROM deposit_events WHERE account = $1 AND tx_hash = $2")
                    .bind(format!("{account:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .fetch_optional(&test_db.pool)
                    .await
                    .unwrap();

            assert!(result.is_some());
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("value"), value.to_string());
            assert_eq!(row.get::<i64, _>("block_number"), metadata.block_number as i64);
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM deposit_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_withdrawal_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty withdrawals - should not fail
        db.add_withdrawal_events(&[]).await.unwrap();

        // Create test withdrawals
        let mut withdrawals = Vec::new();
        for i in 0..10 {
            let account = Address::from([i as u8; 20]);
            let value = U256::from(500 + i);
            let metadata = TxMetadata::new(
                B256::from([(i + 50) as u8; 32]),
                Address::from([100; 20]),
                200 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            withdrawals.push((account, value, metadata));
        }

        // Add withdrawals in batch
        db.add_withdrawal_events(&withdrawals).await.unwrap();

        // Verify withdrawals were added correctly
        for (account, value, metadata) in &withdrawals {
            let result =
                sqlx::query("SELECT * FROM withdrawal_events WHERE account = $1 AND tx_hash = $2")
                    .bind(format!("{account:x}"))
                    .bind(format!("{:x}", metadata.tx_hash))
                    .fetch_optional(&test_db.pool)
                    .await
                    .unwrap();

            assert!(result.is_some());
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("value"), value.to_string());
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM withdrawal_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_collateral_deposit_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty deposits - should not fail
        db.add_collateral_deposit_events(&[]).await.unwrap();

        // Create test deposits
        let mut deposits = Vec::new();
        for i in 0..10 {
            let account = Address::from([(i + 10) as u8; 20]);
            let value = U256::from(2000 + i);
            let metadata = TxMetadata::new(
                B256::from([(i + 100) as u8; 32]),
                Address::from([100; 20]),
                300 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            deposits.push((account, value, metadata));
        }

        // Add deposits in batch
        db.add_collateral_deposit_events(&deposits).await.unwrap();

        // Verify deposits were added correctly
        for (account, value, metadata) in &deposits {
            let result = sqlx::query(
                "SELECT * FROM collateral_deposit_events WHERE account = $1 AND tx_hash = $2",
            )
            .bind(format!("{account:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some());
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("value"), value.to_string());
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM collateral_deposit_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_collateral_withdrawal_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty withdrawals - should not fail
        db.add_collateral_withdrawal_events(&[]).await.unwrap();

        // Create test withdrawals
        let mut withdrawals = Vec::new();
        for i in 0..10 {
            let account = Address::from([(i + 20) as u8; 20]);
            let value = U256::from(1500 + i);
            let metadata = TxMetadata::new(
                B256::from([(i + 150) as u8; 32]),
                Address::from([100; 20]),
                400 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            withdrawals.push((account, value, metadata));
        }

        // Add withdrawals in batch
        db.add_collateral_withdrawal_events(&withdrawals).await.unwrap();

        // Verify withdrawals were added correctly
        for (account, value, metadata) in &withdrawals {
            let result = sqlx::query(
                "SELECT * FROM collateral_withdrawal_events WHERE account = $1 AND tx_hash = $2",
            )
            .bind(format!("{account:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some());
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("value"), value.to_string());
        }

        // Verify total count
        let count_result =
            sqlx::query("SELECT COUNT(*) as count FROM collateral_withdrawal_events")
                .fetch_one(&test_db.pool)
                .await
                .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }

    #[sqlx::test(migrations = "./migrations")]
    async fn test_add_callback_failed_events(pool: sqlx::PgPool) {
        let test_db = test_db(pool).await;
        let db = &*test_db.db;

        // Test with empty events - should not fail
        db.add_callback_failed_events(&[]).await.unwrap();

        // Create test events
        let mut events = Vec::new();
        for i in 0..10 {
            let request_id = U256::from(i);
            let callback_address = Address::from([(i + 30) as u8; 20]);
            let error_data = vec![i as u8; 32];
            let metadata = TxMetadata::new(
                B256::from([(i + 200) as u8; 32]),
                Address::from([100; 20]),
                500 + i as u64,
                1600000000 + i as u64,
                i as u64,
            );
            events.push((request_id, callback_address, error_data, metadata));
        }

        // Add events in batch
        db.add_callback_failed_events(&events).await.unwrap();

        // Verify events were added correctly
        for (request_id, callback_address, _error_data, metadata) in &events {
            let result = sqlx::query(
                "SELECT * FROM callback_failed_events WHERE request_id = $1 AND tx_hash = $2",
            )
            .bind(format!("{request_id:x}"))
            .bind(format!("{:x}", metadata.tx_hash))
            .fetch_optional(&test_db.pool)
            .await
            .unwrap();

            assert!(result.is_some());
            let row = result.unwrap();
            assert_eq!(row.get::<String, _>("callback_address"), format!("{callback_address:x}"));
        }

        // Verify total count
        let count_result = sqlx::query("SELECT COUNT(*) as count FROM callback_failed_events")
            .fetch_one(&test_db.pool)
            .await
            .unwrap();
        assert_eq!(count_result.get::<i64, _>("count"), 10);
    }
}
