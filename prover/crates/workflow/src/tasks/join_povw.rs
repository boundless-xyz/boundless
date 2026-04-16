// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    tasks::{CleanupKeys, RECUR_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{JoinReq, metrics::helpers};

/// Run a POVW join request
pub async fn join_povw(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<CleanupKeys> {
    let start_time = Instant::now();
    let job_prefix = format!("job:{job_id}");

    // Get the left and right receipts
    let left_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.left);
    let right_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.right);

    // Get receipts using Redis helper
    let left_receipt_bytes = agent
        .hot_get_bytes(&left_receipt_key)
        .await
        .with_context(|| format!("failed to get receipt for key: {left_receipt_key}"))?;
    let right_receipt_bytes = agent
        .hot_get_bytes(&right_receipt_key)
        .await
        .with_context(|| format!("failed to get receipt for key: {right_receipt_key}"))?;

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
    agent
        .hot_set_bytes(&povw_output_key, povw_receipt_asset)
        .await
        .context("Failed to write joined POVW receipt to hot store")?;

    // Record total task duration and success
    helpers::record_task_operation(
        "join_povw",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );
    Ok(CleanupKeys(vec![left_receipt_key, right_receipt_key]))
}
