// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::Agent;
use anyhow::{Context as _, Result, bail};
use risc0_zkvm::{InnerReceipt, ProverOpts, Receipt};
use std::time::Instant;
use workflow_common::{
    CompressType, SnarkReq, SnarkResp,
    metrics::helpers,
    s3::{BLAKE3_GROTH16_BUCKET_DIR, GROTH16_BUCKET_DIR, RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR},
};

/// Converts a stark, stored in s3 to a snark
pub async fn stark2snark(agent: &Agent, job_id: &str, req: &SnarkReq) -> Result<SnarkResp> {
    let start_time = Instant::now();
    tracing::info!("Converting stark to snark for job: {job_id}");
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{}.bincode", req.receipt);
    tracing::debug!("Downloading receipt, {receipt_key}");
    let s3_read_start = Instant::now();
    let receipt: Receipt = match agent.s3_client.read_from_s3(&receipt_key).await {
        Ok(data) => {
            helpers::record_s3_operation("read", "success", s3_read_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_s3_operation("read", "error", s3_read_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to download receipt from obj store"));
        }
    };

    tracing::debug!("performing identity predicate on receipt, {job_id}");

    let (snark_receipt_bytes, bucket_dir) = match req.compress_type {
        CompressType::None => bail!("Cannot convert to snark with no compression"),
        CompressType::Groth16 => {
            let groth16_receipt = match agent
                .prover
                .as_ref()
                .context("[BENTO-SNARK-002] Missing prover from resolve task")?
                .compress(&ProverOpts::groth16(), &receipt)
            {
                Ok(receipt) => {
                    helpers::record_task_operation("snark", "compress", "success", 0.0);
                    receipt
                }
                Err(e) => {
                    helpers::record_task_operation("snark", "compress", "error", 0.0);
                    return Err(e.context("groth16 compress failed"));
                }
            };
            if !matches!(groth16_receipt.inner, InnerReceipt::Groth16(_)) {
                bail!("[BENTO-SNARK-004] failed to create groth16 receipt");
            }
            groth16_receipt
                .verify_integrity_with_context(&agent.verifier_ctx)
                .context("[BENTO-SNARK-005] Failed to verify compressed snark receipt")?;
            (bincode::serialize(&groth16_receipt)?, GROTH16_BUCKET_DIR)
        }
        CompressType::Blake3Groth16 => {
            let blake3_receipt = match blake3_groth16::compress_blake3_groth16(&receipt)
                .await
                .context("[BENTO-SNARK-007] blake3 groth16 compress failed")
            {
                Ok(blake3_receipt) => {
                    helpers::record_task_operation("snark", "compress", "success", 0.0);
                    blake3_receipt
                }
                Err(e) => {
                    helpers::record_task_operation("snark", "compress", "error", 0.0);
                    return Err(e.context("blake3 groth16 compress failed"));
                }
            };
            blake3_receipt
                .verify_integrity()
                .context("[BENTO-SNARK-008] Failed to verify blake3 snark receipt")?;
            (bincode::serialize(&blake3_receipt)?, BLAKE3_GROTH16_BUCKET_DIR)
        }
    };

    let key = &format!("{RECEIPT_BUCKET_DIR}/{bucket_dir}/{job_id}.bincode");

    receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("[BENTO-SNARK-005] Failed to verify compressed snark receipt")?;

    tracing::info!("Uploading snark receipt to S3: {key}");

    let s3_write_start = Instant::now();
    match agent.s3_client.write_buf_to_s3(key, snark_receipt_bytes).await {
        Ok(()) => {
            helpers::record_s3_operation(
                "write",
                "success",
                s3_write_start.elapsed().as_secs_f64(),
            );
        }
        Err(e) => {
            helpers::record_s3_operation("write", "error", s3_write_start.elapsed().as_secs_f64());
            return Err(e.context("Failed to upload final receipt to obj store"));
        }
    }

    // Record total task duration and success
    helpers::record_task_operation(
        "snark",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    Ok(SnarkResp { snark: job_id.to_string() })
}
