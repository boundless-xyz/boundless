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
use risc0_zkvm::{ReceiptClaim, SegmentReceipt, SuccinctReceipt};
use uuid::Uuid;
use workflow_common::JoinReq;

/// Run the join operation
pub async fn join(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    let mut conn = agent.redis_pool.get().await?;
    // Build the redis keys for the right and left joins
    let job_prefix = format!("job:{job_id}");
    let recur_receipts_prefix = format!("{job_prefix}:{RECUR_RECEIPT_PATH}");

    let left_path_key = format!("{recur_receipts_prefix}:{}", request.left);
    let right_path_key = format!("{recur_receipts_prefix}:{}", request.right);

    let (left_receipt_data, right_receipt_data): (Vec<u8>, Vec<u8>) =
        conn.mget::<_, (Vec<u8>, Vec<u8>)>(&[&left_path_key, &right_path_key]).await.with_context(
            || format!("failed to get receipts for keys: {left_path_key}, {right_path_key}"),
        )?;

    // Handle each receipt independently - they could be mixed types
    let prover = agent.prover.as_ref().context("Missing prover from resolve task")?;
    let left_idx = request.left;
    let right_idx = request.right;

    let (left_receipt, right_receipt) = tokio::try_join!(
        async {
            match deserialize_obj::<SegmentReceipt>(&left_receipt_data) {
                Ok(segment) => {
                    let receipt = prover
                        .lift(&segment)
                        .with_context(|| format!("Failed to lift segment {left_idx}"))?;
                    tracing::debug!("lifting complete {job_id} - {left_idx}");
                    Ok::<_, anyhow::Error>(receipt)
                }
                Err(_) => {
                    let receipt: SuccinctReceipt<ReceiptClaim> =
                        deserialize_obj(&left_receipt_data)
                            .context("Failed to deserialize left receipt")?;
                    Ok(receipt)
                }
            }
        },
        async {
            match deserialize_obj::<SegmentReceipt>(&right_receipt_data) {
                Ok(segment) => {
                    let receipt = prover
                        .lift(&segment)
                        .with_context(|| format!("Failed to lift segment {right_idx}"))?;
                    tracing::debug!("lifting complete {job_id} - {right_idx}");
                    Ok::<_, anyhow::Error>(receipt)
                }
                Err(_) => {
                    let receipt: SuccinctReceipt<ReceiptClaim> =
                        deserialize_obj(&right_receipt_data)
                            .context("Failed to deserialize right receipt")?;
                    Ok(receipt)
                }
            }
        }
    )?;

    tracing::trace!("Joining {job_id} - {} + {} -> {}", request.left, request.right, request.idx);

    let joined = agent
        .prover
        .as_ref()
        .context("Missing prover from join task")?
        .join(&left_receipt, &right_receipt)?;
    let join_result = serialize_obj(&joined).expect("Failed to serialize the segment");
    let output_key = format!("{recur_receipts_prefix}:{}", request.idx);
    redis::set_key_with_expiry(&mut conn, &output_key, join_result, Some(agent.args.redis_ttl))
        .await?;

    tracing::debug!("Join Complete {job_id} - {}", request.left);

    Ok(())
}
