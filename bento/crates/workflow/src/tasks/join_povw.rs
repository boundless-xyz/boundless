// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code as governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECUR_RECEIPT_PATH},
    Agent,
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use uuid::Uuid;
use workflow_common::JoinPovwReq;

/// Run a POVW join request
pub async fn join_povw(agent: &Agent, job_id: &Uuid, request: &JoinPovwReq) -> Result<()> {
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Get the left and right receipts
    let left_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.left);
    let right_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.right);

    let left_receipt_bytes: Vec<u8> = conn
        .get::<_, Vec<u8>>(&left_receipt_key)
        .await
        .with_context(|| format!("left receipt not found for key: {left_receipt_key}"))?;

    let right_receipt_bytes: Vec<u8> = conn
        .get::<_, Vec<u8>>(&right_receipt_key)
        .await
        .with_context(|| format!("right receipt not found for key: {right_receipt_key}"))?;

    // Try to deserialize as POVW receipts first, then fall back to regular receipts
    let (left_receipt, right_receipt): (SuccinctReceipt<WorkClaim<ReceiptClaim>>, SuccinctReceipt<WorkClaim<ReceiptClaim>>) = {
        // Try POVW receipts first
        if let (Ok(left), Ok(right)) = (
            deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&left_receipt_bytes),
            deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&right_receipt_bytes)
        ) {
            (left, right)
        } else {
            // If we can't deserialize as POVW receipts, we need to handle this case
            // For now, we'll require that POVW receipts are properly formatted
            // This ensures the workflow is consistent
            return Err(anyhow::anyhow!(
                "POVW join requires POVW-formatted receipts. Left receipt {} and right receipt {} are not in POVW format.",
                request.left, request.right
            ));
        }
    };

    tracing::debug!("Starting POVW join of receipts {} and {}", request.left, request.right);

    // Use POVW-specific join - this is required for POVW functionality
    let joined_receipt = if let Some(prover) = agent.prover.as_ref() {
        prover.join_povw(&left_receipt, &right_receipt)
            .context("POVW join method not available - POVW functionality requires RISC Zero POVW support")?
    } else {
        return Err(anyhow::anyhow!("No prover available for join task"));
    };

    tracing::debug!("Completed POVW join: {} and {}", request.left, request.right);

    // Store the joined POVW receipt (this is what finalization will need to unwrap)
    let povw_output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.idx);
    let povw_receipt_asset =
        serialize_obj(&joined_receipt).context("Failed to serialize joined POVW receipt")?;

    redis::set_key_with_expiry(&mut conn, &povw_output_key, povw_receipt_asset, Some(agent.args.redis_ttl))
        .await
        .context("Failed to write joined POVW receipt to Redis")?;

    // Unwrap the POVW receipt to get the final receipt for immediate use
    let unwrapped_receipt = if let Some(prover) = agent.prover.as_ref() {
        prover.unwrap_povw(&joined_receipt)
            .context("POVW unwrap method not available - POVW functionality requires RISC Zero POVW support")?
    } else {
        return Err(anyhow::anyhow!("No prover available for unwrap task"));
    };

    tracing::debug!("Completed POVW unwrap for join: {}", request.idx);

    // Store the unwrapped receipt as well (for backward compatibility)
    let unwrapped_output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}_unwrapped", request.idx);
    let unwrapped_receipt_asset =
        serialize_obj(&unwrapped_receipt).context("Failed to serialize unwrapped receipt")?;

    redis::set_key_with_expiry(&mut conn, &unwrapped_output_key, unwrapped_receipt_asset, Some(agent.args.redis_ttl))
        .await
        .context("Failed to write unwrapped receipt to Redis")?;

    Ok(())
}
