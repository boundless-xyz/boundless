// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
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
    let segment = deserialize_obj(&segment_vec)
        .context("[BENTO-PROVE-001] Failed to deserialize segment data from redis")?;

    let segment_receipt = agent
        .prover
        .as_ref()
        .context("[BENTO-PROVE-002] Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
        .context("[BENTO-PROVE-003] Failed to prove segment")?;

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-PROVE-004] Failed to verify segment receipt integrity")?;

    tracing::debug!("Completed proof: {job_id} - {index}");

    tracing::debug!("lifting {job_id} - {index}");

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    if agent.is_povw_enabled() {
        let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = agent
            .prover
            .as_ref()
            .context("[BENTO-PROVE-005] Missing prover from resolve task")?
            .lift_povw(&segment_receipt)
            .with_context(|| format!("[BENTO-PROVE-006] Failed to POVW lift segment {index}"))?;

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVE-007] Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted POVW receipt
        let lift_asset =
            serialize_obj(&lift_receipt).expect("Failed to serialize the POVW segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await?;
    } else {
        let lift_receipt: SuccinctReceipt<ReceiptClaim> = agent
            .prover
            .as_ref()
            .context("[BENTO-PROVE-008] Missing prover from resolve task")?
            .lift(&segment_receipt)
            .with_context(|| format!("[BENTO-PROVE-009] Failed to lift segment {index}"))?;

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVE-010] Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted regular receipt
        let lift_asset = serialize_obj(&lift_receipt).expect("Failed to serialize the segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await?;
    }
    conn.unlink::<_, ()>(&segment_key)
        .await
        .context("[BENTO-PROVE-011] Failed to delete segment key")?;

    Ok(())
}
