// Copyright 2025 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::AsyncCommands,
    tasks::{COPROC_CB_PATH, serialize_obj},
};
use anyhow::{Context, Result, anyhow, bail};
use deadpool_redis::redis::pipe;
use risc0_zkvm::{Digest, ProveKeccakRequest};
use std::collections::{HashMap, HashSet};
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

/// Batch keccak prove operations with optimized Redis operations
pub async fn batch_keccak(
    agent: &Agent,
    job_id: &Uuid,
    requests: &[(String, KeccakReq)], // (task_id, request) pairs
) -> Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let start_time = Instant::now();
    let batch_size = requests.len();
    tracing::debug!("Starting batch keccak for job {job_id} with {batch_size} keccak proves");

    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");

    // Collect all unique claim_digest keys needed across all keccak tasks
    let mut unique_digests = HashSet::new();
    for (_, req) in requests {
        unique_digests.insert(req.claim_digest);
    }

    // Convert to Vec to maintain consistent ordering
    let unique_digests_vec: Vec<Digest> = unique_digests.into_iter().collect();
    let input_keys: Vec<String> = unique_digests_vec
        .iter()
        .map(|digest| format!("{job_prefix}:{COPROC_CB_PATH}:{digest}"))
        .collect();

    // MGET all unique keccak inputs at once
    let mget_start = Instant::now();
    let inputs_data: Vec<Vec<u8>> = conn
        .mget(&input_keys)
        .await
        .context("Failed to MGET keccak inputs")?;
    helpers::record_redis_operation("mget", "success", mget_start.elapsed().as_secs_f64());

    tracing::debug!(
        "Fetched {} unique keccak inputs for {batch_size} proves in {:?}",
        input_keys.len(),
        mget_start.elapsed()
    );

    // Build a map of claim_digest -> input data
    let mut input_map: HashMap<Digest, Vec<u8>> = HashMap::new();
    for (i, digest) in unique_digests_vec.iter().enumerate() {
        input_map.insert(*digest, inputs_data[i].clone());
    }

    // Process each keccak prove sequentially
    let mut keccak_results = Vec::with_capacity(batch_size);

    for (task_id, req) in requests {
        tracing::debug!("Keccak proving {}", req.claim_digest);

        // Get input from map
        let keccak_input = input_map
            .get(&req.claim_digest)
            .with_context(|| format!("Keccak input {} not found in batch", req.claim_digest))?;

        let keccak_req = ProveKeccakRequest {
            claim_digest: req.claim_digest,
            po2: req.po2,
            control_root: req.control_root,
            input: try_keccak_bytes_to_input(keccak_input)?,
        };

        if keccak_req.input.is_empty() {
            bail!(
                "[BENTO-KECCAK-BATCH-001] Received empty keccak input with claim_digest: {}",
                req.claim_digest
            );
        }

        // Prove keccak
        let keccak_start = Instant::now();
        let keccak_receipt = agent
            .prover
            .as_ref()
            .context("[BENTO-KECCAK-BATCH-002] Missing prover from keccak prove task")?
            .prove_keccak(&keccak_req)?;
        let keccak_elapsed = keccak_start.elapsed().as_secs_f64();
        helpers::record_task_operation("keccak", "prove_keccak", "success", keccak_elapsed);

        let keccak_receipt_bytes = serialize_obj(&keccak_receipt)
            .context("[BENTO-KECCAK-BATCH-003] Failed to serialize keccak receipt")?;

        keccak_results.push((task_id.clone(), keccak_receipt_bytes));
        tracing::debug!("Completed keccak proving {}", req.claim_digest);
    }

    // Pipeline SET operations for all keccak receipts
    let pipeline_start = Instant::now();
    let mut pipe_set = pipe();
    for (task_id, result) in &keccak_results {
        let output_key = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}:{task_id}");
        pipe_set.set_ex(&output_key, result, agent.args.redis_ttl);
    }
    pipe_set
        .query_async::<_, ()>(&mut *conn)
        .await
        .context("Failed to pipeline SET keccak results")?;
    helpers::record_redis_operation(
        "pipeline_set",
        "success",
        pipeline_start.elapsed().as_secs_f64(),
    );

    tracing::debug!(
        "Stored {} keccak results in {:?}",
        batch_size,
        pipeline_start.elapsed()
    );

    // Note: Not cleaning up inputs per TODO in original code
    // TODO refactor the keccak paths/flow with a breaking change, to avoid holding value until TTL
    //     ref: https://linear.app/boundlessnetwork/issue/BM-2056

    // Record total batch operation
    helpers::record_task_operation(
        "keccak",
        "batch_complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    tracing::info!(
        "Batch keccak complete for job {job_id}: {batch_size} proves in {:?}",
        start_time.elapsed()
    );

    Ok(())
}

/// Run the keccak prove + lift operation (delegates to batch_keccak for code reuse)
pub async fn keccak(
    agent: &Agent,
    job_id: &Uuid,
    task_id: &str,
    request: &KeccakReq,
) -> Result<()> {
    // Just call the batch version with a single item
    batch_keccak(agent, job_id, &[(task_id.to_string(), *request)]).await
}
