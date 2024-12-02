// Copyright 2024 RISC Zero, Inc.
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
use sqlx::PgPool;
use uuid::Uuid;
use workflow_common::{
    ExecutorResp, AUX_WORK_TYPE, EXEC_WORK_TYPE, JOIN_WORK_TYPE, PROVE_WORK_TYPE, SNARK_WORK_TYPE,
};

pub async fn get_or_create_streams(
    pool: &PgPool,
    user_id: &str,
) -> Result<(Uuid, Uuid, Uuid, Uuid, Uuid)> {
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

    let gpu_prove_stream = if let Some(res) = taskdb::get_stream(pool, user_id, PROVE_WORK_TYPE)
        .await
        .context("Failed to get gpu prove stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu stream for key: {user_id}");
        taskdb::create_stream(pool, PROVE_WORK_TYPE, 0, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu prove stream")?
    };

    let gpu_join_stream = if let Some(res) = taskdb::get_stream(pool, user_id, JOIN_WORK_TYPE)
        .await
        .context("Failed to get gpu join stream")?
    {
        res
    } else {
        tracing::info!("Creating a new gpu join stream for key: {user_id}");
        taskdb::create_stream(pool, JOIN_WORK_TYPE, 0, 1.0, user_id)
            .await
            .context("Failed to create taskdb gpu join stream")?
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

    Ok((aux_stream, exec_stream, gpu_prove_stream, gpu_join_stream, snark_stream))
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
