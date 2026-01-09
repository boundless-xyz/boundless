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
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{JoinReq, metrics::helpers};

/// Run a POVW join request
pub async fn join_povw(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    let start_time = Instant::now();
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Get the left and right receipts
    let left_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.left);
    let right_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.right);

    // Get receipts using Redis helper
    let (left_receipt_bytes, right_receipt_bytes): (Vec<u8>, Vec<u8>) = conn
        .mget::<_, (Vec<u8>, Vec<u8>)>(&[&left_receipt_key, &right_receipt_key])
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to get receipts for keys: {left_receipt_key}, {right_receipt_key}: {e}"
            )
        })?;

    // Deserialize POVW receipts
    let (left_receipt, right_receipt): (
        SuccinctReceipt<WorkClaim<ReceiptClaim>>,
        SuccinctReceipt<WorkClaim<ReceiptClaim>>,
    ) = (
        deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&left_receipt_bytes)?,
        deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&right_receipt_bytes)?,
    );

    left_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOINPOVW-001] Failed to verify left receipt integrity")?;
    right_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOINPOVW-002] Failed to verify right receipt integrity")?;

    tracing::debug!("Starting POVW join of receipts {} and {}", request.left, request.right);
    let join_povw_start = Instant::now();
    // Use POVW-specific join - this is required for POVW functionality
    let joined_receipt = match agent.prover.as_ref() {
        Some(prover) => match prover.join_povw(&left_receipt, &right_receipt) {
            Ok(receipt) => {
                helpers::record_task(
                    "join_povw",
                    "join_povw",
                    "success",
                    join_povw_start.elapsed().as_secs_f64(),
                );
                receipt
            }
            Err(e) => {
                helpers::record_task_operation("join_povw", "join_povw", "error", 0.0);
                return Err(e.context(
                        "POVW join method not available - POVW functionality requires RISC Zero POVW support",
                    ));
            }
        },
        None => return Err(anyhow::anyhow!("No prover available for join task")),
    };

    joined_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-JOINPOVW-003] Failed to verify joined POVW receipt integrity")?;

    tracing::debug!("Completed POVW join: {} and {}", request.left, request.right);

    // Store the joined POVW receipt (this is what finalization will need to unwrap)
    let povw_output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.idx);
    let povw_receipt_asset = serialize_obj(&joined_receipt)
        .context("[BENTO-JOINPOVW-004] Failed to serialize joined POVW receipt")?;

    // Store joined POVW receipt using Redis helper
    redis::set_key_with_expiry(
        &mut conn,
        &povw_output_key,
        povw_receipt_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to write joined POVW receipt to Redis: {e}"))?;

    // Clean up intermediate POVW receipts
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&[&left_receipt_key, &right_receipt_key]).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(
        "unlink",
        cleanup_status,
        cleanup_start.elapsed().as_secs_f64(),
    );
    cleanup_result.map_err(|e| anyhow::anyhow!("Failed to delete POVW join receipt keys: {e}"))?;

    // Record total task duration and success
    helpers::record_task_operation(
        "join_povw",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );
    Ok(())
}
