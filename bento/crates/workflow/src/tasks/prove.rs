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
    // Get a single Redis connection
    let mut conn = agent.get_redis_connection().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");
    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    tracing::info!("Starting proof of idx: {job_id} - {index}");
    // Fetch segment data
    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&segment_key)
        .await
        .with_context(|| format!("segment data not found for segment key: {segment_key}"))?;

    // Deserialize the segment
    let segment =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    // Perform the proof operation
    let segment_receipt = agent
        .prover
        .as_ref()
        .context("Missing prover from prove task")?
        .prove_segment(&agent.verifier_ctx, &segment)
        .context("Failed to prove segment")?;

    tracing::info!("Completed proof: {job_id} - {index}");

    // Clone necessary data for the cleanup task
    let pool = agent.redis_pool.clone();
    let ttl = agent.args.redis_ttl;
    let output_key_clone = output_key.clone();

    // Spawn a separate task to handle serialization and storage
    // This allows the main function to return immediately and pick up the next GPU task
    tokio::spawn(async move {
        // Serialize the result (CPU-bound)
        let receipt_asset = match serialize_obj(&segment_receipt) {
            Ok(asset) => asset,
            Err(e) => {
                tracing::error!("Failed to serialize the segment receipt: {}", e);
                return;
            }
        };

        // Store in Redis (I/O bound)
        let mut conn = match redis::get_connection(&pool).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to get Redis connection for cleanup: {}", e);
                return;
            }
        };

        if let Err(e) =
            redis::set_key_with_expiry(&mut conn, &output_key_clone, receipt_asset, Some(ttl)).await
        {
            tracing::error!("Failed to store receipt in Redis: {}", e);
        } else {
            tracing::info!("Proof result stored: {output_key_clone}");
        }
    });

    // Check if there's a next segment to process (index + 1)
    let next_index = index + 1;
    let next_segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{next_index}");

    // Try to fetch the next segment data
    if let Ok(segment_vec) = conn.get::<_, Vec<u8>>(&next_segment_key).await {
        let next_task_id = format!("{task_id}_next");
        let next_output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{next_task_id}");

        tracing::info!("Found next segment, processing: {job_id} - {next_index}");

        // Deserialize the segment
        let segment = match deserialize_obj(&segment_vec) {
            Ok(seg) => seg,
            Err(e) => {
                tracing::error!("Failed to deserialize next segment data: {}", e);
                return Ok(()); // Return successfully from the first segment
            }
        };

        // Perform the proof operation for the second segment
        let segment_receipt = match agent
            .prover
            .as_ref()
            .context("Missing prover from prove task")?
            .prove_segment(&agent.verifier_ctx, &segment)
        {
            Ok(receipt) => receipt,
            Err(e) => {
                tracing::error!("Failed to prove next segment: {}", e);
                return Ok(()); // Return successfully from the first segment
            }
        };

        tracing::info!("Completed proof of second segment: {job_id} - {next_index}");

        // Clone necessary data for the cleanup task
        let pool = agent.redis_pool.clone();
        let ttl = agent.args.redis_ttl;
        let output_key_clone = next_output_key.clone();

        // Spawn a separate task to handle serialization and storage
        tokio::spawn(async move {
            // Serialize the result (CPU-bound)
            let receipt_asset = match serialize_obj(&segment_receipt) {
                Ok(asset) => asset,
                Err(e) => {
                    tracing::error!("Failed to serialize the segment receipt: {}", e);
                    return;
                }
            };

            // Store in Redis (I/O bound)
            let mut conn = match redis::get_connection(&pool).await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::error!("Failed to get Redis connection for cleanup: {}", e);
                    return;
                }
            };

            if let Err(e) =
                redis::set_key_with_expiry(&mut conn, &output_key_clone, receipt_asset, Some(ttl)).await
            {
                tracing::error!("Failed to store receipt in Redis: {}", e);
            } else {
                tracing::info!("Proof result stored: {output_key_clone}");
            }
        });
    }

    // Return immediately after proof(s) are complete, allowing next GPU task to start
    Ok(())
}
