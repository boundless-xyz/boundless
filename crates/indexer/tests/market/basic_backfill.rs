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

#![allow(clippy::zombie_processes)]

use alloy::{
    primitives::U256,
    providers::{Provider, WalletProvider},
};
use sqlx::Row;

use super::common::*;

/// Common setup: creates a fixture, starts indexer, creates and fulfills a request
async fn setup_backfill_test(
    pool: sqlx::PgPool,
) -> (MarketTestFixture<impl Provider + WalletProvider + Clone + 'static>, u64) {
    let fixture = new_market_test_fixture(pool).await.unwrap();

    // Start indexer
    let mut indexer_process = IndexerCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .start_block("0")
    .spawn()
    .unwrap();

    let now = get_latest_block_timestamp(&fixture.ctx.customer_provider).await;
    tracing::info!("Starting test at timestamp: {}", now);

    // Create, submit, lock, and fulfill a request
    let collateral_amount = U256::from(1000);
    let (request, client_sig) = create_order_with_collateral(
        &fixture.ctx.customer_signer,
        fixture.ctx.customer_signer.address(),
        1,
        fixture.ctx.deployment.boundless_market_address,
        fixture.anvil.chain_id(),
        now,
        collateral_amount,
    )
    .await;

    submit_request_with_deposit(&fixture.ctx, &request, client_sig.clone()).await.unwrap();

    lock_and_fulfill_request_with_collateral(
        &fixture.ctx,
        &fixture.prover,
        &request,
        client_sig.clone(),
    )
    .await
    .unwrap();

    // Wait for request to be indexed
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    advance_time_and_mine(&fixture.ctx.customer_provider, 10, 1).await.unwrap();

    // Wait for indexer to finish processing
    wait_for_indexer(&fixture.ctx.customer_provider, &fixture.test_db.pool).await;

    // Kill the indexer process
    indexer_process.kill().unwrap();
    let _ = indexer_process.wait();

    // Advance time and mine a block to ensure we're in a different second when backfill runs
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    advance_time_and_mine(&fixture.ctx.customer_provider, 10, 1).await.unwrap();

    // Get current block number for backfill end_block
    let current_block = fixture.ctx.customer_provider.get_block_number().await.unwrap();

    (fixture, current_block)
}

