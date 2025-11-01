// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{SEGMENT_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use uuid::Uuid;
use workflow_common::ProveReq;

/// Run a prove request
pub async fn prover(
    agent: &Agent,
    job_id: &Uuid,
    _task_id: &str,
    request: &ProveReq,
) -> Result<()> {
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

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify segment receipt integrity")?;

    tracing::debug!("Completed proof: {job_id} - {index}");

    let output_key = format!("{job_prefix}:{SEGMENT_RECEIPT_PATH}:{index}");
    let serialized_receipt =
        serialize_obj(&segment_receipt).context("Failed to serialize segment receipt")?;
    redis::set_key_with_expiry(
        &mut conn,
        &output_key,
        serialized_receipt,
        Some(agent.args.redis_ttl),
    )
    .await?;
    conn.unlink::<_, ()>(&segment_key).await.context("Failed to delete segment key")?;

    Ok(())
}
