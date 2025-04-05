// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECEIPT_PATH, RECUR_RECEIPT_PATH},
    Agent,
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SegmentReceipt, SuccinctReceipt, Unknown};
use uuid::Uuid;
use workflow_common::ResolveReq;

/// Run the resolve operation
pub async fn resolver(agent: &Agent, job_id: &Uuid, request: &ResolveReq) -> Result<Option<u64>> {
    tracing::info!("Starting resolve for job_id: {job_id}");

    let max_idx = &request.max_idx;
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{max_idx}");

    let mut conn = redis::get_connection(&agent.redis_pool).await?;
    let receipt: Vec<u8> = conn.get::<_, Vec<u8>>(&root_receipt_key).await.with_context(|| {
        format!("segment data not found for root receipt key: {root_receipt_key}")
    })?;

    // Deserialize receipt, handling both direct SuccinctReceipt and SegmentReceipt that needs lifting
    let mut conditional_receipt: SuccinctReceipt<ReceiptClaim> =
        if let Ok(res) = deserialize_obj::<SuccinctReceipt<ReceiptClaim>>(&receipt) {
            res
        } else {
            tracing::info!("Deserializing receipt to SegmentReceipt");
            let segment_receipt: SegmentReceipt = deserialize_obj(&receipt)
                .context("could not deserialize the root receipt to SegmentReceipt")?;

            tracing::info!("Lifting SegmentReceipt");
            agent
                .prover
                .as_ref()
                .context("Missing prover from resolve task")?
                .lift(&segment_receipt)
                .context("Failed to lift SegmentReceipt")?
        };

    // Process guest output and assumptions
    let mut assumptions_len: Option<u64> = None;
    if let Ok(claim_value) = conditional_receipt.claim.clone().as_value() {
        if claim_value.output.is_some() {
            if let Some(guest_output) = claim_value.output.as_value()? {
                if !guest_output.assumptions.is_empty() {
                    let assumptions = match guest_output.assumptions.as_value() {
                        Ok(assumptions) => assumptions.iter().collect::<Vec<_>>(),
                        Err(e) => {
                            return Err(anyhow::anyhow!(
                                "Failed to unwrap the assumptions of the guest output: {e:?}"
                            ))
                        }
                    };

                    tracing::info!("Resolving {} assumption(s)", assumptions.len());
                    assumptions_len = Some(assumptions.len().try_into()?);

                    // Create a pool for Redis connections
                    let pool = agent.redis_pool.clone();

                    // Handle union receipt if present
                    if let Some(idx) = request.union_max_idx {
                        let union_receipt_key = format!("{job_prefix}:{RECEIPT_PATH}:{idx}");
                        tracing::info!("Processing union receipt key: {union_receipt_key}");

                        let union_receipt_bytes = conn
                            .get::<_, Vec<u8>>(&union_receipt_key)
                            .await
                            .context("Failed to get union receipt from Redis")?;

                        let union_receipt: SuccinctReceipt<Unknown> = deserialize_obj(
                            &union_receipt_bytes,
                        )
                        .context("Failed to deserialize to SuccinctReceipt<Unknown> type")?;

                        let union_claim = format!("{:?}", union_receipt.claim);
                        tracing::info!("Resolving union claim: {union_claim}");

                        // Resolve union receipt first
                        conditional_receipt = agent
                            .prover
                            .as_ref()
                            .context("Missing prover from resolve task")?
                            .resolve(&conditional_receipt, &union_receipt)
                            .context("Failed to resolve the union receipt")?;

                        // Skip this assumption if we process it later in the loop
                        let skip_claim = union_claim;

                        // Process remaining assumptions
                        for assumption in assumptions {
                            let assumption_claim = format!("{:?}", assumption.as_value()?.claim);

                            // Skip if already processed as union
                            if assumption_claim == skip_claim {
                                tracing::info!(
                                    "Skipping already processed union claim: {assumption_claim}"
                                );
                                continue;
                            }

                            tracing::info!("Processing assumption: {assumption_claim}");
                            let assumption_key = format!("{receipts_key}:{assumption_claim}");

                            // Fetch assumption receipt
                            let mut task_conn = redis::get_connection(&pool).await?;
                            let bytes =
                                task_conn.get::<_, Vec<u8>>(&assumption_key).await.with_context(
                                    || format!("Receipt not found for key: {assumption_key}"),
                                )?;

                            // Deserialize the assumption receipt
                            let assumption_receipt: SuccinctReceipt<Unknown> =
                                deserialize_obj(&bytes).with_context(|| {
                                    format!(
                                        "Failed to deserialize receipt for key: {assumption_key}"
                                    )
                                })?;

                            // Resolve this assumption
                            conditional_receipt = agent
                                .prover
                                .as_ref()
                                .context("Missing prover from resolve task")?
                                .resolve(&conditional_receipt, &assumption_receipt)
                                .context("Failed to resolve assumption")?;
                        }
                    } else {
                        // Process all assumptions without union optimization
                        for assumption in assumptions {
                            let assumption_claim = format!("{:?}", assumption.as_value()?.claim);
                            tracing::info!("Processing assumption: {assumption_claim}");
                            let assumption_key = format!("{receipts_key}:{assumption_claim}");

                            // Fetch assumption receipt
                            let mut task_conn = redis::get_connection(&pool).await?;
                            let bytes =
                                task_conn.get::<_, Vec<u8>>(&assumption_key).await.with_context(
                                    || format!("Receipt not found for key: {assumption_key}"),
                                )?;

                            // Deserialize the assumption receipt
                            let assumption_receipt: SuccinctReceipt<Unknown> =
                                deserialize_obj(&bytes).with_context(|| {
                                    format!(
                                        "Failed to deserialize receipt for key: {assumption_key}"
                                    )
                                })?;

                            // Resolve this assumption
                            conditional_receipt = agent
                                .prover
                                .as_ref()
                                .context("Missing prover from resolve task")?
                                .resolve(&conditional_receipt, &assumption_receipt)
                                .context("Failed to resolve assumption")?;
                        }
                    }

                    tracing::info!("Resolve complete");
                }
            }
        }
    }

    // Write out the resolved receipt
    tracing::info!("Serializing resolved receipt");
    let serialized_asset =
        serialize_obj(&conditional_receipt).context("Failed to serialize the asset")?;

    tracing::info!("Writing resolved receipt to Redis key: {root_receipt_key}");
    redis::set_key_with_expiry(
        &mut conn,
        &root_receipt_key,
        serialized_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .context("Failed to set root receipt key with expiry")?;

    tracing::info!("Resolve operation completed successfully");
    Ok(assumptions_len)
}
