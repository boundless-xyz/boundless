// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECEIPT_PATH, RECUR_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, Unknown};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{
    KECCAK_RECEIPT_PATH, ResolveReq,
    metrics::{RESOLVE_DURATION, TASK_DURATION, TASK_OPERATIONS, helpers},
};

/// Run the resolve operation
pub async fn resolver(agent: &Agent, job_id: &Uuid, request: &ResolveReq) -> Result<Option<u64>> {
    let start_time = Instant::now();
    let max_idx = &request.max_idx;
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{max_idx}");

    tracing::debug!("Starting resolve for job_id: {job_id}, max_idx: {max_idx}");

    let mut conn = agent.redis_pool.get().await?;
    let redis_start = Instant::now();
    let receipt: Vec<u8> = match conn.get::<_, Vec<u8>>(&root_receipt_key).await {
        Ok(data) => {
            helpers::record_redis_operation("get", "success", redis_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_redis_operation("get", "error", redis_start.elapsed().as_secs_f64());
            return Err(anyhow::anyhow!(e).context(format!(
                "segment data not found for root receipt key: {root_receipt_key}"
            )));
        }
    };

    tracing::debug!("Root receipt size: {} bytes", receipt.len());
    let mut conditional_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(&receipt)?;

    let mut assumptions_len: Option<u64> = None;
    if conditional_receipt.claim.clone().as_value()?.output.is_some() {
        if let Some(guest_output) =
            conditional_receipt.claim.clone().as_value()?.output.as_value()?
        {
            if !guest_output.assumptions.is_empty() {
                let assumptions = guest_output
                    .assumptions
                    .as_value()
                    .context("Failed unwrap the assumptions of the guest output")?
                    .iter();

                tracing::debug!("Resolving {} assumption(s)", assumptions.len());
                assumptions_len =
                    Some(assumptions.len().try_into().context("Failed to convert to u64")?);

                let mut union_claim = String::new();
                if let Some(idx) = request.union_max_idx {
                    let union_root_receipt_key =
                        format!("{job_prefix}:{KECCAK_RECEIPT_PATH}:{idx}");
                    tracing::debug!(
                        "Deserializing union_root_receipt_key: {union_root_receipt_key}"
                    );
                    let union_receipt: Vec<u8> = conn.get(&union_root_receipt_key).await?;
                    let union_receipt: SuccinctReceipt<Unknown> =
                        deserialize_obj(&union_receipt)
                            .context("Failed to deserialize to SuccinctReceipt<Unknown> type")?;
                    union_claim = union_receipt.claim.digest().to_string();

                    // Resolve union receipt
                    tracing::debug!("Resolving union claim digest: {union_claim}");
                    conditional_receipt = match agent
                        .prover
                        .as_ref()
                        .context("Missing prover from resolve task")?
                        .resolve(&conditional_receipt, &union_receipt)
                    {
                        Ok(receipt) => {
                            TASK_OPERATIONS
                                .with_label_values(&["resolve", "resolve_union", "success"])
                                .inc();
                            receipt
                        }
                        Err(e) => {
                            TASK_OPERATIONS
                                .with_label_values(&["resolve", "resolve_union", "error"])
                                .inc();
                            return Err(e.context("Failed to resolve the union receipt"));
                        }
                    };
                }

                for assumption in assumptions {
                    let assumption_claim = assumption.as_value()?.claim.to_string();
                    if assumption_claim.eq(&union_claim) {
                        tracing::debug!("Skipping already resolved union claim: {union_claim}");
                        continue;
                    }
                    let assumption_key = format!("{receipts_key}:{assumption_claim}");
                    tracing::debug!("Deserializing assumption with key: {assumption_key}");
                    let assumption_bytes: Vec<u8> = conn
                        .get(&assumption_key)
                        .await
                        .context("corroborating receipt not found: key {assumption_key}")?;

                    let assumption_receipt: SuccinctReceipt<Unknown> =
                        deserialize_obj(&assumption_bytes).with_context(|| {
                            format!("could not deserialize assumption receipt: {assumption_key}")
                        })?;

                    // Resolve
                    conditional_receipt = match agent
                        .prover
                        .as_ref()
                        .context("Missing prover from resolve task")?
                        .resolve(&conditional_receipt, &assumption_receipt)
                    {
                        Ok(receipt) => {
                            TASK_OPERATIONS
                                .with_label_values(&["resolve", "resolve_assumption", "success"])
                                .inc();
                            receipt
                        }
                        Err(e) => {
                            TASK_OPERATIONS
                                .with_label_values(&["resolve", "resolve_assumption", "error"])
                                .inc();
                            return Err(e.context("Failed to resolve the conditional receipt"));
                        }
                    };
                }
                tracing::debug!("Resolve complete for job_id: {job_id}");
            }
        }
    }

    // Write out the resolved receipt
    tracing::debug!("Serializing resolved receipt");
    let serialized_asset =
        serialize_obj(&conditional_receipt).context("Failed to serialize resolved receipt")?;

    tracing::debug!("Writing resolved receipt to Redis key: {root_receipt_key}");
    let redis_write_start = Instant::now();
    match redis::set_key_with_expiry(
        &mut conn,
        &root_receipt_key,
        serialized_asset,
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
            return Err(anyhow::anyhow!("Failed to set root receipt key with expiry: {e}"));
        }
    }

    // Record total task duration and success
    TASK_DURATION.observe(start_time.elapsed().as_secs_f64());
    RESOLVE_DURATION.observe(start_time.elapsed().as_secs_f64());
    helpers::record_task_operation("resolve", "complete", "success");

    tracing::info!("Resolve operation completed successfully");
    Ok(assumptions_len)
}
