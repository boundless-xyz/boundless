// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, AsyncCommands, RedisPool},
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use workflow_common::ProveReq;

/// Type alias for the prefetch cache
type PrefetchCache = Arc<Mutex<HashMap<Uuid, HashMap<usize, Vec<u8>>>>>;

/// Prefetch a segment into the cache
async fn prefetch_segment(
    redis_pool: &RedisPool,
    prefetch_cache: &PrefetchCache,
    job_id: &Uuid,
    segment_index: usize,
) -> Result<()> {
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{segment_index}");

    let mut conn = redis_pool.get().await?;
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;

    // Store in prefetch cache
    {
        let mut cache = prefetch_cache.lock().unwrap();
        cache.entry(*job_id).or_default().insert(segment_index, segment_vec);
    }

    tracing::debug!("Prefetched segment {segment_index} for job {job_id}");
    Ok(())
}

/// Get a segment from cache or Redis
async fn get_segment(
    redis_pool: &RedisPool,
    prefetch_cache: &PrefetchCache,
    job_id: &Uuid,
    segment_index: usize,
) -> Result<Vec<u8>> {
    // First check prefetch cache
    {
        let cache = prefetch_cache.lock().unwrap();
        if let Some(job_cache) = cache.get(job_id)
            && let Some(segment_data) = job_cache.get(&segment_index)
        {
            tracing::debug!("Using prefetched segment {segment_index} for job {job_id}");
            return Ok(segment_data.clone());
        }
    }

    // Fallback to Redis if not in cache
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{segment_index}");

    let mut conn = redis_pool.get().await?;
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;

    tracing::debug!("Fetched segment {segment_index} from Redis for job {job_id}");
    Ok(segment_vec)
}

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let index = request.index;

    tracing::debug!("Starting proof of idx: {job_id} - {index}");

    // Use the get_segment function which checks prefetch cache first
    let segment_vec = get_segment(&agent.redis_pool, &agent.prefetch_cache, job_id, index).await?;
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    // Start prefetching the next segment while GPU works on current one
    let next_index = index + 1;
    let prefetch_handle = {
        let redis_pool = agent.redis_pool.clone();
        let prefetch_cache = agent.prefetch_cache.clone();
        let job_id = *job_id;
        tokio::spawn(async move {
            if let Err(e) =
                prefetch_segment(&redis_pool, &prefetch_cache, &job_id, next_index).await
            {
                tracing::debug!("Failed to prefetch segment {next_index} for job {job_id}: {e}");
            }
        })
    };

    let segment_receipt = agent
        .prover
        .as_ref()
        .context("Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
        .context("Failed to prove segment")?;

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify segment receipt integrity")?;

    tracing::debug!("Completed proof: {job_id} - {index}");

    // Wait for prefetch to complete (it may have already finished)
    let _ = prefetch_handle.await;

    tracing::debug!("lifting {job_id} - {index}");

    let job_prefix = format!("job:{job_id}");
    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    let mut conn = agent.redis_pool.get().await?;

    if agent.is_povw_enabled() {
        let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = agent
            .prover
            .as_ref()
            .context("Missing prover from resolve task")?
            .lift_povw(&segment_receipt)
            .with_context(|| format!("Failed to POVW lift segment {index}"))?;

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted POVW receipt
        let lift_asset =
            serialize_obj(&lift_receipt).expect("Failed to serialize the POVW segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await?;
    } else {
        let lift_receipt: SuccinctReceipt<ReceiptClaim> = agent
            .prover
            .as_ref()
            .context("Missing prover from resolve task")?
            .lift(&segment_receipt)
            .with_context(|| format!("Failed to lift segment {index}"))?;

        lift_receipt
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("Failed to verify lift receipt integrity")?;

        tracing::debug!("lifting complete {job_id} - {index}");

        // Write out lifted regular receipt
        let lift_asset = serialize_obj(&lift_receipt).expect("Failed to serialize the segment");
        redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(agent.args.redis_ttl))
            .await?;
    }

    Ok(())
}
