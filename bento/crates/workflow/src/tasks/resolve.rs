// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{
        deserialize_obj, serialize_obj, KECCAK_RECEIPT_PATH, RECEIPT_PATH, RECUR_RECEIPT_PATH,
    },
    Agent,
};
use anyhow::{Context, Result};
use risc0_zkvm::{sha::Digestible, ReceiptClaim, SuccinctReceipt};
use uuid::Uuid;
use workflow_common::ResolveReq;

/// Run the resolve operation
pub async fn resolver(agent: &Agent, job_id: &Uuid, request: &ResolveReq) -> Result<Option<u64>> {
    let max_idx = &request.max_idx;
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{max_idx}");

    let mut conn = redis::get_connection(&agent.redis_pool).await?;
    let join_receipt_bytes: Vec<u8> =
        conn.get::<_, Vec<u8>>(&root_receipt_key).await.with_context(|| {
            format!("segment data not found for root receipt key: {root_receipt_key}")
        })?;

    let mut conditional_receipt: SuccinctReceipt<ReceiptClaim> =
        deserialize_obj(&join_receipt_bytes).with_context(|| {
            format!("Failed to deserialize root receipt for job: {job_id}, max_idx: {max_idx}")
        })?;

    let mut assumptions_len: Option<u64> = None;

    if let Some(guest_output) = conditional_receipt
        .claim
        .clone()
        .as_value()
        .context("Failed unwrap the claim of the conditional receipt")?
        .output
        .as_value()
        .context("Failed unwrap the output of the conditional receipt")?
    {
        if !guest_output.assumptions.is_empty() {
            let assumptions = guest_output
                .assumptions
                .as_value()
                .context("Failed unwrap the assumptions of the conditional receipt")?
                .iter();
            tracing::info!("Resolving {} assumption(s)", assumptions.len());
            assumptions_len = Some(assumptions.len().try_into()?);
            let mut union_claim = String::new();
            if let Some(idx) = request.union_max_idx {
                let union_root_receipt_key = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}:{idx}");
                tracing::info!("Deserializing union_receipt_key: {union_root_receipt_key}");

                let union_receipt_bytes: Vec<u8> =
                    conn.get::<_, Vec<u8>>(&union_root_receipt_key).await.with_context(|| {
                        format!(
                            "segment data not found for root receipt key: {union_root_receipt_key}"
                        )
                    })?;

                let union_receipt: SuccinctReceipt<risc0_zkvm::Unknown> =
                    deserialize_obj(&union_receipt_bytes).with_context(|| {
                        format!(
                        "Failed to deserialize root receipt for job: {job_id}, max_idx: {max_idx}"
                    )
                    })?;

                union_claim = union_receipt.claim.digest().to_string();

                // Try to resolve the union claim
                tracing::info!("Resolving union claim: {union_claim}");
                conditional_receipt = agent
                    .prover
                    .as_ref()
                    .context("Missing prover from resolve task")?
                    .resolve(&conditional_receipt, &union_receipt)?;

                tracing::info!("Union claim resolved");
            }

            for assumption in assumptions {
                let assumption_claim = assumption.as_value()?.claim.to_string();
                if assumption_claim.eq(&union_claim) {
                    tracing::info!("Skipping already resolved union claim: {union_claim}");
                    continue;
                }
                let assumption_key = format!("{receipts_key}:{assumption_claim}");
                tracing::info!("Deserializing assumption with key: {assumption_key}");
                let assumption_bytes: Vec<u8> = conn
                    .get(&assumption_key)
                    .await
                    .context("corroborating receipt not found: key {assumption_key}")?;

                let assumption_receipt: SuccinctReceipt<risc0_zkvm::Unknown> =
                    deserialize_obj(&assumption_bytes).with_context(|| {
                        format!("could not deserialize assumption receipt: {assumption_key}")
                    })?;

                // Resolve
                conditional_receipt = agent
                    .prover
                    .as_ref()
                    .context("Missing prover from resolve task")?
                    .resolve(&conditional_receipt, &assumption_receipt)
                    .context("Failed to resolve the conditional receipt")?;
            }
            tracing::info!("Resolve complete");
        }
    }

    // Write out the resolved receipt
    let serialized_asset =
        serialize_obj(&conditional_receipt).expect("Failed to serialize the asset");

    redis::set_key_with_expiry(
        &mut conn,
        &root_receipt_key,
        serialized_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .expect("Failed to set root receipt key with expiry");

    Ok(assumptions_len)
}
