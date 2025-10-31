// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    redis::{self, RedisPool},
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{
    DEFAULT_MAX_PO2, ProverOpts, ReceiptClaim, SegmentReceipt, SuccinctReceipt, VerifierContext,
    WorkClaim, get_prover_server,
};
use serde_json::Value;
use sqlx::{PgPool, query_scalar};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{ProveReq, metrics::helpers};

/// Result of scheduling a prove task.
pub enum ProverOutcome {
    /// All follow-up work (lift, DB update) has been offloaded and will complete asynchronously.
    Deferred,
}

/// Run a prove request
pub async fn prover(
    agent: &Agent,
    job_id: &Uuid,
    task_id: &str,
    request: &ProveReq,
    max_retries: i32,
) -> Result<ProverOutcome> {
    let start_time = Instant::now();
    let index = request.index;
    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let segment_key = format!("{job_prefix}:{SEGMENTS_PATH}:{index}");

    tracing::debug!("Starting proof of idx: {job_id} - {index}");

    // Record Redis operation for segment retrieval
    let segment_vec: Vec<u8> = match redis::get_key(&mut conn, &segment_key).await {
        Ok(data) => data,
        Err(e) => {
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
        Ok(receipt) => receipt,
        Err(e) => return Err(e),
    };
    let prove_elapsed_time = prove_start.elapsed().as_secs_f64();
    helpers::record_task_operation("prove", "prove_segment", "success", prove_elapsed_time);

    segment_receipt
        .verify_integrity_with_context(&agent.verifier_ctx)
        .context("Failed to verify segment receipt integrity")?;

    helpers::record_task("prove", "prove_segment", "success", prove_elapsed_time);

    tracing::debug!("Completed proof: {job_id} - {index}");
    let segment_receipt_bytes = serialize_obj(&segment_receipt)
        .context("Failed to serialize segment receipt for background lift")?;
    drop(conn);

    // Background the lift, Redis write, and DB update so prover can move to next job
    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");
    let db_pool = agent.db_pool.clone();
    let redis_pool = agent.redis_pool.clone();
    let is_povw = agent.is_povw_enabled();
    let redis_ttl = agent.args.redis_ttl;
    let job_id_clone = *job_id;
    let task_id_string = task_id.to_owned();

    let max_segment_po2 = std::cmp::max(agent.segment_po2 as usize, DEFAULT_MAX_PO2);

    let background_ctx = OffloadContext {
        redis_pool,
        db_pool,
        job_id: job_id_clone,
        task_id: task_id_string,
        segment_key,
        output_key,
        redis_ttl,
        is_povw,
        max_retries,
        start_time,
        max_segment_po2,
    };

    tokio::spawn(async move {
        if let Err(err) = offload_lift_and_finalize(background_ctx, segment_receipt_bytes).await {
            tracing::error!("Background prove completion failed for job {job_id_clone}: {:#}", err);
        }
    });

    Ok(ProverOutcome::Deferred)
}

struct OffloadContext {
    redis_pool: RedisPool,
    db_pool: PgPool,
    job_id: Uuid,
    task_id: String,
    segment_key: String,
    output_key: String,
    redis_ttl: u64,
    is_povw: bool,
    max_retries: i32,
    start_time: Instant,
    max_segment_po2: usize,
}

async fn offload_lift_and_finalize(
    ctx: OffloadContext,
    segment_receipt_bytes: Vec<u8>,
) -> Result<()> {
    let OffloadContext {
        redis_pool,
        db_pool,
        job_id,
        task_id,
        segment_key,
        output_key,
        redis_ttl,
        is_povw,
        max_retries,
        start_time,
        max_segment_po2,
    } = ctx;

    let job_id_for_logs = job_id;
    let task_id_for_logs = task_id.clone();
    let db_pool_clone = db_pool.clone();

    let result = perform_lift_write_and_finalize(
        redis_pool,
        db_pool,
        job_id,
        task_id.clone(),
        segment_key,
        output_key,
        redis_ttl,
        is_povw,
        start_time,
        segment_receipt_bytes,
        max_segment_po2,
    )
    .await;

    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            helpers::record_task_operation(
                "prove",
                "complete",
                "error",
                start_time.elapsed().as_secs_f64(),
            );

            let err_msg = format!("{:#}", err);
            if let Err(db_err) = handle_prove_failure(
                db_pool_clone,
                job_id_for_logs,
                task_id,
                max_retries,
                err_msg.clone(),
            )
            .await
            {
                tracing::error!(
                    "Failed to record prove failure for {job_id_for_logs}:{task_id_for_logs}: {:#}",
                    db_err
                );
            }
            tracing::warn!(
                "Prove lift/write failed for {job_id_for_logs}:{task_id_for_logs}: {err_msg}"
            );
            Ok(())
        }
    }
}

