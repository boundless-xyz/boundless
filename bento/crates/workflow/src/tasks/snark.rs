// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::Agent;
use anyhow::{Context as _, Result, bail};
use risc0_zkvm::{InnerReceipt, ProverOpts, Receipt};
use workflow_common::{
    CompressType, SnarkReq, SnarkResp,
    s3::{GROTH16_BUCKET_DIR, RECEIPT_BUCKET_DIR, SHRINK_BITVM2_BUCKET_DIR, STARK_BUCKET_DIR},
};

/// Converts a stark, stored in s3 to a snark
pub async fn stark2snark(agent: &Agent, job_id: &str, req: &SnarkReq) -> Result<SnarkResp> {
    tracing::info!("Converting stark to snark for job: {job_id}");
    let receipt_key = format!("{RECEIPT_BUCKET_DIR}/{STARK_BUCKET_DIR}/{}.bincode", req.receipt);
    tracing::debug!("Downloading receipt, {receipt_key}");
    let receipt: Receipt = agent
        .s3_client
        .read_from_s3(&receipt_key)
        .await
        .context("[BENTO-SNARK-001] Failed to download receipt from obj store")?;

    tracing::debug!("performing identity predicate on receipt, {job_id}");

    let (snark_receipt, bucket_dir) = match req.compress_type {
        CompressType::None => bail!("Cannot convert to snark with no compression"),
        CompressType::Groth16 => (
            agent
                .prover
                .as_ref()
                .context("Missing prover from resolve task")?
                .compress(&ProverOpts::groth16(), &receipt)
                .context("groth16 compress failed")?,
            GROTH16_BUCKET_DIR,
        ),
        CompressType::ShrinkBitvm2 => (
            shrink_bitvm2::compress_bitvm2(&receipt)
                .await
                .context("shrink blake3 groth16 failed")?,
            SHRINK_BITVM2_BUCKET_DIR,
        ),
    };
    if !matches!(snark_receipt.inner, InnerReceipt::Groth16(_)) {
        bail!("[BENTO-SNARK-004] failed to create groth16 receipt");
    }

    let key = &format!("{RECEIPT_BUCKET_DIR}/{bucket_dir}/{job_id}.bincode");
    // TODO(ec2): fixme
    // receipt
    //     .verify_integrity_with_context(&agent.verifier_ctx)
    //     .context("[BENTO-SNARK-005] Failed to verify compressed snark receipt")?;

    tracing::debug!("Uploading snark receipt to S3: {key}");

    agent
        .s3_client
        .write_to_s3(key, snark_receipt)
        .await
        .context("[BENTO-SNARK-006] Failed to upload final receipt to obj store")?;

    Ok(SnarkResp { snark: job_id.to_string() })
}
