// Copyright 2026 Boundless Foundation, Inc.
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
use workflow_common::{KECCAK_RECEIPT_PATH, KeccakReq, metrics::helpers};

fn try_keccak_bytes_to_input(input: &[u8]) -> Result<Vec<[u64; 25]>> {
    let chunks = input.chunks_exact(std::mem::size_of::<[u64; 25]>());
    if !chunks.remainder().is_empty() {
        bail!("[BENTO-KECCAK-001] Input length must be a multiple of KeccakState size");
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
    let keccak_input_path =
        format!("job:{job_id}:{}:{task_id}:{}", COPROC_CB_PATH, request.claim_digest);

    // Get keccak input using Redis helper, preferring the task-scoped key but falling back to the
    // legacy location to handle in-progress tasks during upgrade.
    let keccak_input: Vec<u8> = match redis::get_key(&mut conn, &keccak_input_path).await {
        Ok(input) => input,
        Err(_) => {
            tracing::warn!(
                "[BENTO-KECCAK-013] Keccak input not found for task {} digest {}, falling back to legacy key",
                task_id,
                request.claim_digest
            );
            let legacy_keccak_input_path =
                format!("job:{job_id}:{}:{}", COPROC_CB_PATH, request.claim_digest);
            redis::get_key(&mut conn, &legacy_keccak_input_path).await?
        }
    };

    let keccak_req = ProveKeccakRequest {
        claim_digest: request.claim_digest,
        po2: request.po2,
        control_root: request.control_root,
        input: try_keccak_bytes_to_input(&keccak_input)?,
    };

    if keccak_req.input.is_empty() {
        anyhow::bail!(
            "[BENTO-KECCAK-002] Received empty keccak input with claim_digest: {}",
            request.claim_digest
        );
    }

    tracing::debug!("Keccak proving {}", request.claim_digest);

    // Record keccak proving operation
    let keccak_receipt = match agent
        .prover
        .as_ref()
        .context("[BENTO-KECCAK-003] Missing prover from keccak prove task")?
        .prove_keccak(&keccak_req)
    {
        Ok(receipt) => {
            helpers::record_task_operation("keccak", "prove_keccak", "success", 0.0);
            receipt
        }
        Err(e) => {
            helpers::record_task_operation("keccak", "prove_keccak", "error", 0.0);
            return Err(e.context("Failed to prove_keccak"));
        }
    };

    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}:{task_id}");
    let keccak_receipt_bytes = serialize_obj(&keccak_receipt)
        .context("[BENTO-KECCAK-005] Failed to serialize keccak receipt")?;

    // Store keccak receipt using Redis helper
    redis::set_key_with_expiry(
        &mut conn,
        &receipts_key,
        keccak_receipt_bytes,
        Some(agent.args.redis_ttl),
    )
    .await
    .map_err(|e| anyhow::anyhow!(e).context("Failed to write keccak receipt to redis"))?;

    tracing::debug!("Completed keccak proving {}", request.claim_digest);

    // Clean up keccak input
    let cleanup_start = Instant::now();
    let cleanup_result = conn.unlink::<_, ()>(&keccak_input_path).await;
    let cleanup_status = if cleanup_result.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(
        "unlink",
        cleanup_status,
        cleanup_start.elapsed().as_secs_f64(),
    );
    cleanup_result
        .map_err(|e| anyhow::anyhow!(e).context("Failed to delete keccak input path key"))?;

    // Record total task duration and success
    helpers::record_task_operation(
        "keccak",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );
    Ok(())
}