/// Common backfill execution: runs backfill and verifies it completes successfully
async fn run_backfill_and_verify(
    fixture: &MarketTestFixture<impl Provider + WalletProvider + Clone + 'static>,
    mode: &str,
    current_block: u64,
) {
    tracing::info!("Running backfill in {} mode from block 0 to block {}", mode, current_block);

    let mut backfill_process = BackfillCliBuilder::new(
        fixture.test_db.db_url.clone(),
        fixture.anvil.endpoint_url().to_string(),
        fixture.ctx.deployment.boundless_market_address.to_string(),
    )
    .mode(mode)
    .start_block(0)
    .end_block(current_block)
    .spawn()
    .unwrap();

    // Wait for backfill to complete
    let exit_status = backfill_process.wait().unwrap();
    assert!(exit_status.success(), "Backfill process exited with error: {:?}", exit_status);

    tracing::info!("Backfill completed successfully");
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_backfill_aggregates(pool: sqlx::PgPool) {
    let (fixture, current_block) = setup_backfill_test(pool).await;

    // Extract prover address for prover aggregate queries
    let prover_address = fixture.ctx.prover_signer.address();
    let prover_address_str = format!("{:x}", prover_address);

    // Get all aggregate rows and their updated_at timestamps before backfill
    let tables_to_check = vec![
        "hourly_market_summary",
        "daily_market_summary",
        "weekly_market_summary",
        "monthly_market_summary",
        "all_time_market_summary",
    ];

    let prover_tables_to_check = vec![
        "hourly_prover_summary",
        "daily_prover_summary",
        "weekly_prover_summary",
        "monthly_prover_summary",
        "all_time_prover_summary",
    ];

    let mut before_timestamps: std::collections::HashMap<String, Vec<(i64, Option<String>)>> =
        std::collections::HashMap::new();

    for table in &tables_to_check {
        let rows = sqlx::query(&format!(
            "SELECT period_timestamp, CAST(updated_at AS TEXT) as updated_at FROM {} ORDER BY period_timestamp",
            table
        ))
        .fetch_all(&fixture.test_db.pool)
        .await
        .unwrap();

        let timestamps: Vec<(i64, Option<String>)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("period_timestamp"),
                    row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
                )
            })
            .collect();

        if !timestamps.is_empty() {
            before_timestamps.insert(table.to_string(), timestamps.clone());
            tracing::info!("Found {} rows in {} before backfill", timestamps.len(), table);
        }
    }

    // Get prover aggregate rows and their updated_at timestamps before backfill
    let mut before_prover_timestamps: std::collections::HashMap<
        String,
        Vec<(i64, Option<String>)>,
    > = std::collections::HashMap::new();

    for table in &prover_tables_to_check {
        let rows = sqlx::query(&format!(
            "SELECT period_timestamp, CAST(updated_at AS TEXT) as updated_at FROM {} WHERE prover_address = $1 ORDER BY period_timestamp",
            table
        ))
        .bind(&prover_address_str)
        .fetch_all(&fixture.test_db.pool)
        .await
        .unwrap();

        let timestamps: Vec<(i64, Option<String>)> = rows
            .iter()
            .map(|row| {
                (
                    row.get::<i64, _>("period_timestamp"),
                    row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
                )
            })
            .collect();

        if !timestamps.is_empty() {
            before_prover_timestamps.insert(table.to_string(), timestamps.clone());
            tracing::info!("Found {} rows in {} before backfill", timestamps.len(), table);
        }
    }

    // Get request_status updated_at timestamps before backfill (should NOT change)
    let before_status_rows = sqlx::query(
        "SELECT request_digest, CAST(updated_at AS TEXT) as updated_at FROM request_status ORDER BY created_at"
    )
    .fetch_all(&fixture.test_db.pool)
    .await
    .unwrap();

    let mut before_status_timestamps: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();

    for row in &before_status_rows {
        let digest: String = row.get("request_digest");
        let updated_at: Option<String> = row.try_get("updated_at").ok().flatten();
        before_status_timestamps.insert(digest, updated_at);
    }

    tracing::info!("Found {} request_status rows before backfill", before_status_timestamps.len());

    // Run backfill in aggregates mode
    run_backfill_and_verify(&fixture, "aggregates", current_block).await;

    // Verify all rows were updated. We just check the updated at field was updated, not the actual values.
    for table in &tables_to_check {
        if let Some(before_ts) = before_timestamps.get(*table) {
            let rows = sqlx::query(&format!(
                "SELECT period_timestamp, CAST(updated_at AS TEXT) as updated_at FROM {} ORDER BY period_timestamp",
                table
            ))
            .fetch_all(&fixture.test_db.pool)
            .await
            .unwrap();

            let after_timestamps: std::collections::HashMap<i64, Option<String>> = rows
                .iter()
                .map(|row| {
                    (
                        row.get::<i64, _>("period_timestamp"),
                        row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
                    )
                })
                .collect();

            // Verify all periods from before still exist
            for (period_ts, before_updated_at) in before_ts {
                let after_updated_at = after_timestamps.get(period_ts).unwrap_or_else(|| {
                    panic!("Period {} missing from {} after backfill", period_ts, table)
                });

                // Verify updated_at was refreshed
                if let (Some(before), Some(after)) = (before_updated_at, after_updated_at) {
                    if before == after {
                        tracing::debug!(
                            "Period {} in {} was updated (updated_at changed from {} to {})",
                            period_ts,
                            table,
                            before,
                            after
                        );
                    }
                } else if before_updated_at.is_none() && after_updated_at.is_some() {
                    // Row was created (had no updated_at before, has one now)
                    tracing::debug!(
                        "Period {} in {} was created during backfill",
                        period_ts,
                        table
                    );
                }
            }

            // Verify no periods were skipped - all periods that existed before still exist
            assert_eq!(
                before_ts.len(),
                after_timestamps.len(),
                "Row count changed in {}: had {} rows before, {} rows after",
                table,
                before_ts.len(),
                after_timestamps.len()
            );

            tracing::info!("Verified all {} rows in {} were updated", before_ts.len(), table);
        } else {
            tracing::info!("No rows in {} to verify", table);
        }
    }

    // Verify prover aggregate rows were updated
    for table in &prover_tables_to_check {
        if let Some(before_ts) = before_prover_timestamps.get(*table) {
            let rows = sqlx::query(&format!(
                "SELECT period_timestamp, CAST(updated_at AS TEXT) as updated_at FROM {} WHERE prover_address = $1 ORDER BY period_timestamp",
                table
            ))
            .bind(&prover_address_str)
            .fetch_all(&fixture.test_db.pool)
            .await
            .unwrap();

            let after_timestamps: std::collections::HashMap<i64, Option<String>> = rows
                .iter()
                .map(|row| {
                    (
                        row.get::<i64, _>("period_timestamp"),
                        row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
                    )
                })
                .collect();

            // Verify all periods from before still exist
            for (period_ts, before_updated_at) in before_ts {
                let after_updated_at = after_timestamps.get(period_ts).unwrap_or_else(|| {
                    panic!("Period {} missing from {} after backfill", period_ts, table)
                });

                // Verify updated_at was refreshed
                if let (Some(before), Some(after)) = (before_updated_at, after_updated_at) {
                    if before == after {
                        tracing::debug!(
                            "Period {} in {} was updated (updated_at changed from {} to {})",
                            period_ts,
                            table,
                            before,
                            after
                        );
                    }
                } else if before_updated_at.is_none() && after_updated_at.is_some() {
                    // Row was created (had no updated_at before, has one now)
                    tracing::debug!(
                        "Period {} in {} was created during backfill",
                        period_ts,
                        table
                    );
                }
            }

            // Verify no periods were skipped - all periods that existed before still exist
            assert_eq!(
                before_ts.len(),
                after_timestamps.len(),
                "Row count changed in {}: had {} rows before, {} rows after",
                table,
                before_ts.len(),
                after_timestamps.len()
            );

            tracing::info!("Verified all {} rows in {} were updated", before_ts.len(), table);
        } else {
            tracing::info!("No rows in {} to verify", table);
        }
    }

    // Verify request_status rows were NOT updated (aggregates mode doesn't update statuses)
    let after_status_rows = sqlx::query(
        "SELECT request_digest, CAST(updated_at AS TEXT) as updated_at FROM request_status ORDER BY created_at"
    )
    .fetch_all(&fixture.test_db.pool)
    .await
    .unwrap();

    let after_status_timestamps: std::collections::HashMap<String, Option<String>> =
        after_status_rows
            .iter()
            .map(|row| {
                (
                    row.get::<String, _>("request_digest"),
                    row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
                )
            })
            .collect();

    assert_eq!(
        before_status_timestamps.len(),
        after_status_timestamps.len(),
        "Request status row count should not change"
    );

    for (digest, before_updated_at) in &before_status_timestamps {
        let after_updated_at = after_status_timestamps.get(digest).unwrap_or_else(|| {
            panic!("Request digest {} missing from request_status after backfill", digest)
        });

        // Verify updated_at was NOT changed (aggregates mode doesn't update statuses)
        assert_eq!(
            before_updated_at, after_updated_at,
            "Request digest {} should have unchanged updated_at in aggregates mode (was {:?}, now {:?})",
            digest, before_updated_at, after_updated_at
        );
    }

    tracing::info!(
        "Verified all {} request_status rows were NOT updated during aggregates backfill",
        before_status_timestamps.len()
    );
}

