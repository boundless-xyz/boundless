// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use uuid::Uuid;
use workflow_common::ProveReq;

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let index = request.index;
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::debug!("Starting proof of idx: {job_id} - {index}");
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    // Perform all prover operations in a scope to ensure they're dropped before awaits
    let lift_asset = {
        let prover = agent.create_prover();
        let verifier_ctx = agent.create_verifier_ctx();
        let results = prover.segment_preflight(&segment)?;

        // Acquire semaphore permit before GPU operations to prevent OOM
        let _permit = agent
            .gpu_semaphore
            .acquire()
            .await
            .context("Failed to acquire GPU semaphore permit")?;

        tracing::debug!("Acquired GPU permit for segment proof: {job_id} - {index}");
        let segment_receipt = tokio::task::block_in_place(|| {
            prover.prove_segment_core(&verifier_ctx, results).context("Failed to prove segment")
        })?;

        // Drop permit once GPU work is complete
        drop(_permit);

        segment_receipt
            .verify_integrity_with_context(&verifier_ctx)
            .context("Failed to verify segment receipt integrity")?;

        tracing::debug!("Completed proof: {job_id} - {index}");

        tracing::debug!("lifting {job_id} - {index}");

        if agent.is_povw_enabled() {
            let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = prover
                .lift_povw(&segment_receipt)
                .with_context(|| format!("Failed to POVW lift segment {index}"))?;

            lift_receipt
                .verify_integrity_with_context(&verifier_ctx)
                .context("Failed to verify lift receipt integrity")?;

            tracing::debug!("lifting complete {job_id} - {index}");

            // Write out lifted POVW receipt
            serialize_obj(&lift_receipt).expect("Failed to serialize the POVW segment")
        } else {
            let lift_receipt: SuccinctReceipt<ReceiptClaim> = prover
                .lift(&segment_receipt)
                .with_context(|| format!("Failed to lift segment {index}"))?;

            lift_receipt
                .verify_integrity_with_context(&verifier_ctx)
                .context("Failed to verify lift receipt integrity")?;

            tracing::debug!("lifting complete {job_id} - {index}");

            // Write out lifted regular receipt
            serialize_obj(&lift_receipt).expect("Failed to serialize the segment")
        }
    }; // prover and verifier_ctx are dropped here

    redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
        .await?;

    conn.unlink::<_, ()>(&segment_key).await.context("Failed to delete segment key")?;

    Ok(())
}
