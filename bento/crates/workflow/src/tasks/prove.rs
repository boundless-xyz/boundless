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
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{
    ProveReq,
    metrics::{PROVE_DURATION, TASK_DURATION, TASK_OPERATIONS, helpers},
};

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let start_time = Instant::now();
    let index = request.index;
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::debug!("Starting proof of idx: {job_id} - {index}");

    // Record Redis operation for segment retrieval
    let redis_start = Instant::now();
    let segment_vec: Vec<u8> = match conn.get::<_, Vec<u8>>(&segment_key).await {
        Ok(data) => {
            helpers::record_redis_operation("get", "success", redis_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_redis_operation("get", "error", redis_start.elapsed().as_secs_f64());
            return Err(anyhow::anyhow!(e)
                .context(format!("segment data not found for segment key: {segment_key}")));
        }
    };
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    // Record proving operation
    let prove_start = Instant::now();
    let segment_receipt = match agent
        .prover
        .as_ref()
        .context("Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
    {
        Ok(receipt) => {
            PROVE_DURATION.observe(prove_start.elapsed().as_secs_f64());
            TASK_OPERATIONS.with_label_values(&["prove", "prove_segment", "success"]).inc();
            receipt
        }
        Err(e) => {
            TASK_OPERATIONS.with_label_values(&["prove", "prove_segment", "error"]).inc();
            return Err(e.context("Failed to prove segment"));
        }
    };

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify segment receipt integrity")?;

    tracing::debug!("Completed proof: {job_id} - {index}");

    tracing::debug!("lifting {job_id} - {index}");

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    if agent.is_povw_enabled() {
        let lift_povw_start = Instant::now();
        let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = match agent
            .prover
            .as_ref()
            .context("Missing prover from resolve task")?
            .lift_povw(&segment_receipt)
        {
            Ok(receipt) => {
                LIFT_POVW_DURATION.observe(lift_povw_start.elapsed().as_secs_f64());
                TASK_OPERATIONS.with_label_values(&["prove", "lift_povw", "success"]).inc();
                receipt
            }
            Err(e) => {
                TASK_OPERATIONS.with_label_values(&["prove", "lift_povw", "error"]).inc();
                return Err(e.context(format!("Failed to POVW lift segment {index}")));
            }
        };

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted POVW receipt
        let lift_asset =
            serialize_obj(&lift_receipt).expect("Failed to serialize the POVW segment");
        let redis_write_start = Instant::now();
        match redis::set_key_with_expiry(
            &mut conn,
            &output_key,
            lift_asset,
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
                return Err(e.into());
            }
        }
    } else {
        let lift_receipt: SuccinctReceipt<ReceiptClaim> = match agent
            .prover
            .as_ref()
            .context("Missing prover from resolve task")?
            .lift(&segment_receipt)
        {
            Ok(receipt) => {
                TASK_OPERATIONS.with_label_values(&["prove", "lift", "success"]).inc();
                receipt
            }
            Err(e) => {
                TASK_OPERATIONS.with_label_values(&["prove", "lift", "error"]).inc();
                return Err(e.context(format!("Failed to lift segment {index}")));
            }
        };

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted regular receipt
        let lift_asset = serialize_obj(&lift_receipt).expect("Failed to serialize the segment");
        let redis_write_start = Instant::now();
        match redis::set_key_with_expiry(
            &mut conn,
            &output_key,
            lift_asset,
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
                return Err(e.into());
            }
        }
    }

    // Record segment cleanup operation
    let cleanup_start = Instant::now();
    match conn.unlink::<_, ()>(&segment_key).await {
        Ok(()) => {
            helpers::record_redis_operation(
                "unlink",
                "success",
                cleanup_start.elapsed().as_secs_f64(),
            );
        }
        Err(e) => {
            helpers::record_redis_operation(
                "unlink",
                "error",
                cleanup_start.elapsed().as_secs_f64(),
            );
            return Err(anyhow::anyhow!(e).context("Failed to delete segment key"));
        }
    }

    // Record total task duration and success
    TASK_DURATION.observe(start_time.elapsed().as_secs_f64());
    helpers::record_task_operation("prove", "complete", "success");

    Ok(())
}
