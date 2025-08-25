// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    povw::PovwConfig,
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, read_image_id, RECUR_RECEIPT_PATH},
    Agent,
};
use anyhow::{bail, Context, Result};
use workflow_common::FinalizeReq;
// use aws_sdk_s3::primitives::ByteStream;
use risc0_zkvm::{InnerReceipt, Receipt, ReceiptClaim, SuccinctReceipt, WorkClaim};
use uuid::Uuid;
use workflow_common::s3::{RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR};

/// Run finalize tasks / cleanup
///
/// Creates the final rollup receipt, uploads that to S3
/// job path
pub async fn finalize(agent: &Agent, job_id: &Uuid, request: &FinalizeReq) -> Result<()> {
    let mut conn = agent.redis_pool.get().await?;

    let job_prefix = format!("job:{job_id}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.max_idx);

    // pull the root receipt from redis
    let root_receipt: Vec<u8> = conn
        .get::<_, Vec<u8>>(&root_receipt_key)
        .await
        .with_context(|| format!("failed to get the root receipt key: {root_receipt_key}"))?;

    // Try to deserialize as POVW receipt first, then fall back to regular receipt
    let final_receipt: SuccinctReceipt<ReceiptClaim> = if let Some(_povw_config) = PovwConfig::from_env() {
        tracing::info!("POVW enabled, attempting to unwrap POVW receipt");

        // First try to deserialize as a POVW receipt (WorkClaim<ReceiptClaim>)
        if let Ok(povw_receipt) = deserialize_obj::<SuccinctReceipt<WorkClaim<ReceiptClaim>>>(&root_receipt) {
            tracing::info!("Detected POVW receipt, unwrapping...");

            // Unwrap the POVW receipt to get the final receipt
            match agent.prover.as_ref() {
                Some(prover) => {
                    // Use the POVW unwrap method - this is required for POVW functionality
                    prover.unwrap_povw(&povw_receipt)
                        .context("POVW unwrap method not available - POVW functionality requires RISC Zero POVW support")?
                }
                None => {
                    tracing::error!("No prover available for POVW unwrapping");
                    return Err(anyhow::anyhow!("POVW unwrapping requires a prover but none is available"));
                }
            }
        } else {
            // Fall back to regular receipt if POVW deserialization fails
            tracing::warn!("Failed to deserialize as POVW receipt, trying regular receipt");
            let regular_receipt: SuccinctReceipt<ReceiptClaim> =
                deserialize_obj(&root_receipt).context("could not deserialize the root receipt")?;
            regular_receipt
        }
    } else {
        // POVW not enabled, use regular receipt
        let regular_receipt: SuccinctReceipt<ReceiptClaim> =
            deserialize_obj(&root_receipt).context("could not deserialize the root receipt")?;
        regular_receipt
    };

    // construct the journal key and grab the journal from redis
    let journal_key = format!("{job_prefix}:journal");
    let journal: Vec<u8> = conn
        .get::<_, Vec<u8>>(&journal_key)
        .await
        .with_context(|| format!("Journal data not found for key ID: {journal_key}"))?;

    let journal: Vec<u8> = deserialize_obj(&journal).context("could not deseriailize the journal")?;

    // Create the final rollup receipt
    let rollup_receipt = Receipt::new(InnerReceipt::Succinct(final_receipt), journal.clone());

    // build the image ID for pulling the image from redis
    let image_key = format!("{job_prefix}:image_id");
    let image_id_string: String = conn
        .get::<_, String>(&image_key)
        .await
        .with_context(|| format!("Journal data not found for key ID: {image_key}"))?;
    let image_id = read_image_id(&image_id_string)?;

    // Verify the final receipt
    rollup_receipt.verify(image_id).context("Receipt verification failed")?;

    if !matches!(rollup_receipt.inner, InnerReceipt::Succinct(_)) {
        bail!("rollup_receipt is not succinct")
    }

    let key = &format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{job_id}.bincode");
    tracing::debug!("Uploading rollup receipt to S3: {}", key);
    agent
        .s3_client
        .write_to_s3(key, rollup_receipt)
        .await
        .context("Failed to upload final receipt to obj store")?;

    tracing::debug!("Deleting the keyspace {job_prefix}:*");
    // remove the redis keys after job is completed
    redis::scan_and_delete(&mut conn, &job_prefix)
        .await
        .context("Failed to delete all redis keys")?;

    Ok(())
}
