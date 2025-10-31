// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::Agent;
use anyhow::{Context as _, Result, bail};
use risc0_zkvm::{InnerReceipt, ProverOpts, Receipt};
use std::time::Instant;
use workflow_common::{
    SnarkReq, SnarkResp,
    metrics::helpers,
    s3::{GROTH16_BUCKET_DIR, RECEIPT_BUCKET_DIR, STARK_BUCKET_DIR},
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

    let opts = ProverOpts::groth16();
    let snark_receipt = match agent
        .prover
        .as_ref()
        .context("Missing prover from resolve task")?
        .compress(&opts, &receipt)
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

    if !matches!(snark_receipt.inner, InnerReceipt::Groth16(_)) {
        bail!("failed to create groth16 receipt");
    }

    receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify compressed snark receipt")?;

    let key = &format!("{RECEIPT_BUCKET_DIR}/{GROTH16_BUCKET_DIR}/{job_id}.bincode");
    tracing::debug!("Uploading snark receipt to S3: {key}");

    let s3_write_start = Instant::now();
    match agent.s3_client.write_to_s3(key, snark_receipt).await {
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
