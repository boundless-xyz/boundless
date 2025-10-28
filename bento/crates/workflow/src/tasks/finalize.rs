// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::AsyncCommands,
    tasks::{RECUR_RECEIPT_PATH, deserialize_obj, read_image_id},
};
use anyhow::{Context, Result, bail};
use std::time::Instant;
use workflow_common::{
    FinalizeReq,
    metrics::{TASK_DURATION, helpers},
};
// use aws_sdk_s3::primitives::ByteStream;
use risc0_zkvm::{InnerReceipt, Receipt, ReceiptClaim, SuccinctReceipt};
use uuid::Uuid;
use workflow_common::s3::{RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR};

/// Run finalize tasks / cleanup
///
/// Creates the final rollup receipt, uploads that to S3
/// job path
pub async fn finalize(agent: &Agent, job_id: &Uuid, request: &FinalizeReq) -> Result<()> {
    let start_time = Instant::now();
    let mut conn = agent.redis_pool.get().await?;

    let job_prefix = format!("job:{job_id}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{}", request.max_idx);

    // pull the root receipt from redis
    let redis_start = Instant::now();
    let root_receipt: Vec<u8> = match conn.get::<_, Vec<u8>>(&root_receipt_key).await {
        Ok(data) => {
            helpers::record_redis_operation("get", "success", redis_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_redis_operation("get", "error", redis_start.elapsed().as_secs_f64());
            return Err(anyhow::anyhow!(e)
                .context(format!("failed to get the root receipt key: {root_receipt_key}")));
        }
    };

    let root_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(&root_receipt)
        .with_context(|| {
            format!(
                "could not deserialize the root receipt. Data length: {} bytes, first 32 bytes (hex): {:02x?}",
                root_receipt.len(),
                &root_receipt[..root_receipt.len().min(32)]
            )
        })?;

    // construct the journal key and grab the journal from redis
    let journal_key = format!("{job_prefix}:journal");
    let redis_journal_start = Instant::now();
    let journal: Vec<u8> = match conn.get::<_, Vec<u8>>(&journal_key).await {
        Ok(data) => {
            helpers::record_redis_operation(
                "get",
                "success",
                redis_journal_start.elapsed().as_secs_f64(),
            );
            data
        }
        Err(e) => {
            helpers::record_redis_operation(
                "get",
                "error",
                redis_journal_start.elapsed().as_secs_f64(),
            );
            return Err(anyhow::anyhow!(e)
                .context(format!("Journal data not found for key ID: {journal_key}")));
        }
    };

    let journal = deserialize_obj(&journal)
        .with_context(|| {
            format!(
                "could not deserialize the journal. Data length: {} bytes, first 32 bytes (hex): {:02x?}",
                journal.len(),
                &journal[..journal.len().min(32)]
            )
        });
    let rollup_receipt = Receipt::new(InnerReceipt::Succinct(root_receipt), journal?);

    // build the image ID for pulling the image from redis
    let image_key = format!("{job_prefix}:image_id");
    let redis_image_start = Instant::now();
    let image_id_string: String = match conn.get::<_, String>(&image_key).await {
        Ok(data) => {
            helpers::record_redis_operation(
                "get",
                "success",
                redis_image_start.elapsed().as_secs_f64(),
            );
            data
        }
        Err(e) => {
            helpers::record_redis_operation(
                "get",
                "error",
                redis_image_start.elapsed().as_secs_f64(),
            );
            return Err(anyhow::anyhow!(e)
                .context(format!("Journal data not found for key ID: {image_key}")));
        }
    };
    let image_id = read_image_id(&image_id_string)?;

    rollup_receipt.verify(image_id).context("Receipt verification failed")?;

    if !matches!(rollup_receipt.inner, InnerReceipt::Succinct(_)) {
        bail!("rollup_receipt is not Succinct")
    }

    let key = &format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{job_id}.bincode");
    tracing::debug!("Uploading rollup receipt to S3: {}", key);
    let s3_start = Instant::now();
    match agent.s3_client.write_to_s3(key, rollup_receipt).await {
        Ok(()) => {
            helpers::record_s3_operation("write", "success", s3_start.elapsed().as_secs_f64());
        }
        Err(e) => {
            helpers::record_s3_operation("write", "error", s3_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to upload final receipt to obj store"));
        }
    }

    // Record total task duration and success
    TASK_DURATION.observe(start_time.elapsed().as_secs_f64());
    helpers::record_task_operation("finalize", "complete", "success");

    Ok(())
}
