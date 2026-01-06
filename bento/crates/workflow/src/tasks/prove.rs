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
use deadpool_redis::redis::{self as redis_crate, pipe};
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

    tracing::debug!("All proofs complete, starting parallel lifts");

    // Step 2: Lift all segments in parallel using blocking tasks
    let lift_start = Instant::now();
    let prover = agent.prover.clone().context("[BENTO-PROVE-BATCH-003] Missing prover")?;
    let verifier_ctx = agent.verifier_ctx.clone();
    let is_povw = agent.is_povw_enabled();

    let mut lift_tasks = Vec::with_capacity(batch_size);
    for (index, segment_receipt) in segment_receipts {
        let prover = prover.clone();
        let verifier_ctx = verifier_ctx.clone();

        let task = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<u8>)> {
            if is_povw {
                let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> =
                    prover.lift_povw(&segment_receipt)?;
                lift_receipt
                    .verify_integrity_with_context(&verifier_ctx)
                    .context("[BENTO-PROVE-BATCH-004] Failed to verify POVW lift integrity")?;
                Ok((index, serialize_obj(&lift_receipt).expect("Failed to serialize POVW")))
            } else {
                let lift_receipt: SuccinctReceipt<ReceiptClaim> =
                    prover.lift(&segment_receipt)?;
                lift_receipt
                    .verify_integrity_with_context(&verifier_ctx)
                    .context("[BENTO-PROVE-BATCH-005] Failed to verify lift integrity")?;
                Ok((index, serialize_obj(&lift_receipt).expect("Failed to serialize")))
            }
        });

        lift_tasks.push(task);
    }

    // Wait for all lifts to complete
    let mut lift_results = Vec::with_capacity(batch_size);
    for task in lift_tasks {
        let (index, lift_data) = task.await.context("Lift task panicked")??;
        lift_results.push((index, lift_data));
        tracing::debug!("Completed lift for segment {index}");
    }

    // Sort by index to maintain order
    lift_results.sort_by_key(|(index, _)| *index);
    let receipts: Vec<Vec<u8>> = lift_results.into_iter().map(|(_, data)| data).collect();

    let lift_elapsed = lift_start.elapsed().as_secs_f64();
    let lift_op = if is_povw { "lift_povw_batch" } else { "lift_batch" };
    helpers::record_task_operation("prove", lift_op, "success", lift_elapsed);
    tracing::debug!("All {batch_size} lifts complete in {:?}", lift_start.elapsed());

    // Pipeline SET operations for all receipts
    let pipeline_start = Instant::now();
    let mut pipe = pipe();
    for (i, (task_id, _)) in requests.iter().enumerate() {
        let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");
        pipe.set_ex(&output_key, &receipts[i], agent.args.redis_ttl as usize);
    }
    pipe.query_async(&mut *conn)
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
