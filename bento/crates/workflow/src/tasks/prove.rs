// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::Segment;
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

    // Try to deserialize as segment first, then prove it
    let segment_receipt = match deserialize_obj::<Segment>(&segment_vec) {
        Ok(segment) => {
            // Successfully deserialized as segment, now prove it
            agent
                .prover
                .as_ref()
                .context("Missing prover from prove task")?
                .prove_segment(&agent.verifier_ctx, &segment)
                .context("Failed to prove segment")?
        }
        Err(_) => {
            // Failed to deserialize as segment, try as already-proven receipt
            deserialize_obj(&segment_vec)
                .context("Failed to deserialize segment data from redis")?
        }
    };

    tracing::debug!("Completed proof: {job_id} - {index}");
    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    let serialized_receipt =
        serialize_obj(&segment_receipt).context("Failed to serialize segment receipt")?;
    redis::set_key_with_expiry(
        &mut conn,
        &output_key,
        serialized_receipt,
        Some(agent.args.redis_ttl),
    )
    .await?;

    Ok(())
}
