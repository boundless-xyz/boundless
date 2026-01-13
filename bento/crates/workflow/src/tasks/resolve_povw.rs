// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self},
    tasks::{RECEIPT_PATH, RECUR_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::sha::Digestible;
use risc0_zkvm::{GenericReceipt, ReceiptClaim, SuccinctReceipt, Unknown, WorkClaim};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{
    KECCAK_RECEIPT_PATH, ResolveReq, metrics::helpers, s3::WORK_RECEIPTS_BUCKET_DIR,
};

/// Run the POVW resolve operation
pub async fn resolve_povw(
    agent: &Agent,
    job_id: &Uuid,
    request: &ResolveReq,
) -> Result<Option<u64>> {
    let start_time = Instant::now();
    let max_idx = &request.max_idx;
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{max_idx}");

    tracing::debug!("Starting POVW resolve for job_id: {job_id}, max_idx: {max_idx}");

    let mut conn = agent.redis_pool.get().await?;
    // Get root receipt using Redis helper
    let receipt: Vec<u8> = redis::get_key(&mut conn, &root_receipt_key).await.map_err(|e| {
        anyhow::anyhow!(e)
            .context(format!("segment data not found for root receipt key: {root_receipt_key}"))
    })?;

    tracing::debug!("Root receipt size: {} bytes", receipt.len());

    // Deserialize as POVW receipt
    let povw_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> =
        deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&receipt)
            .context("Failed to deserialize as POVW receipt")?;

    // Unwrap the POVW receipt to get the ReceiptClaim for processing
    let povw_unwrap_start = Instant::now();
    let mut conditional_receipt: SuccinctReceipt<ReceiptClaim> =
        match agent.prover.as_ref().unwrap().unwrap_povw(&povw_receipt) {
            Ok(receipt) => {
                helpers::record_task(
                    "resolve_povw",
                    "povw_unwrap",
                    "success",
                    povw_unwrap_start.elapsed().as_secs_f64(),
                );
                receipt
            }
            Err(e) => {
                helpers::record_task(
                    "resolve_povw",
                    "povw_unwrap",
                    "error",
                    povw_unwrap_start.elapsed().as_secs_f64(),
                );
                return Err(e.context("POVW unwrap failed"));
            }
        };

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
                    let union_receipt: Vec<u8> =
                        redis::get_key(&mut conn, &union_root_receipt_key).await.context(
                            format!("Failed to get union receipt: {union_root_receipt_key}"),
                        )?;

                    // Debug: Check the size and content of the union receipt
                    tracing::debug!("Union receipt size: {} bytes", union_receipt.len());
                    if union_receipt.is_empty() {
                        return Err(anyhow::anyhow!(
                            "Union receipt is empty for key: {}",
                            union_root_receipt_key
                        ));
                    }

                    let union_receipt: SuccinctReceipt<Unknown> = deserialize_obj(&union_receipt)
                        .with_context(|| {
                        format!(
                            "Failed to deserialize union receipt (size: {} bytes) from key: {}",
                            union_receipt.len(),
                            union_root_receipt_key
                        )
                    })?;
                    union_claim = union_receipt.claim.digest().to_string();

                    // Resolve union receipt
                    tracing::debug!("Resolving union claim digest: {union_claim}");
                    conditional_receipt = agent
                        .prover
                        .as_ref()
                        .context("Missing prover from resolve task")?
                        .resolve(&conditional_receipt, &union_receipt)
                        .context("Failed to resolve the union receipt")?;
                }

                for assumption in assumptions {
                    let assumption_claim = assumption.as_value()?.claim.to_string();
                    if assumption_claim.eq(&union_claim) {
                        tracing::debug!("Skipping already resolved union claim: {union_claim}");
                        continue;
                    }
                    let assumption_key = format!("{receipts_key}:{assumption_claim}");
                    tracing::debug!("Deserializing assumption with key: {assumption_key}");
                    let assumption_bytes: Vec<u8> =
                        redis::get_key(&mut conn, &assumption_key).await.context(format!(
                            "corroborating receipt not found: key {assumption_key}"
                        ))?;

                    // Debug: Check the size and content of the assumption receipt
                    tracing::debug!(
                        "Assumption receipt size: {} bytes for key: {}",
                        assumption_bytes.len(),
                        assumption_key
                    );
                    if assumption_bytes.is_empty() {
                        return Err(anyhow::anyhow!(
                            "Assumption receipt is empty for key: {}",
                            assumption_key
                        ));
                    }

                    let assumption_receipt = deserialize_obj(&assumption_bytes)
                        .with_context(|| format!("Failed to deserialize assumption receipt (size: {} bytes) from key: {}", assumption_bytes.len(), assumption_key))?;

                    // Resolve
                    conditional_receipt = agent
                        .prover
                        .as_ref()
                        .context("Missing prover from resolve task")?
                        .resolve(&conditional_receipt, &assumption_receipt)
                        .context("Failed to resolve the conditional receipt")?;
                }
                tracing::debug!("Resolve complete for job_id: {job_id}");
            }
        }
    }

    // Write out the resolved receipt
    tracing::debug!("Serializing resolved receipt");
    let serialized_asset =
        serialize_obj(&conditional_receipt).context("Failed to serialize resolved receipt")?;

    // Store resolved receipt using Redis helper
    tracing::debug!("Writing resolved receipt to Redis key: {root_receipt_key}");
    redis::set_key_with_expiry(
        &mut conn,
        &root_receipt_key,
        serialized_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e).context("Failed to set root receipt key with expiry"))?;

    // Save the resolved receipt to work receipts bucket for later consumption
    let work_receipt_key = format!("{WORK_RECEIPTS_BUCKET_DIR}/{job_id}.bincode");
    tracing::debug!("Saving resolved POVW receipt to work receipts bucket: {work_receipt_key}");

    // Save the resolved receipt to work receipts bucket for later consumption
    // Wrap the POVW receipt as GenericReceipt::Succinct for RISC Zero VM integration
    let wrapped_povw_receipt = GenericReceipt::Succinct(povw_receipt.clone());

    agent
        .s3_client
        .write_to_s3(&work_receipt_key, &wrapped_povw_receipt)
        .await
        .context("Failed to save resolved POVW receipt to work receipts bucket")?;

    // Store POVW metadata alongside the receipt
    let metadata_key = format!("{WORK_RECEIPTS_BUCKET_DIR}/{job_id}_metadata.json");

    // Only include POVW fields if they are actually set and non-empty
    let mut metadata_fields = serde_json::Map::new();
    metadata_fields.insert("job_id".to_string(), serde_json::Value::String(job_id.to_string()));
    if let Ok(log_id) = std::env::var("POVW_LOG_ID") {
        metadata_fields.insert("povw_log_id".to_string(), serde_json::Value::String(log_id));
    }

    let povw_job_number = povw_receipt
        .clone()
        .claim
        .value()
        .and_then(|x| x.work.value())
        .ok()
        .map(|work| format!("{}", work.nonce_min.job))
        .context("Failed to get POVW job number")
        .unwrap();

    metadata_fields
        .insert("povw_job_number".to_string(), serde_json::Value::String(povw_job_number));

    let povw_metadata = serde_json::Value::Object(metadata_fields);

    agent
        .s3_client
        .write_buf_to_s3(&metadata_key, serde_json::to_vec(&povw_metadata)?)
        .await
        .context("Failed to save POVW metadata to work receipts bucket")?;

    tracing::info!("POVW resolve operation completed successfully");
    // Record total operation duration
    helpers::record_task_operation(
        "resolve_povw",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(assumptions_len)
}
