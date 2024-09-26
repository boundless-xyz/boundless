// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use anyhow::{Context, Result};
use sqlx::PgPool;
use uuid::Uuid;
use workflow_common::{
    ExecutorResp, AUX_WORK_TYPE, EXEC_WORK_TYPE, GPU_WORK_TYPE, SNARK_WORK_TYPE,
};

pub async fn get_or_create_streams(
    pool: &PgPool,
    user_id: &str,
) -> Result<(Uuid, Uuid, Uuid, Uuid)> {
    let aux_stream = if let Some(res) = taskdb::get_stream(pool, user_id, AUX_WORK_TYPE)
        .await
        .context("Failed to get aux stream")?
    {
        res
    } else {
        tracing::info!("Creating a new aux stream for key: {user_id}");
        taskdb::create_stream(pool, AUX_WORK_TYPE, 0, 1.0, user_id)
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
        taskdb::create_stream(pool, EXEC_WORK_TYPE, 0, 1.0, user_id)
            .await
            .context("Failed to create taskdb exec stream")?
    };

    let gpu_stream = if let Some(res) = taskdb::get_stream(pool, user_id, GPU_WORK_TYPE)
        .await
        .context("Failed to get gpu stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu stream for key: {user_id}");
        taskdb::create_stream(pool, GPU_WORK_TYPE, 0, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu stream")?
    };

    let snark_stream = if let Some(res) = taskdb::get_stream(pool, user_id, SNARK_WORK_TYPE)
        .await
        .context("Failed to get snark stream")?
    {
        res
    } else {
        tracing::info!("Creating a new snark stream for key: {user_id}");
        taskdb::create_stream(pool, SNARK_WORK_TYPE, 0, 1.0, user_id)
            .await
            .context("Failed to create taskdb snark stream")?
    };

    Ok((aux_stream, exec_stream, gpu_stream, snark_stream))
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
