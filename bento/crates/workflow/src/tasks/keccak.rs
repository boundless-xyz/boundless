// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{serialize_obj, COPROC_CB_PATH, RECEIPT_PATH},
    Agent,
};
use anyhow::{anyhow, bail, Context, Result};
use risc0_zkvm::ProveKeccakRequest;
use uuid::Uuid;
use workflow_common::KeccakReq;

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
    let mut conn = redis::get_connection(&agent.redis_pool).await?;

    let keccak_input_path = format!("job:{job_id}:{}:{}", COPROC_CB_PATH, request.claim_digest);
    let keccak_input: Vec<u8> = conn
        .get::<_, Vec<u8>>(&keccak_input_path)
        .await
        .with_context(|| format!("segment data not found for segment key: {keccak_input_path}"))?;

    let keccak_req = ProveKeccakRequest {
        claim_digest: request.claim_digest,
        po2: request.po2,
        control_root: request.control_root,
        input: try_keccak_bytes_to_input(&keccak_input)?,
    };

    if keccak_req.input.is_empty() {
        anyhow::bail!("Received empty keccak input with claim_digest: {}", request.claim_digest);
    }

    tracing::info!("Keccak proving {}", request.claim_digest);

    // Main computational work
    let keccak_receipt = agent
        .prover
        .as_ref()
        .context("Missing prover from keccak prove task")?
        .prove_keccak(&keccak_req)
        .context("Failed to prove_keccak")?;

    // Clone data needed for background task
    let pool = agent.redis_pool.clone();
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}:{task_id}");
    let ttl = agent.args.redis_ttl;
    let claim_digest = request.claim_digest;

    // Spawn background task for serialization and storage
    tokio::spawn(async move {
        // Serialize the receipt (CPU-bound)
        let keccak_receipt_bytes = match serialize_obj(&keccak_receipt) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::error!("Failed to serialize keccak receipt: {}", e);
                return;
            }
        };

        // Store in Redis (I/O bound)
        let mut task_conn = match redis::get_connection(&pool).await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!("Failed to get Redis connection for keccak storage: {}", e);
                return;
            }
        };

        if let Err(e) = redis::set_key_with_expiry(
            &mut task_conn,
            &receipts_key,
            keccak_receipt_bytes,
            Some(ttl),
        )
        .await
        {
            tracing::error!("Failed to write keccak receipt to redis: {}", e);
            return;
        }

        tracing::info!("Completed keccak proving and storage for {}", claim_digest);
    });

    tracing::info!("Completed keccak computation for {}", request.claim_digest);

    // Return immediately after computation
    Ok(())
}
