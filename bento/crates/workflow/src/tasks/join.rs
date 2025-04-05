// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECUR_RECEIPT_PATH},
    Agent,
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SegmentReceipt, SuccinctReceipt};
use std::rc::Rc;
use uuid::Uuid;
use workflow_common::JoinReq;

/// Run the join operation
pub async fn join(agent: &Agent, job_id: &Uuid, request: &JoinReq) -> Result<()> {
    // Build the redis keys for the right and left joins
    let job_prefix = format!("job:{job_id}");
    let recur_receipts_prefix = format!("{job_prefix}:{RECUR_RECEIPT_PATH}");

    let left_path_key = format!("{recur_receipts_prefix}:{}", request.left);
    let right_path_key = format!("{recur_receipts_prefix}:{}", request.right);
    let output_key = format!("{recur_receipts_prefix}:{}", request.idx);

    // Get the prover for lifting operations
    let prover = agent.prover.as_ref().context("Missing prover from join task")?;
    let prover_clone = Rc::clone(prover);

    // Get Redis pool for connections
    let pool = agent.redis_pool.clone();

    tracing::info!(
        "Processing join {job_id} - {} + {} -> {}",
        request.left,
        request.right,
        request.idx
    );

    // Process both receipts in parallel, using try_join! to stop on first error
    let (left_receipt, right_receipt) = tokio::try_join!(
        async {
            // Get left receipt and process it
            let mut conn = redis::get_connection(&pool).await?;
            let bytes = conn.get::<_, Vec<u8>>(&left_path_key).await?;

            // Process left receipt - lift if needed
            if let Ok(segment) = deserialize_obj::<SegmentReceipt>(&bytes) {
                tracing::info!("Lifting left receipt {job_id} - {}", request.left);
                let lifted = prover
                    .lift(&segment)
                    .with_context(|| format!("Failed to lift left segment {}", request.left))?;
                tracing::info!("Left receipt lifted {job_id} - {}", request.left);
                Ok(lifted)
            } else {
                // Try to deserialize as already-lifted SuccinctReceipt
                deserialize_obj::<SuccinctReceipt<ReceiptClaim>>(&bytes)
                    .context("Failed to deserialize left receipt")
            }
        },
        async {
            // Get right receipt and process it
            let mut conn = redis::get_connection(&pool).await?;
            let bytes = conn.get::<_, Vec<u8>>(&right_path_key).await?;

            // Process right receipt - lift if needed
            if let Ok(segment) = deserialize_obj::<SegmentReceipt>(&bytes) {
                tracing::info!("Lifting right receipt {job_id} - {}", request.right);
                let lifted = prover_clone
                    .lift(&segment)
                    .with_context(|| format!("Failed to lift right segment {}", request.right))?;
                tracing::info!("Right receipt lifted {job_id} - {}", request.right);
                Ok(lifted)
            } else {
                // Try to deserialize as already-lifted SuccinctReceipt
                deserialize_obj::<SuccinctReceipt<ReceiptClaim>>(&bytes)
                    .context("Failed to deserialize right receipt")
            }
        },
    )?;

    // Join the receipts
    tracing::info!(
        "Joining receipts {job_id} - {} + {} -> {}",
        request.left,
        request.right,
        request.idx
    );
    let joined = agent
        .prover
        .as_ref()
        .context("Missing prover from join task")?
        .join(&left_receipt, &right_receipt)?;

    // Serialize and store in separate future to return quickly
    let job_id_clone = *job_id;
    let left_idx = request.left;
    let ttl = agent.args.redis_ttl;

    // Use spawn_local if available to reduce thread creation overhead
    tokio::task::spawn(async move {
        // Serialize result
        let join_result = match serialize_obj(&joined) {
            Ok(result) => result,
            Err(e) => {
                tracing::error!("Failed to serialize joined receipt: {}", e);
                return;
            }
        };

        // Store result
        let mut conn = match redis::get_connection(&pool).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to get Redis connection for storage: {}", e);
                return;
            }
        };

        if let Err(e) =
            redis::set_key_with_expiry(&mut conn, &output_key, join_result, Some(ttl)).await
        {
            tracing::error!("Failed to store joined receipt: {}", e);
            return;
        }

        tracing::info!("Join result stored for {job_id_clone} - {left_idx}");
    });

    tracing::info!("Join computation complete {job_id} - {}", request.left);
    Ok(())
}
