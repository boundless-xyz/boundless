// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::AsyncCommands,
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use deadpool_redis::redis::pipe;
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{ProveReq, metrics::helpers};

/// Batch prove multiple segments with optimized Redis operations
pub async fn batch_prover(
    agent: &Agent,
    job_id: &Uuid,
    requests: &[(String, ProveReq)], // (task_id, request) pairs
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let start_time = Instant::now();
    let batch_size = requests.len();
    tracing::debug!("Starting batch proof for job {job_id} with {batch_size} segments");

    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Build all segment keys
    let segment_keys: Vec<String> = requests
        .iter()
        .map(|(_, req)| format!("{job_prefix}:{SEGMENTS_PATH}:{}", req.index))
        .collect();

    // MGET all segments at once
    let mget_start = Instant::now();
    let segments_data: Vec<Vec<u8>> = conn
        .mget(&segment_keys)
        .await
        .context("Failed to MGET segments")?;
    helpers::record_redis_operation("mget", "success", mget_start.elapsed().as_secs_f64());

    tracing::debug!("Fetched {batch_size} segments in {:?}", mget_start.elapsed());

    // Deserialize all segments
    let segments: Result<Vec<_>> = segments_data
        .iter()
        .enumerate()
        .map(|(i, data)| {
            deserialize_obj(data).with_context(|| {
                format!("Failed to deserialize segment at index {}", requests[i].1.index)
            })
        })
        .collect();
    let segments = segments?;

    // Step 1: Prove all segments sequentially
    let mut segment_receipts = Vec::with_capacity(batch_size);
    for (i, segment) in segments.iter().enumerate() {
        let index = requests[i].1.index;
        tracing::debug!("Proving segment {index} ({}/{batch_size})", i + 1);

        let prove_start = Instant::now();
        let segment_receipt = agent
            .prover
            .as_ref()
            .context("[BENTO-PROVE-BATCH-001] Missing prover from batch prove task")?
            .prove_segment(&agent.verifier_ctx, segment)?;
        let prove_elapsed = prove_start.elapsed().as_secs_f64();
        helpers::record_task_operation("prove", "prove_segment", "success", prove_elapsed);

        segment_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVE-BATCH-002] Failed to verify segment receipt integrity")?;

        segment_receipts.push((index, segment_receipt));
        tracing::debug!("Completed proof for segment {index}");
    }

    tracing::debug!("All proofs complete, starting lifts");

    // Step 2: Lift all segments sequentially
    let lift_start = Instant::now();
    let prover = agent.prover.as_ref().context("[BENTO-PROVE-BATCH-003] Missing prover")?;
    let verifier_ctx = &agent.verifier_ctx;
    let is_povw = agent.is_povw_enabled();

    let mut receipts = Vec::with_capacity(batch_size);
    for (index, segment_receipt) in segment_receipts {
        let lift_data = if is_povw {
            let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> =
                prover.lift_povw(&segment_receipt)?;
            lift_receipt
                .verify_integrity_with_context(verifier_ctx)
                .context("[BENTO-PROVE-BATCH-004] Failed to verify POVW lift integrity")?;
            serialize_obj(&lift_receipt).expect("Failed to serialize POVW")
        } else {
            let lift_receipt: SuccinctReceipt<ReceiptClaim> = prover.lift(&segment_receipt)?;
            lift_receipt
                .verify_integrity_with_context(verifier_ctx)
                .context("[BENTO-PROVE-BATCH-005] Failed to verify lift integrity")?;
            serialize_obj(&lift_receipt).expect("Failed to serialize")
        };

        receipts.push(lift_data);
        tracing::debug!("Completed lift for segment {index}");
    }

    let lift_elapsed = lift_start.elapsed().as_secs_f64();
    let lift_op = if is_povw { "lift_povw_batch" } else { "lift_batch" };
    helpers::record_task_operation("prove", lift_op, "success", lift_elapsed);
    tracing::debug!("All {batch_size} lifts complete in {:?}", lift_start.elapsed());

    // Pipeline SET operations for all receipts
    let pipeline_start = Instant::now();
    let mut pipe = pipe();
    for (i, (task_id, _)) in requests.iter().enumerate() {
        let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");
        pipe.set_ex(&output_key, &receipts[i], agent.args.redis_ttl);
    }
    pipe.query_async::<_, ()>(&mut *conn)
        .await
        .context("Failed to pipeline SET receipts")?;
    helpers::record_redis_operation(
        "pipeline_set",
        "success",
        pipeline_start.elapsed().as_secs_f64(),
    );

    tracing::debug!("Stored {batch_size} receipts in {:?}", pipeline_start.elapsed());

    // UNLINK all segments at once
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&segment_keys).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("unlink", cleanup_status, cleanup_start.elapsed().as_secs_f64());
    cleanup_result.context("Failed to UNLINK segment keys")?;

    tracing::debug!("Cleaned up {batch_size} segments");

    // Record total batch operation
    helpers::record_task_operation(
        "prove",
        "batch_complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    tracing::info!(
        "Batch proof complete for job {job_id}: {batch_size} segments in {:?}",
        start_time.elapsed()
    );

    Ok(())
}

/// Run a prove request (delegates to batch_prover for code reuse)
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    // Just call the batch version with a single item
    batch_prover(agent, job_id, &[(task_id.to_string(), request.clone())]).await
}
