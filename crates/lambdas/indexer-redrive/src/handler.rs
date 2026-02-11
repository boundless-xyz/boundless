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

use anyhow::{Context, Result};
use lambda_runtime::{Error, LambdaEvent};
use serde::{Deserialize, Serialize};
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::env;
use std::str::FromStr;
use tracing::{info, instrument};

const SECONDS_PER_DAY: i64 = 86400;
const STUCK_EXECUTING_THRESHOLD_SECS: i64 = 3600;

#[derive(Deserialize, Debug)]
pub struct RedriveEvent {
    #[serde(default)]
    pub requestor: Option<String>,
    pub lookback_days: u32,
    #[serde(default)]
    pub include_stuck_executing: Option<bool>,
    #[serde(default)]
    pub dry_run: Option<bool>,
}

#[derive(Serialize)]
pub struct RedriveResponse {
    pub redriven_count: u64,
    pub message: String,
}

#[instrument(skip_all, err(Debug))]
pub async fn function_handler(event: LambdaEvent<RedriveEvent>) -> Result<RedriveResponse, Error> {
    let payload = event.payload;
    let db_url = env::var("DB_URL").context("DB_URL environment variable is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(sqlx::postgres::PgConnectOptions::from_str(&db_url)?)
        .await
        .context("Failed to connect to database")?;

    let lookback_secs = i64::from(payload.lookback_days) * SECONDS_PER_DAY;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("System time before UNIX_EPOCH")?
        .as_secs() as i64;
    let created_since = now_secs - lookback_secs;
    let stuck_before = now_secs - STUCK_EXECUTING_THRESHOLD_SECS;
    let dry_run = payload.dry_run.unwrap_or(false);
    let include_stuck = payload.include_stuck_executing.unwrap_or(false);

    let mut total: u64 = 0;
    let requestor = payload.requestor.as_deref();

    let failed_list = list_failed_for_redrive(&pool, created_since, requestor).await?;
    if !failed_list.is_empty() {
        let request_ids: Vec<String> = failed_list
            .iter()
            .map(|(_, id)| id.clone().unwrap_or_else(|| "".to_string()))
            .collect();
        let action = if dry_run { "Would redrive" } else { "Redriving" };
        info!(
            count = failed_list.len(),
            request_ids = ?request_ids,
            "{} FAILED cycle_counts (request_id list above)",
            action
        );
    }
    total += failed_list.len() as u64;

    if include_stuck {
        let stuck_list =
            list_stuck_executing_for_redrive(&pool, stuck_before, created_since, requestor).await?;
        if !stuck_list.is_empty() {
            let request_ids: Vec<String> = stuck_list
                .iter()
                .map(|(_, id)| id.clone().unwrap_or_else(|| "".to_string()))
                .collect();
            let action = if dry_run { "Would redrive" } else { "Redriving" };
            info!(
                count = stuck_list.len(),
                request_ids = ?request_ids,
                "{} stuck EXECUTING cycle_counts (request_id list above)",
                action
            );
        }
        total += stuck_list.len() as u64;
    }

    if dry_run {
        return Ok(RedriveResponse {
            redriven_count: total,
            message: format!("Dry run: would redrive {} row(s)", total),
        });
    }

    let failed_affected = redrive_failed(&pool, now_secs, created_since, requestor).await?;
    let stuck_affected = if include_stuck {
        redrive_stuck_executing(&pool, now_secs, stuck_before, created_since, requestor).await?
    } else {
        0
    };
    total = failed_affected + stuck_affected;

    Ok(RedriveResponse { redriven_count: total, message: format!("Redrove {} row(s)", total) })
}

async fn list_failed_for_redrive(
    pool: &PgPool,
    created_since: i64,
    requestor: Option<&str>,
) -> Result<Vec<(String, Option<String>)>> {
    let rows = sqlx::query(
        r#"
        SELECT cc.request_digest, rs.request_id
        FROM cycle_counts cc
        JOIN request_status rs ON cc.request_digest = rs.request_digest
        WHERE cc.cycle_status = 'FAILED'
          AND rs.created_at >= $1
          AND ($2::TEXT IS NULL OR rs.client_address = $2)
        ORDER BY rs.created_at DESC
        "#,
    )
    .bind(created_since)
    .bind(requestor)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let digest: String = r.get("request_digest");
            let id: Option<String> = r.get("request_id");
            (digest, id)
        })
        .collect())
}

async fn list_stuck_executing_for_redrive(
    pool: &PgPool,
    stuck_before: i64,
    created_since: i64,
    requestor: Option<&str>,
) -> Result<Vec<(String, Option<String>)>> {
    let rows = sqlx::query(
        r#"
        SELECT cc.request_digest, rs.request_id
        FROM cycle_counts cc
        JOIN request_status rs ON cc.request_digest = rs.request_digest
        WHERE cc.cycle_status = 'EXECUTING'
          AND cc.updated_at < $1
          AND rs.created_at >= $2
          AND ($3::TEXT IS NULL OR rs.client_address = $3)
        ORDER BY rs.created_at DESC
        "#,
    )
    .bind(stuck_before)
    .bind(created_since)
    .bind(requestor)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let digest: String = r.get("request_digest");
            let id: Option<String> = r.get("request_id");
            (digest, id)
        })
        .collect())
}

async fn redrive_failed(
    pool: &PgPool,
    now_secs: i64,
    created_since: i64,
    requestor: Option<&str>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE cycle_counts cc
        SET cycle_status = 'PENDING', session_uuid = NULL, updated_at = $1
        FROM request_status rs
        WHERE cc.request_digest = rs.request_digest
          AND cc.cycle_status = 'FAILED'
          AND rs.created_at >= $2
          AND ($3::TEXT IS NULL OR rs.client_address = $3)
        "#,
    )
    .bind(now_secs)
    .bind(created_since)
    .bind(requestor)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

async fn redrive_stuck_executing(
    pool: &PgPool,
    now_secs: i64,
    stuck_before: i64,
    created_since: i64,
    requestor: Option<&str>,
) -> Result<u64> {
    let result = sqlx::query(
        r#"
        UPDATE cycle_counts cc
        SET cycle_status = 'PENDING', session_uuid = NULL, updated_at = $1
        FROM request_status rs
        WHERE cc.request_digest = rs.request_digest
          AND cc.cycle_status = 'EXECUTING'
          AND cc.updated_at < $2
          AND rs.created_at >= $3
          AND ($4::TEXT IS NULL OR rs.client_address = $4)
        "#,
    )
    .bind(now_secs)
    .bind(stuck_before)
    .bind(created_since)
    .bind(requestor)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
