// Copyright 2025 RISC Zero, Inc.
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
use workflow_common::{
    JoinReq,
    metrics::{JOIN_DURATION, TASK_DURATION, helpers},
};

/// Run the join operation
pub async fn join(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    let start_time = Instant::now();
    let mut conn = agent.redis_pool.get().await?;
    // Build the redis keys for the right and left joins
    let job_prefix = format!("job:{job_id}");
    let recur_receipts_prefix = format!("{job_prefix}:{RECUR_RECEIPT_PATH}");

    let left_path_key = format!("{recur_receipts_prefix}:{}", request.left);
    let right_path_key = format!("{recur_receipts_prefix}:{}", request.right);

    // Record Redis operation for receipt retrieval
    let redis_start = Instant::now();
    let (left_receipt, right_receipt): (Vec<u8>, Vec<u8>) = match conn
        .mget::<_, (Vec<u8>, Vec<u8>)>(&[&left_path_key, &right_path_key])
        .await
    {
        Ok(data) => {
            helpers::record_redis_operation("mget", "success", redis_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_redis_operation("mget", "error", redis_start.elapsed().as_secs_f64());
            return Err(anyhow::anyhow!(e).context(format!(
                "failed to get receipts for keys: {left_path_key}, {right_path_key}"
            )));
        }
    };

    let left_receipt: SuccinctReceipt<ReceiptClaim> =
        deserialize_obj(&left_receipt).context("Failed to deserialize left receipt")?;
    let right_receipt: SuccinctReceipt<ReceiptClaim> =
        deserialize_obj(&right_receipt).context("Failed to deserialize right receipt")?;

    left_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify left receipt integrity")?;
    right_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify right receipt integrity")?;

    tracing::trace!("Joining {job_id} - {} + {} -> {}", request.left, request.right, request.idx);

    // Record join operation
    let joined = match agent
        .prover
        .as_ref()
        .context("Missing prover from join task")?
        .join(&left_receipt, &right_receipt)
    {
        Ok(receipt) => {
            helpers::record_task_operation("join", "join_receipts", "success");
            receipt
        }
        Err(e) => {
            helpers::record_task_operation("join", "join_receipts", "error");
            return Err(e);
        }
    };
    joined
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify join receipt integrity")?;

    let join_result = serialize_obj(&joined).expect("Failed to serialize the segment");
    let output_key = format!("{recur_receipts_prefix}:{}", request.idx);

    // Record Redis write operation
    let redis_write_start = Instant::now();
    match redis::set_key_with_expiry(
        &mut conn,
        &output_key,
        join_result,
        Some(agent.args.redis_ttl),
    )
    .await
    {
        Ok(()) => {
            helpers::record_redis_operation(
                "set_key_with_expiry",
                "success",
                redis_write_start.elapsed().as_secs_f64(),
            );
        }
        Err(e) => {
            helpers::record_redis_operation(
                "set_key_with_expiry",
                "error",
                redis_write_start.elapsed().as_secs_f64(),
            );
            return Err(e.into());
        }
    }

    tracing::debug!("Join Complete {job_id} - {}", request.left);

    // Record cleanup operation
    let cleanup_start = Instant::now();
    match conn.unlink::<_, ()>(&[&left_path_key, &right_path_key]).await {
        Ok(()) => {
            helpers::record_redis_operation(
                "unlink",
                "success",
                cleanup_start.elapsed().as_secs_f64(),
            );
        }
        Err(e) => {
            helpers::record_redis_operation(
                "unlink",
                "error",
                cleanup_start.elapsed().as_secs_f64(),
            );
            return Err(anyhow::anyhow!(e).context("Failed to delete join receipt keys"));
        }
    }

    // Record total task duration and success
    TASK_DURATION.observe(start_time.elapsed().as_secs_f64());
    JOIN_DURATION.observe(start_time.elapsed().as_secs_f64());
    helpers::record_task_operation("join", "complete", "success");

    Ok(())
}
