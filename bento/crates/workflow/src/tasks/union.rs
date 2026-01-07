// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::AsyncCommands,
    tasks::{deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use deadpool_redis::redis::pipe;
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{KECCAK_RECEIPT_PATH, UnionReq, metrics::helpers};

/// Batch union multiple receipt pairs with optimized Redis operations
pub async fn batch_union(
    agent: &Agent,
    job_id: &Uuid,
    requests: &[(String, UnionReq)], // (task_id, request) pairs
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let start_time = Instant::now();
    let batch_size = requests.len();
    tracing::debug!("Starting batch union for job {job_id} with {batch_size} unions");

    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let keccak_receipts_prefix = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}");

    // Collect all unique input receipt keys needed across all unions
    let mut unique_keys = HashSet::new();
    for (_, req) in requests {
        unique_keys.insert(req.left);
        unique_keys.insert(req.right);
    }

    // Convert to Vec to maintain consistent ordering
    let unique_keys_vec: Vec<usize> = unique_keys.into_iter().collect();
    let keys: Vec<String> = unique_keys_vec
        .iter()
        .map(|k| format!("{keccak_receipts_prefix}:{k}"))
        .collect();

    // MGET all unique input receipts at once
    let mget_start = Instant::now();
    let receipts_data: Vec<Vec<u8>> = conn
        .mget(&keys)
        .await
        .context("Failed to MGET union input receipts")?;
    helpers::record_redis_operation("mget", "success", mget_start.elapsed().as_secs_f64());

    tracing::debug!(
        "Fetched {} unique receipts for {batch_size} unions in {:?}",
        keys.len(),
        mget_start.elapsed()
    );

    // Build a map of receipt_id -> receipt data
    let mut receipt_map: HashMap<usize, Vec<u8>> = HashMap::new();
    for (i, key) in unique_keys_vec.iter().enumerate() {
        receipt_map.insert(*key, receipts_data[i].clone());
    }

    // Process each union sequentially
    let mut union_results = Vec::with_capacity(batch_size);
    let mut keys_to_cleanup = Vec::new();

    for (_task_id, req) in requests {
        tracing::debug!("Union {} + {} -> {}", req.left, req.right, req.idx);

        // Get receipts from map
        let left_data = receipt_map
            .get(&req.left)
            .with_context(|| format!("Left receipt {} not found in batch", req.left))?;
        let right_data = receipt_map
            .get(&req.right)
            .with_context(|| format!("Right receipt {} not found in batch", req.right))?;

        let left_receipt = deserialize_obj(left_data)
            .context("[BENTO-UNION-BATCH-001] Failed to deserialize left receipt")?;
        let right_receipt = deserialize_obj(right_data)
            .context("[BENTO-UNION-BATCH-002] Failed to deserialize right receipt")?;

        // Perform union
        let union_start = Instant::now();
        let unioned = agent
            .prover
            .as_ref()
            .context("[BENTO-UNION-BATCH-003] Missing prover from union task")?
            .union(&left_receipt, &right_receipt)
            .context("[BENTO-UNION-BATCH-004] Failed to union on left/right receipt")?
            .into_unknown();
        let union_elapsed = union_start.elapsed().as_secs_f64();
        helpers::record_task_operation("union", "union_receipts", "success", union_elapsed);

        unioned
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-UNION-BATCH-005] Failed to verify union receipt integrity")?;

        let union_result = serialize_obj(&unioned)
            .context("[BENTO-UNION-BATCH-006] Failed to serialize union receipt")?;
        union_results.push((req.idx.clone(), union_result));

        // Track keys to cleanup
        keys_to_cleanup.push(format!("{keccak_receipts_prefix}:{}", req.left));
        keys_to_cleanup.push(format!("{keccak_receipts_prefix}:{}", req.right));

        tracing::debug!("Completed union for {}", req.idx);
    }

    // Pipeline SET operations for all union results
    let pipeline_start = Instant::now();
    let mut pipe_set = pipe();
    for (idx, result) in &union_results {
        let output_key = format!("{keccak_receipts_prefix}:{idx}");
        pipe_set.set_ex(&output_key, result, agent.args.redis_ttl);
    }
    pipe_set
        .query_async::<_, ()>(&mut *conn)
        .await
        .context("Failed to pipeline SET union results")?;
    helpers::record_redis_operation(
        "pipeline_set",
        "success",
        pipeline_start.elapsed().as_secs_f64(),
    );

    tracing::debug!(
        "Stored {} union results in {:?}",
        batch_size,
        pipeline_start.elapsed()
    );

    // UNLINK all input receipts at once
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&keys_to_cleanup).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation("unlink", cleanup_status, cleanup_start.elapsed().as_secs_f64());
    cleanup_result.context("Failed to UNLINK union input keys")?;

    tracing::debug!("Cleaned up {} input receipts", keys_to_cleanup.len());

    // Record total batch operation
    helpers::record_task_operation(
        "union",
        "batch_complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    tracing::info!(
        "Batch union complete for job {job_id}: {batch_size} unions in {:?}",
        start_time.elapsed()
    );

    Ok(())
}

/// Run the union operation (delegates to batch_union for code reuse)
pub async fn union(agent: &Agent, job_id: &Uuid, request: &UnionReq) -> Result<()> {
    // Generate a temporary task_id for the single union
    let task_id = request.idx.to_string();

    // Just call the batch version with a single item
    batch_union(agent, job_id, &[(task_id, request.clone())]).await
}
