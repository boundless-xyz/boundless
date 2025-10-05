// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
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
    _user_id: &str,
) -> Result<(String, String, String, String, String)> {
    // Create streams for each worker type - they will be reused if they already exist
    tracing::info!("Creating streams for worker types");

    let aux_stream =
        tdb::create_stream(AUX_WORK_TYPE).await.context("Failed to create aux stream")?;

    let exec_stream =
        tdb::create_stream(EXEC_WORK_TYPE).await.context("Failed to create exec stream")?;

    let gpu_prove_stream =
        tdb::create_stream(PROVE_WORK_TYPE).await.context("Failed to create gpu prove stream")?;

    let gpu_coproc_stream =
        tdb::create_stream(COPROC_WORK_TYPE).await.context("Failed to create gpu coproc stream")?;

    let gpu_join_stream =
        tdb::create_stream(JOIN_WORK_TYPE).await.context("Failed to create gpu join stream")?;

    Ok((aux_stream, exec_stream, gpu_prove_stream, gpu_coproc_stream, gpu_join_stream))
}

pub async fn get_exec_stats(job_id: &Uuid) -> Result<ExecutorResp> {
    // For Redis-only implementation, we'll return a default response
    // In a real implementation, you might want to store this data in Redis
    // or retrieve it from the task output in Redis
    Ok(ExecutorResp {
        user_cycles: 0,
        segments: 0,
        total_cycles: 0,
        assumption_count: 0,
        povw_log_id: None,
        povw_job_number: None,
    })
}