async fn perform_lift_write_and_finalize(
    redis_pool: RedisPool,
    db_pool: PgPool,
    job_id: Uuid,
    task_id: String,
    segment_key: String,
    output_key: String,
    redis_ttl: u64,
    is_povw: bool,
    start_time: Instant,
    segment_receipt_bytes: Vec<u8>,
    max_segment_po2: usize,
) -> Result<()> {
    let lift_label = if is_povw { "lift_povw" } else { "lift" };
    let lift_start = Instant::now();
    let job_id_for_logs = job_id;
    let task_id_for_logs = task_id.clone();

    let lift_result = tokio::task::spawn_blocking(move || {
        run_lift(is_povw, segment_receipt_bytes, job_id_for_logs, task_id_for_logs, max_segment_po2)
    })
    .await
    .context("Background lift task panicked")?;

    let (_op_type, lift_asset) = match lift_result {
        Ok((label, asset)) => {
            helpers::record_task_operation(
                "prove",
                label,
                "success",
                lift_start.elapsed().as_secs_f64(),
            );
            tracing::debug!("lifting complete {job_id} - {task_id}");
            (label, asset)
        }
        Err(err) => {
            helpers::record_task_operation(
                "prove",
                lift_label,
                "error",
                lift_start.elapsed().as_secs_f64(),
            );
            return Err(err);
        }
    };

    let mut conn = redis_pool
        .get()
        .await
        .context("Failed to get Redis connection for background prove completion")?;
    redis::set_key_with_expiry(&mut conn, &output_key, lift_asset, Some(redis_ttl))
        .await
        .context("Failed to persist lifted receipt in Redis")?;
    redis::unlink_key(&mut conn, &segment_key)
        .await
        .context("Failed to delete segment key after lift")?;

    taskdb::update_task_done(&db_pool, &job_id, &task_id, Value::Null)
        .await
        .context("Failed to report prove task completion")?;

    helpers::record_task_operation(
        "prove",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );
    Ok(())
}

fn run_lift(
    is_povw: bool,
    segment_receipt_bytes: Vec<u8>,
    job_id: Uuid,
    task_id: String,
    max_segment_po2: usize,
) -> Result<(&'static str, Vec<u8>)> {
    let prover_opts = ProverOpts::from_max_po2(max_segment_po2);
    let prover = get_prover_server(&prover_opts)
        .context("Failed to create prover server for background lift")?;
    let verifier_ctx = VerifierContext::default();
    let segment_receipt: SegmentReceipt = deserialize_obj(&segment_receipt_bytes)
        .context("Failed to deserialize segment receipt in background lift")?;

    if is_povw {
        tracing::debug!("lifting {job_id} - {task_id}");
        let lift_receipt: SuccinctReceipt<WorkClaim<ReceiptClaim>> = prover
            .lift_povw(&segment_receipt)
            .context("Failed to lift POVW receipt in background")?;
        lift_receipt
            .verify_integrity_with_context(&verifier_ctx)
            .context("Failed to verify lifted POVW receipt integrity")?;
        let serialized =
            serialize_obj(&lift_receipt).context("Failed to serialize lifted POVW receipt")?;
        Ok(("lift_povw", serialized))
    } else {
        tracing::debug!("lifting {job_id} - {task_id}");
        let lift_receipt: SuccinctReceipt<ReceiptClaim> =
            prover.lift(&segment_receipt).context("Failed to lift receipt in background")?;
        lift_receipt
            .verify_integrity_with_context(&verifier_ctx)
            .context("Failed to verify lifted receipt integrity")?;
        let serialized =
            serialize_obj(&lift_receipt).context("Failed to serialize lifted receipt")?;
        Ok(("lift", serialized))
    }
}

async fn handle_prove_failure(
    db_pool: PgPool,
    job_id: Uuid,
    task_id: String,
    max_retries: i32,
    mut err_str: String,
) -> Result<()> {
    if err_str.is_empty() {
        err_str = "unknown prove failure".to_string();
    }
    err_str.truncate(1024);

    if max_retries > 0 {
        let current_retries = query_scalar::<_, i32>(
            "SELECT retries FROM tasks WHERE job_id = $1 AND task_id = $2 AND state = 'running'",
        )
        .bind(job_id)
        .bind(&task_id)
        .fetch_optional(&db_pool)
        .await
        .context("Failed to read current retries for prove task")?;

        if let Some(current) = current_retries {
            if current + 1 > max_retries {
                let final_err = format!("retry max hit: {err_str}");
                taskdb::update_task_failed(&db_pool, &job_id, &task_id, &final_err)
                    .await
                    .context("Failed to report prove failure")?;
                return Ok(());
            }
        }

        if !taskdb::update_task_retry(&db_pool, &job_id, &task_id)
            .await
            .context("Failed to update prove retries")?
        {
            tracing::info!("update_task_retry no-op for {job_id}:{task_id}");
        }
    } else {
        taskdb::update_task_failed(&db_pool, &job_id, &task_id, &err_str)
            .await
            .context("Failed to report prove failure without retries")?;
    }

    Ok(())
}
