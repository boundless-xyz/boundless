// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use deadpool_redis::redis::pipe;
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{JoinReq, metrics::helpers};

/// Batch join multiple receipt pairs with optimized Redis operations
pub async fn batch_join(
    agent: &Agent,
    job_id: &Uuid,
    requests: &[(String, JoinReq)], // (task_id, request) pairs
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let start_time = Instant::now();
    let batch_size = requests.len();
    tracing::debug!("Starting batch join for job {job_id} with {batch_size} joins");

    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let recur_receipts_prefix = format!("{job_prefix}:{RECUR_RECEIPT_PATH}");

    // Collect all unique input receipt keys needed across all joins
    let mut unique_keys = HashSet::new();
    for (_, req) in requests {
        unique_keys.insert(req.left.clone());
        unique_keys.insert(req.right.clone());
    }

    // Convert to Vec to maintain consistent ordering
    let unique_keys_vec: Vec<String> = unique_keys.into_iter().collect();
    let keys: Vec<String> = unique_keys_vec
        .iter()
        .map(|k| format!("{recur_receipts_prefix}:{k}"))
        .collect();

    // MGET all unique input receipts at once
    let mget_start = Instant::now();
    let receipts_data: Vec<Vec<u8>> = conn
        .mget(&keys)
        .await
        .context("Failed to MGET join input receipts")?;
    helpers::record_redis_operation("mget", "success", mget_start.elapsed().as_secs_f64());

    tracing::debug!("Fetched {} unique receipts for {batch_size} joins in {:?}",
        keys.len(), mget_start.elapsed());

    // Build a map of receipt_id -> receipt data
    let mut receipt_map: HashMap<String, Vec<u8>> = HashMap::new();
    for (i, key) in unique_keys_vec.iter().enumerate() {
        receipt_map.insert(key.clone(), receipts_data[i].clone());
    }

    // Process each join sequentially
    let mut join_results = Vec::with_capacity(batch_size);
    let mut keys_to_cleanup = Vec::new();

    for (task_id, req) in requests {
        tracing::debug!("Joining {} + {} -> {}", req.left, req.right, req.idx);

        // Get receipts from map
        let left_data = receipt_map.get(&req.left)
            .with_context(|| format!("Left receipt {} not found in batch", req.left))?;
        let right_data = receipt_map.get(&req.right)
            .with_context(|| format!("Right receipt {} not found in batch", req.right))?;

        let left_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(left_data)
            .context("[BENTO-JOIN-BATCH-001] Failed to deserialize left receipt")?;
        let right_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(right_data)
            .context("[BENTO-JOIN-BATCH-002] Failed to deserialize right receipt")?;

        left_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-JOIN-BATCH-003] Failed to verify left receipt integrity")?;
        right_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-JOIN-BATCH-004] Failed to verify right receipt integrity")?;

        // Perform join
        let join_start = Instant::now();
        let joined = agent
            .prover
            .as_ref()
            .context("[BENTO-JOIN-BATCH-005] Missing prover from join task")?
            .join(&left_receipt, &right_receipt)?;
        let join_elapsed = join_start.elapsed().as_secs_f64();
        helpers::record_task_operation("join", "join_receipts", "success", join_elapsed);

        joined
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-JOIN-BATCH-006] Failed to verify join receipt integrity")?;

        let join_result = serialize_obj(&joined).expect("Failed to serialize joined receipt");
        join_results.push((req.idx.clone(), join_result));

        // Track keys to cleanup
        keys_to_cleanup.push(format!("{recur_receipts_prefix}:{}", req.left));
        keys_to_cleanup.push(format!("{recur_receipts_prefix}:{}", req.right));

        tracing::debug!("Completed join for {}", req.idx);
    }

    // Pipeline SET operations for all join results
    let pipeline_start = Instant::now();
    let mut pipe_set = pipe();
    for (idx, result) in &join_results {
        let output_key = format!("{recur_receipts_prefix}:{idx}");
        pipe_set.set_ex(&output_key, result, agent.args.redis_ttl);
    }
    pipe_set.query_async::<_, ()>(&mut *conn)
        .await
        .context("Failed to pipeline SET join results")?;
    helpers::record_redis_operation(
        "pipeline_set",
        "success",
        pipeline_start.elapsed().as_secs_f64(),
    );

    tracing::debug!("Stored {} join results in {:?}", batch_size, pipeline_start.elapsed());

    // UNLINK all input receipts at once
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&keys_to_cleanup).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("unlink", cleanup_status, cleanup_start.elapsed().as_secs_f64());
    cleanup_result.context("Failed to UNLINK join input keys")?;

    tracing::debug!("Cleaned up {} input receipts", keys_to_cleanup.len());

    // Record total batch operation
    helpers::record_task_operation(
        "join",
        "batch_complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    tracing::info!(
        "Batch join complete for job {job_id}: {batch_size} joins in {:?}",
        start_time.elapsed()
    );

    Ok(())
}

/// Run the join operation (delegates to batch_join for code reuse)
pub async fn join(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    // Generate a temporary task_id for the single join
    let task_id = request.idx.clone();

    // Just call the batch version with a single item
    batch_join(agent, job_id, &[(task_id, request.clone())]).await
}
