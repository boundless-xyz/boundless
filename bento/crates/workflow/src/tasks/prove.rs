// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECUR_RECEIPT_PATH, SEGMENTS_PATH},
    Agent,
};
use anyhow::{Context, Result};
use uuid::Uuid;
use workflow_common::ProveReq;

/// Run a prove request
pub async fn prover(agent: &Agent, job_id: &Uuid, task_id: &str, request: &ProveReq) -> Result<()> {
    let index = request.index;
    let mut conn = redis::get_connection(&agent.redis_pool).await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::info!("Starting proof of idx: {job_id} - {index}");
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    let segment_receipt = agent
        .prover
        .as_ref()
        .context("Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
        .context("Failed to prove segment")?;

    tracing::info!("Completed proof: {job_id} - {index}");

    // Clone necessary data for the cleanup task
    let pool = agent.redis_pool.clone();
    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");
    let ttl = agent.args.redis_ttl;

    // Spawn a separate task to handle serialization and storage
    // This allows the main function to return immediately and pick up the next GPU task
    tokio::spawn(async move {
        // Serialize the result (CPU-bound)
        let receipt_asset =
            serialize_obj(&segment_receipt).expect("Failed to serialize the segment receipt");

        // Store in Redis (I/O bound)
        let mut conn = match redis::get_connection(&pool).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to get Redis connection for cleanup: {}", e);
                return;
            }
        };

        if let Err(e) =
            redis::set_key_with_expiry(&mut conn, &output_key, receipt_asset, Some(ttl)).await
        {
            tracing::error!("Failed to store receipt in Redis: {}", e);
        }

        tracing::info!("Proof result stored: {output_key}");
    });

    // Return immediately after proof is complete, allowing next GPU task to start
    Ok(())
}
