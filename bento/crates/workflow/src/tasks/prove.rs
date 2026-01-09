// Copyright 2026 Boundless Foundation, Inc.
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
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{ProveReq, metrics::helpers};

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let start_time = Instant::now();
    let index = request.index;
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::debug!("Starting proof of idx: {job_id} - {index}");

    // Record Redis operation for segment retrieval
    let segment_vec: Vec<u8> = match redis::get_key(&mut conn, &segment_key).await {
        Ok(data) => data,
        Err(e) => {
            return Err(anyhow::anyhow!(e)
                .context(format!("segment data not found for segment key: {segment_key}")));
        }
    };

    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    // Record proving operation
    let prove_start = Instant::now();
    let segment_receipt = match agent
        .prover
        .as_ref()
        .context("[BENTO-PROVE-002] Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
    {
        Ok(receipt) => receipt,
        Err(e) => return Err(e),
    };
    let prove_elapsed_time = prove_start.elapsed().as_secs_f64();
    helpers::record_task_operation("prove", "prove_segment", "success", prove_elapsed_time);

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-PROVE-004] Failed to verify segment receipt integrity")?;

    helpers::record_task("prove", "prove_segment", "success", prove_elapsed_time);

    tracing::debug!("Completed proof: {job_id} - {index}");

    tracing::debug!("lifting {job_id} - {index}");

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    if agent.is_povw_enabled() {
        let lift_povw_start = Instant::now();
        let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = match agent
            .prover
            .as_ref()
            .context("[BENTO-PROVE-005] Missing prover from resolve task")?
            .lift_povw(&segment_receipt)
        {
            Ok(receipt) => receipt,
            Err(e) => return Err(e),
        };
        let lift_povw_elapsed_time = lift_povw_start.elapsed().as_secs_f64();

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("Failed to verify lift receipt integrity")?;
        helpers::record_task_operation("prove", "lift_povw", "success", lift_povw_elapsed_time);

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted POVW receipt
        let lift_asset =
            serialize_obj(&lift_receipt).expect("Failed to serialize the POVW segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await
            .context("Failed to set POVW receipt key with expiry")?;
    } else {
        let lift_receipt: SuccinctReceipt<ReceiptClaim> = match agent
            .prover
            .as_ref()
            .context("[BENTO-PROVE-008] Missing prover from resolve task")?
            .lift(&segment_receipt)
        {
            Ok(receipt) => receipt,
            Err(e) => return Err(e),
        };

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVE-010] Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted regular receipt
        let lift_asset = serialize_obj(&lift_receipt).expect("Failed to serialize the segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await
            .context("Failed to set receipt key with expiry")?;
    }

    // Clean up segment
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&segment_key).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(
        "unlink",
        cleanup_status,
        cleanup_start.elapsed().as_secs_f64(),
    );
    cleanup_result.map_err(|e| anyhow::anyhow!(e).context("Failed to delete segment key"))?;

    // Record total task duration and success
    helpers::record_task_operation(
        "prove",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(())
}