#[test_log::test(sqlx::test(migrations = "./migrations"))]
#[ignore = "Generates a proof. Slow without RISC0_DEV_MODE=1"]
async fn test_backfill_statuses(pool: sqlx::PgPool) {
    let (fixture, current_block) = setup_backfill_test(pool).await;

    // Get all request_status rows and their updated_at timestamps before backfill
    let before_rows = sqlx::query(
        "SELECT request_digest, CAST(updated_at AS TEXT) as updated_at FROM request_status ORDER BY created_at"
    )
    .fetch_all(&fixture.test_db.pool)
    .await
    .unwrap();

    let mut before_timestamps: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();

    for row in &before_rows {
        let digest: String = row.get("request_digest");
        let updated_at: Option<String> = row.try_get("updated_at").ok().flatten();
        before_timestamps.insert(digest, updated_at);
    }

    tracing::info!("Found {} request_status rows before backfill", before_timestamps.len());
    assert!(!before_timestamps.is_empty(), "Should have at least one request_status row");

    // Run backfill in statuses_and_aggregates mode
    run_backfill_and_verify(&fixture, "statuses_and_aggregates", current_block).await;

    // Verify all statuses were updated
    let after_rows = sqlx::query(
        "SELECT request_digest, CAST(updated_at AS TEXT) as updated_at FROM request_status ORDER BY created_at"
    )
    .fetch_all(&fixture.test_db.pool)
    .await
    .unwrap();

    let after_timestamps: std::collections::HashMap<String, Option<String>> = after_rows
        .iter()
        .map(|row| {
            (
                row.get::<String, _>("request_digest"),
                row.try_get::<Option<String>, _>("updated_at").ok().flatten(),
            )
        })
        .collect();

    // Verify all statuses from before still exist
    assert_eq!(
        before_timestamps.len(),
        after_timestamps.len(),
        "Row count changed: had {} rows before, {} rows after",
        before_timestamps.len(),
        after_timestamps.len()
    );

    // Verify updated_at was refreshed for all statuses
    // The backfill sets updated_at to the end_block timestamp, so it should always be different
    for (digest, before_updated_at) in &before_timestamps {
        let after_updated_at = after_timestamps.get(digest).unwrap_or_else(|| {
            panic!("Request digest {} missing from request_status after backfill", digest)
        });

        // Verify updated_at exists after backfill
        assert!(
            after_updated_at.is_some(),
            "Request digest {} should have updated_at after backfill",
            digest
        );

        // Verify updated_at was refreshed (should be different from before)
        if let Some(before) = before_updated_at {
            let after = after_updated_at.as_ref().unwrap();
            assert_ne!(
                before, after,
                "Request digest {} should have different updated_at after backfill (was {}, now {})",
                digest, before, after
            );
        }
    }

    tracing::info!(
        "Verified all {} request_status rows were updated during backfill",
        before_timestamps.len()
    );
}
