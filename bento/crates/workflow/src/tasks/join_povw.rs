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
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use uuid::Uuid;
use workflow_common::JoinReq;

/// Run a POVW join request
pub async fn join_povw(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Get the left and right receipts
    let left_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.left);
    let right_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.right);

    let (left_receipt_bytes, right_receipt_bytes): (Vec<u8>, Vec<u8>) = conn
        .mget::<_, (Vec<u8>, Vec<u8>)>(&[&left_receipt_key, &right_receipt_key])
        .await
        .with_context(|| {
            format!("failed to get receipts for keys: {left_receipt_key}, {right_receipt_key}")
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
        .context("Failed to verify left receipt integrity")?;
    right_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify right receipt integrity")?;

    tracing::debug!("Starting POVW join of receipts {} and {}", request.left, request.right);

    // Use POVW-specific join - this is required for POVW functionality
    let joined_receipt = if let Some(prover) = agent.prover.as_ref() {
        prover.join_povw(&left_receipt, &right_receipt).context(
            "POVW join method not available - POVW functionality requires RISC Zero POVW support",
        )?
    } else {
        return Err(anyhow::anyhow!("No prover available for join task"));
    };

    joined_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify joined POVW receipt integrity")?;

    tracing::debug!("Completed POVW join: {} and {}", request.left, request.right);

    // Store the joined POVW receipt (this is what finalization will need to unwrap)
    let povw_output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.idx);
    let povw_receipt_asset =
        serialize_obj(&joined_receipt).context("Failed to serialize joined POVW receipt")?;

    redis::set_key_with_expiry(
        &mut conn,
        &povw_output_key,
        povw_receipt_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .context("Failed to write joined POVW receipt to Redis")?;

    Ok(())
}
