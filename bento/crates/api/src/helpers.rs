// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;
use workflow_common::{
    AUX_WORK_TYPE, COPROC_WORK_TYPE, EXEC_WORK_TYPE, ExecutorResp, JOIN_WORK_TYPE, PROVE_WORK_TYPE,
};

/// Extract reserved value from api_key string
/// Format: "v1:reserved:N" where N is a positive number (e.g., "v1:reserved:5" or "v1:reserved:10")
/// Returns 0 if no reserved value is specified
pub fn extract_reserved(api_key: &str) -> i32 {
    if let Some(reserved_str) = api_key.strip_prefix("v1:reserved:")
        && let Ok(reserved) = reserved_str.parse::<i32>()
        && reserved >= 0
    {
        return reserved;
    }
    0
}

pub async fn get_or_create_streams(
    pool: &PgPool,
    user_id: &str,
) -> Result<(Uuid, Uuid, Uuid, Uuid, Uuid)> {
    let reserved = extract_reserved(user_id);
    let aux_stream = if let Some(res) = taskdb::get_stream(pool, user_id, AUX_WORK_TYPE)
        .await
        .context("Failed to get aux stream")?
    {
        res
    } else {
        tracing::info!("Creating a new aux stream for key: {user_id}");
        taskdb::create_stream(pool, AUX_WORK_TYPE, reserved, 1.0, user_id)
            .await
            .context("Failed to create taskdb aux stream")?
    };

    let exec_stream = if let Some(res) = taskdb::get_stream(pool, user_id, EXEC_WORK_TYPE)
        .await
        .context("Failed to get exec stream")?
    {
        res
    } else {
        tracing::info!("Creating a new cpu stream for key: {user_id}");
        taskdb::create_stream(pool, EXEC_WORK_TYPE, reserved, 1.0, user_id)
            .await
            .context("Failed to create taskdb exec stream")?
    };

    let gpu_prove_stream = if let Some(res) = taskdb::get_stream(pool, user_id, PROVE_WORK_TYPE)
        .await
        .context("Failed to get gpu prove stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu stream for key: {user_id}");
        taskdb::create_stream(pool, PROVE_WORK_TYPE, reserved, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu prove stream")?
    };

    let gpu_coproc_stream = if let Some(res) = taskdb::get_stream(pool, user_id, COPROC_WORK_TYPE)
        .await
        .context("Failed to get gpu coproc stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu coproc stream for key: {user_id}");
        taskdb::create_stream(pool, COPROC_WORK_TYPE, reserved, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu coproc stream")?
    };

    let gpu_join_stream = if let Some(res) = taskdb::get_stream(pool, user_id, JOIN_WORK_TYPE)
        .await
        .context("Failed to get gpu join stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu join stream for key: {user_id}");
        taskdb::create_stream(pool, JOIN_WORK_TYPE, reserved, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu join stream")?
    };

    Ok((aux_stream, exec_stream, gpu_prove_stream, gpu_coproc_stream, gpu_join_stream))
}

pub async fn get_exec_stats(pool: &PgPool, job_id: &Uuid) -> Result<ExecutorResp> {
    let res: sqlx::types::Json<ExecutorResp> =
        sqlx::query_scalar("SELECT output FROM tasks WHERE job_id = $1 AND task_id = 'init'")
            .bind(job_id)
            .fetch_one(pool)
            .await
            .context("Failed to get task output as exec response")?;

    Ok(res.0)
}
