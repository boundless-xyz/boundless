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
use risc0_zkvm::{ReceiptClaim, SegmentReceipt, SuccinctReceipt, WorkClaim};
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

    // Handle each receipt independently - they could be mixed types
    let prover = agent.prover.as_ref().context("Missing prover from POVW join task")?;

    let (left_receipt, right_receipt): (
        SuccinctReceipt<WorkClaim<ReceiptClaim>>,
        SuccinctReceipt<WorkClaim<ReceiptClaim>>,
    ) = tokio::try_join!(
        async {
            match deserialize_obj::<SegmentReceipt>(&left_receipt_bytes) {
                Ok(segment_receipt) => {
                    // Successfully deserialized as segment receipt, now lift to POVW
                    let povw_receipt = prover
                        .lift_povw(&segment_receipt)
                        .context("Failed to lift left segment to POVW")?;
                    Ok::<_, anyhow::Error>(povw_receipt)
                }
                Err(_) => {
                    // Failed to deserialize as segment, try as already-lifted POVW receipt
                    let povw_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> =
                        deserialize_obj(&left_receipt_bytes)
                            .context("Failed to deserialize left POVW receipt")?;
                    Ok(povw_receipt)
                }
            }
        },
        async {
            match deserialize_obj::<SegmentReceipt>(&right_receipt_bytes) {
                Ok(segment_receipt) => {
                    // Successfully deserialized as segment receipt, now lift to POVW
                    let povw_receipt = prover
                        .lift_povw(&segment_receipt)
                        .context("Failed to lift right segment to POVW")?;
                    Ok::<_, anyhow::Error>(povw_receipt)
                }
                Err(_) => {
                    // Failed to deserialize as segment, try as already-lifted POVW receipt
                    let povw_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> =
                        deserialize_obj(&right_receipt_bytes)
                            .context("Failed to deserialize right POVW receipt")?;
                    Ok(povw_receipt)
                }
            }
        }
    )?;

    tracing::debug!("Starting POVW join of receipts {} and {}", request.left, request.right);

    // Use POVW-specific join - this is required for POVW functionality
    let joined_receipt = if let Some(prover) = agent.prover.as_ref() {
        prover.join_povw(&left_receipt, &right_receipt).context(
            "POVW join method not available - POVW functionality requires RISC Zero POVW support",
        )?
    } else {
        return Err(anyhow::anyhow!("No prover available for join task"));
    };

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
