// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{COPROC_CB_PATH, serialize_obj},
};
use anyhow::{Context, Result, anyhow, bail};
use risc0_zkvm::ProveKeccakRequest;
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{
    KECCAK_RECEIPT_PATH, KeccakReq,
    metrics::{KECCAK_DURATION, TASK_DURATION, TASK_OPERATIONS, helpers},
};

fn try_keccak_bytes_to_input(input: &[u8]) -> Result<Vec<[u64; 25]>> {
    let chunks = input.chunks_exact(std::mem::size_of::<[u64; 25]>());
    if !chunks.remainder().is_empty() {
        bail!("Input length must be a multiple of KeccakState size");
    }
    chunks
        .map(bytemuck::try_pod_read_unaligned)
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow!("Failed to convert input bytes to KeccakState: {}", e))
}

/// Run the keccak prove + lift operation
pub async fn keccak(
    agent: &Agent,
    job_id: &Uuid,
    task_id: &str,
    request: &KeccakReq,
) -> Result<()> {
    let start_time = Instant::now();
    let mut conn = agent.redis_pool.get().await?;
    let keccak_input_path = format!("job:{job_id}:{}:{}", COPROC_CB_PATH, request.claim_digest);

    // Record Redis operation for keccak input retrieval
    let redis_start = Instant::now();
    let keccak_input: Vec<u8> = match conn.get::<_, Vec<u8>>(&keccak_input_path).await {
        Ok(data) => {
            helpers::record_redis_operation("get", "success", redis_start.elapsed().as_secs_f64());
            data
        }
        Err(e) => {
            helpers::record_redis_operation("get", "error", redis_start.elapsed().as_secs_f64());
            return Err(anyhow::anyhow!(e)
                .context(format!("segment data not found for segment key: {keccak_input_path}")));
        }
    };

    let keccak_req = ProveKeccakRequest {
        claim_digest: request.claim_digest,
        po2: request.po2,
        control_root: request.control_root,
        input: try_keccak_bytes_to_input(&keccak_input)?,
    };

    if keccak_req.input.is_empty() {
        anyhow::bail!("Received empty keccak input with claim_digest: {}", request.claim_digest);
    }

    tracing::debug!("Keccak proving {}", request.claim_digest);

    // Record keccak proving operation
    let keccak_receipt = match agent
        .prover
        .as_ref()
        .context("Missing prover from keccak prove task")?
        .prove_keccak(&keccak_req)
    {
        Ok(receipt) => {
            TASK_OPERATIONS.with_label_values(&["keccak", "prove_keccak", "success"]).inc();
            receipt
        }
        Err(e) => {
            TASK_OPERATIONS.with_label_values(&["keccak", "prove_keccak", "error"]).inc();
            return Err(e.context("Failed to prove_keccak"));
        }
    };

    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}:{task_id}");
    let keccak_receipt_bytes =
        serialize_obj(&keccak_receipt).context("Failed to serialize keccak receipt")?;

    // Record Redis write operation
    let redis_write_start = Instant::now();
    match redis::set_key_with_expiry(
        &mut conn,
        &receipts_key,
        keccak_receipt_bytes,
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
            return Err(anyhow::anyhow!(e).context("Failed to write keccak receipt to redis"));
        }
    }

    tracing::debug!("Completed keccak proving {}", request.claim_digest);

    // Record cleanup operation
    let cleanup_start = Instant::now();
    match conn.unlink::<_, ()>(&keccak_input_path).await {
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
            return Err(anyhow::anyhow!(e).context("Failed to delete keccak input path key"));
        }
    }

    // Record total task duration and success
    TASK_DURATION.observe(start_time.elapsed().as_secs_f64());
    KECCAK_DURATION.observe(start_time.elapsed().as_secs_f64());
    helpers::record_task("keccak", "complete", "success", start_time.elapsed().as_secs_f64());

    Ok(())
}
