// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    povw::PovwConfig,
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECUR_RECEIPT_PATH, SEGMENTS_PATH},
    Agent,
};
use anyhow::{Context, Result};
use uuid::Uuid;
use workflow_common::ProveReq;

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let index = request.index;
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::debug!("Starting proof of idx: {job_id} - {index}");
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    let segment_receipt = agent
        .prover
        .as_ref()
        .context("Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
        .context("Failed to prove segment")?;

    tracing::debug!("Completed proof: {job_id} - {index}");

    let lift_receipt = if let Some(_povw_config) = PovwConfig::from_env() {
        tracing::debug!("POVW lifting {job_id} - {index}");
        if let Some(prover) = agent.prover.as_ref() {
            if let Ok(povw_lift_receipt) = prover.lift_povw(&segment_receipt) {
                tracing::debug!("Used POVW lift method");
                serialize_obj(&povw_lift_receipt).context("Failed to serialize POVW lifted receipt")?
            } else {
                // POVW lift method not available, fall back to regular lift
                tracing::warn!("POVW lift method not available, using regular lift");
                let regular_lift_receipt = prover.lift(&segment_receipt)
                    .with_context(|| format!("Failed to lift segment {index}"))?;
                serialize_obj(&regular_lift_receipt).context("Failed to serialize regular lifted receipt")?
            }
        } else {
            return Err(anyhow::anyhow!("Missing prover from prove task"));
        }
    } else {
        tracing::debug!("lifting {job_id} - {index}");
        let regular_lift_receipt = agent
            .prover
            .as_ref()
            .context("Missing prover from prove task")?
            .lift(&segment_receipt)
            .with_context(|| format!("Failed to lift segment {index}"))?;
        serialize_obj(&regular_lift_receipt).context("Failed to serialize regular lifted receipt")?
    };

    tracing::debug!("lifting complete {job_id} - {index}");

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");
    redis::set_key_with_expiry(
        &mut conn,
        &output_key,
        lift_receipt,
        Some(agent.args.redis_ttl),
    )
    .await?;

    Ok(())
}
