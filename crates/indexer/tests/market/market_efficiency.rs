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

//! Integration tests for market efficiency indexer

use alloy::primitives::B256;
use boundless_indexer::efficiency::{MarketEfficiencyService, MarketEfficiencyServiceConfig};
use std::time::Duration;

use super::common::get_db_url_from_pool;

#[test_log::test(sqlx::test(migrations = "./migrations"))]
async fn test_efficiency_service_populates_tables(pool: sqlx::PgPool) {
    let db_url = get_db_url_from_pool(&pool).await;
    let zero_padded = format!("{:0>78}", 0u32);
    let base_fee_padded = format!("{:0>78}", 1_000_000_000u64);
    let request_id_hex = format!("{:0>78}", format!("{:x}", 1u64));

    let digest1 = format!("{:x}", B256::from([10u8; 32]));
    let digest2 = format!("{:x}", B256::from([11u8; 32]));

    let created_at = 1800000000u64;
    let locked_at = 1800000100i64;
    let fulfilled_at = 1800000200i64;
    let lock_end = 1800001000i64;
    let ramp_up_start = 0i64;
    let ramp_up_period = 1i64;

    sqlx::query(
        r#"
        INSERT INTO request_status (
            request_digest, request_id, request_status, slashed_status, source, client_address,
            created_at, updated_at, locked_at, fulfilled_at, min_price, max_price, lock_collateral,
            ramp_up_start, ramp_up_period, expires_at, lock_end,
            program_cycles, lock_price, lock_price_per_cycle, lock_base_fee,
            image_id, selector, predicate_type, predicate_data, input_type, input_data
        ) VALUES ($1, $2, 'fulfilled', 'N/A', 'onchain', '0x0000000000000000000000000000000000000001',
            $3, $3, $4, $5, $6, $6, $6, $7, $8, $9, $10,
            '1000', '10000', '100', $11,
            'img', '0x12345678', 'type', 'pred', 'input', 'data')
        "#,
    )
    .bind(&digest1)
    .bind(&request_id_hex)
    .bind(created_at as i64)
    .bind(locked_at)
    .bind(fulfilled_at)
    .bind(&zero_padded)
    .bind(ramp_up_start)
    .bind(ramp_up_period)
    .bind(lock_end)
    .bind(lock_end)
    .bind(&base_fee_padded)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"
        INSERT INTO request_status (
            request_digest, request_id, request_status, slashed_status, source, client_address,
            created_at, updated_at, locked_at, fulfilled_at, min_price, max_price, lock_collateral,
            ramp_up_start, ramp_up_period, expires_at, lock_end,
            program_cycles, lock_price, lock_price_per_cycle, lock_base_fee,
            image_id, selector, predicate_type, predicate_data, input_type, input_data
        ) VALUES ($1, $2, 'fulfilled', 'N/A', 'onchain', '0x0000000000000000000000000000000000000002',
            $3, $3, $4, $5, $6, $6, $6, $7, $8, $9, $10,
            '2000', '20000', '200', $11,
            'img', '0x12345678', 'type', 'pred', 'input', 'data')
        "#,
    )
    .bind(&digest2)
    .bind(&request_id_hex)
    .bind(created_at as i64)
    .bind(locked_at)
    .bind(fulfilled_at)
    .bind(&zero_padded)
    .bind(ramp_up_start)
    .bind(ramp_up_period)
    .bind(lock_end)
    .bind(lock_end)
    .bind(&base_fee_padded)
    .execute(&pool)
    .await
    .unwrap();

    let config = MarketEfficiencyServiceConfig {
        interval: Duration::from_secs(3600),
        lookback_days: 14,
        start_time: Some(created_at),
        end_time: Some(created_at + 86400),
    };
    let mut service =
        MarketEfficiencyService::new(&db_url, config).await.expect("failed to create service");
    service.run().await.expect("efficiency service run failed");

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM market_efficiency_orders")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(count.0 >= 1, "expected at least 1 row in market_efficiency_orders, got {}", count.0);
}
