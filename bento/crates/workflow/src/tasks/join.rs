// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{JoinReq, metrics::helpers};

/// Run the join operation
pub async fn join(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    let start_time = Instant::now();
    let mut conn = agent.redis_pool.get().await?;
    // Build the redis keys for the right and left joins
    let job_prefix = format!("job:{job_id}");
    let recur_receipts_prefix = format!("{job_prefix}:{RECUR_RECEIPT_PATH}");

    let left_path_key = format!("{recur_receipts_prefix}:{}", request.left);
    let right_path_key = format!("{recur_receipts_prefix}:{}", request.right);

    // Get receipts using Redis helper
    let (left_receipt, right_receipt): (Vec<u8>, Vec<u8>) = conn
        .mget::<_, (Vec<u8>, Vec<u8>)>(&[&left_path_key, &right_path_key])
        .await
        .map_err(|e| {
            anyhow::anyhow!(e).context(format!(
                "failed to get receipts for keys: {left_path_key}, {right_path_key}"
            ))
        })?;

    let left_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(&left_receipt)
        .context("[BENTO-JOIN-001] Failed to deserialize left receipt")?;
    let right_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(&right_receipt)
        .context("[BENTO-JOIN-002] Failed to deserialize right receipt")?;

    left_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOIN-003] Failed to verify left receipt integrity")?;
    right_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOIN-004] Failed to verify right receipt integrity")?;

    tracing::trace!("Joining {job_id} - {} + {} -> {}", request.left, request.right, request.idx);

    // Record join operation
    let join_start = Instant::now();
    let joined = match agent
        .prover
        .as_ref()
        .context("Missing prover from join task")?
        .join(&left_receipt, &right_receipt)
    {
        Ok(receipt) => {
            helpers::record_task_operation(
                "join",
                "join_receipts",
                "success",
                join_start.elapsed().as_secs_f64(),
            );
            receipt
        }
        Err(e) => {
            helpers::record_task(
                "join",
                "join_receipts",
                "error",
                join_start.elapsed().as_secs_f64(),
            );
            return Err(e);
        }
    };
    joined
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOIN-006] Failed to verify join receipt integrity")?;

    let join_result = serialize_obj(&joined).expect("Failed to serialize the segment");
    let output_key = format!("{recur_receipts_prefix}:{}", request.idx);

    // Store joined receipt using Redis helper
    redis::set_key_with_expiry(&mut conn, &output_key, join_result, Some(agent.args.redis_ttl))
        .await
        .map_err(|e| anyhow::anyhow!(e).context("Failed to store joined receipt"))?;

    tracing::debug!("Join Complete {job_id} - {}", request.left);

    // Clean up intermediate receipts
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&[&left_path_key, &right_path_key]).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(
        "unlink",
        cleanup_status,
        cleanup_start.elapsed().as_secs_f64(),
    );
    cleanup_result.map_err(|e| anyhow::anyhow!(e).context("Failed to delete join receipt keys"))?;

    // Record total task duration and success
    helpers::record_task_operation(
        "join",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(())
}
