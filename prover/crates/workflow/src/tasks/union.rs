// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    tasks::{CleanupKeys, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{KECCAK_RECEIPT_PATH, UnionReq, metrics::helpers};

/// Run the union operation
pub async fn union(agent: &Agent, job_id: &Uuid, request: &UnionReq) -> Result<CleanupKeys> {
    let start_time = Instant::now();
    tracing::debug!("Starting union for job_id: {job_id}");

    // setup redis keys
    let keccak_receipts_prefix = format!("job:{job_id}:{KECCAK_RECEIPT_PATH}");
    let left_receipt_key = format!("{keccak_receipts_prefix}:{0}", request.left);
    let right_receipt_key = format!("{keccak_receipts_prefix}:{0}", request.right);

    // get assets from redis
    let left_receipt_bytes = agent
        .hot_get_bytes(&left_receipt_key)
        .await
        .with_context(|| format!("failed to get receipt for key: {left_receipt_key}"))?;
    let right_receipt_bytes = agent
        .hot_get_bytes(&right_receipt_key)
        .await
        .with_context(|| format!("failed to get receipt for key: {right_receipt_key}"))?;

    let left_receipt = deserialize_obj(&left_receipt_bytes)
        .context("[BENTO-UNION-001] Failed to deserialize left receipt")?;
    let right_receipt = deserialize_obj(&right_receipt_bytes)
        .context("[BENTO-UNION-002] Failed to deserialize right receipt")?;

    // run union
    tracing::debug!("Union {job_id} - {} + {} -> {}", request.left, request.right, request.idx);

    let unioned = agent
        .prover
        .as_ref()
        .context("[BENTO-UNION-003] Missing prover from union prove task")?
        .union(&left_receipt, &right_receipt)
        .context("[BENTO-UNION-004] Failed to union on left/right receipt")?
        .into_unknown();

    unioned
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-UNION-005] Failed to verify union receipt integrity")?;

    // send result to redis
    let union_result =
        serialize_obj(&unioned).context("[BENTO-UNION-006] Failed to serialize union receipt")?;
    let output_key = format!("{keccak_receipts_prefix}:{}", request.idx);
    agent
        .hot_set_bytes(&output_key, union_result)
        .await
        .context("[BENTO-UNION-007] Failed to set hot-store key for union receipt")?;

    tracing::debug!("Union complete {job_id} - {}", request.left);

    // Record total task duration and success
    helpers::record_task_operation(
        "union",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(CleanupKeys(vec![left_receipt_key, right_receipt_key]))
}
