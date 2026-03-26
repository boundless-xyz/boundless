// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use crate::{
    Agent,
    tasks::{RECUR_RECEIPT_PATH, SEGMENTS_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{
    ProverOpts, ReceiptClaim, SegmentReceipt, SuccinctReceipt, VerifierContext, WorkClaim,
    get_prover_server,
};
use std::time::Instant;
use uuid::Uuid;
use workflow_common::{ProvePairReq, metrics::helpers};

/// Run two operations and attach context to each error; returns `(left, right)` or the first error.
fn both_with_context<L, R, E>(
    left: Result<L, E>,
    right: Result<R, E>,
    left_ctx: impl AsRef<str>,
    right_ctx: impl AsRef<str>,
) -> Result<(L, R)>
where
    E: Into<anyhow::Error>,
{
    let left = left.map_err(Into::into).context(left_ctx.as_ref().to_string())?;
    let right = right.map_err(Into::into).context(right_ctx.as_ref().to_string())?;
    Ok((left, right))
}

fn lift_receipt_in_worker(
    receipt: SegmentReceipt,
    prover_init_ctx: &'static str,
    segment_verify_ctx: &'static str,
    lift_ctx: &'static str,
    lift_verify_ctx: &'static str,
) -> Result<SuccinctReceipt<ReceiptClaim>> {
    let verifier_ctx = VerifierContext::default();

    receipt.verify_integrity_with_context(&verifier_ctx).context(segment_verify_ctx)?;

    // The shared Agent prover is an Rc, so each blocking worker constructs its own local prover.
    let prover = get_prover_server(&ProverOpts::default()).context(prover_init_ctx)?;
    let lifted = prover.lift(&receipt).context(lift_ctx)?;
    lifted.verify_integrity_with_context(&verifier_ctx).context(lift_verify_ctx)?;

    Ok(lifted)
}

fn lift_povw_receipt_in_worker(
    receipt: SegmentReceipt,
    prover_init_ctx: &'static str,
    segment_verify_ctx: &'static str,
    lift_ctx: &'static str,
    lift_verify_ctx: &'static str,
) -> Result<SuccinctReceipt<WorkClaim<ReceiptClaim>>> {
    let verifier_ctx = VerifierContext::default();

    receipt.verify_integrity_with_context(&verifier_ctx).context(segment_verify_ctx)?;

    // The shared Agent prover is an Rc, so each blocking worker constructs its own local prover.
    let prover = get_prover_server(&ProverOpts::default()).context(prover_init_ctx)?;
    let lifted = prover.lift_povw(&receipt).context(lift_ctx)?;
    lifted.verify_integrity_with_context(&verifier_ctx).context(lift_verify_ctx)?;

    Ok(lifted)
}

/// Run prove+lift+join for two adjoining segments in one task.
pub async fn prover_pair(
    agent: &Agent,
    job_id: &Uuid,
    task_id: &str,
    request: &ProvePairReq,
) -> Result<()> {
    let start_time = Instant::now();
    let job_prefix = format!("job:{job_id}");
    let left_key = format!("{job_prefix}:{SEGMENTS_PATH}:{}", request.left);
    let right_key = format!("{job_prefix}:{SEGMENTS_PATH}:{}", request.right);

    tracing::debug!(
        "Starting prove-pair {job_id} segments {} + {} -> {task_id}",
        request.left,
        request.right
    );

    let (segment_vec_left, segment_vec_right) =
        tokio::join!(agent.hot_get_bytes(&left_key), agent.hot_get_bytes(&right_key),);
    let segment_vec_left =
        segment_vec_left.context(format!("segment data not found for key: {left_key}"))?;
    let segment_vec_right =
        segment_vec_right.context(format!("segment data not found for key: {right_key}"))?;

    let (segment_left_res, segment_right_res) = tokio::join!(
        tokio::task::spawn_blocking(move || deserialize_obj(&segment_vec_left)),
        tokio::task::spawn_blocking(move || deserialize_obj(&segment_vec_right)),
    );
    let segment_left = segment_left_res
        .context("left deserialize task")?
        .context("Failed to deserialize left segment")?;
    let segment_right = segment_right_res
        .context("right deserialize task")?
        .context("Failed to deserialize right segment")?;

    let prover = agent.prover.as_ref().context("[BENTO-PROVEPAIR-001] Missing prover")?;

    // Prove both segments
    let prove_start = Instant::now();
    let (receipt_left, receipt_right) = both_with_context(
        prover.prove_segment(&agent.verifier_ctx, &segment_left),
        prover.prove_segment(&agent.verifier_ctx, &segment_right),
        "[BENTO-PROVEPAIR-002] Left segment prove failed",
        "[BENTO-PROVEPAIR-003] Right segment prove failed",
    )?;
    helpers::record_task_operation(
        "prove_pair",
        "prove_segments",
        "success",
        prove_start.elapsed().as_secs_f64(),
    );
    agent.start_work_prefetch();

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

    if agent.is_povw_enabled() {
        let lift_start = Instant::now();
        let (lift_left_res, lift_right_res) = tokio::join!(
            tokio::task::spawn_blocking(move || {
                lift_povw_receipt_in_worker(
                    receipt_left,
                    "[BENTO-PROVEPAIR-018] Left post-process prover init failed",
                    "[BENTO-PROVEPAIR-004] Left segment receipt verify failed",
                    "[BENTO-PROVEPAIR-006] Left lift_povw failed",
                    "[BENTO-PROVEPAIR-008] Left lift verify failed",
                )
            }),
            tokio::task::spawn_blocking(move || {
                lift_povw_receipt_in_worker(
                    receipt_right,
                    "[BENTO-PROVEPAIR-019] Right post-process prover init failed",
                    "[BENTO-PROVEPAIR-005] Right segment receipt verify failed",
                    "[BENTO-PROVEPAIR-007] Right lift_povw failed",
                    "[BENTO-PROVEPAIR-009] Right lift verify failed",
                )
            }),
        );
        let lift_left =
            lift_left_res.context("[BENTO-PROVEPAIR-020] Left post-process worker failed")??;
        let lift_right =
            lift_right_res.context("[BENTO-PROVEPAIR-021] Right post-process worker failed")??;
        helpers::record_task_operation(
            "prove_pair",
            "lift_povw",
            "success",
            lift_start.elapsed().as_secs_f64(),
        );

        let join_start = Instant::now();
        let joined = prover
            .join_povw(&lift_left, &lift_right)
            .context("[BENTO-PROVEPAIR-010] join_povw failed")?;
        helpers::record_task_operation(
            "prove_pair",
            "join_povw",
            "success",
            join_start.elapsed().as_secs_f64(),
        );

        joined
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVEPAIR-011] Joined receipt verify failed")?;

        let out_bytes =
            serialize_obj(&joined).context("Failed to serialize joined POVW receipt")?;
        agent
            .hot_set_bytes(&output_key, out_bytes)
            .await
            .context("Failed to write joined receipt")?;
    } else {
        let lift_start = Instant::now();
        let (lift_left_res, lift_right_res) = tokio::join!(
            tokio::task::spawn_blocking(move || {
                lift_receipt_in_worker(
                    receipt_left,
                    "[BENTO-PROVEPAIR-018] Left post-process prover init failed",
                    "[BENTO-PROVEPAIR-004] Left segment receipt verify failed",
                    "[BENTO-PROVEPAIR-012] Left lift failed",
                    "[BENTO-PROVEPAIR-014] Left lift verify failed",
                )
            }),
            tokio::task::spawn_blocking(move || {
                lift_receipt_in_worker(
                    receipt_right,
                    "[BENTO-PROVEPAIR-019] Right post-process prover init failed",
                    "[BENTO-PROVEPAIR-005] Right segment receipt verify failed",
                    "[BENTO-PROVEPAIR-013] Right lift failed",
                    "[BENTO-PROVEPAIR-015] Right lift verify failed",
                )
            }),
        );
        let lift_left =
            lift_left_res.context("[BENTO-PROVEPAIR-020] Left post-process worker failed")??;
        let lift_right =
            lift_right_res.context("[BENTO-PROVEPAIR-021] Right post-process worker failed")??;
        helpers::record_task_operation(
            "prove_pair",
            "lift",
            "success",
            lift_start.elapsed().as_secs_f64(),
        );

        let join_start = Instant::now();
        let joined =
            prover.join(&lift_left, &lift_right).context("[BENTO-PROVEPAIR-016] join failed")?;
        helpers::record_task_operation(
            "prove_pair",
            "join",
            "success",
            join_start.elapsed().as_secs_f64(),
        );

        joined
            .verify_integrity_with_context(&agent.verifier_ctx)
            .context("[BENTO-PROVEPAIR-017] Joined receipt verify failed")?;

        let out_bytes = serialize_obj(&joined).context("Failed to serialize joined receipt")?;
        agent
            .hot_set_bytes(&output_key, out_bytes)
            .await
            .context("Failed to write joined receipt")?;
    }

    // Clean up both segments
    let cleanup_start = Instant::now();
    let d_left = agent.hot_delete(&left_key).await;
    let d_right = agent.hot_delete(&right_key).await;
    let cleanup_status = if d_left.is_ok() && d_right.is_ok() { "success" } else { "error" };
    helpers::record_redis_operation(
        "unlink",
        cleanup_status,
        cleanup_start.elapsed().as_secs_f64(),
    );
    d_left.context("Failed to delete left segment key")?;
    d_right.context("Failed to delete right segment key")?;

    helpers::record_task_operation(
        "prove_pair",
        "complete",
        "success",
        start_time.elapsed().as_secs_f64(),
    );

    tracing::debug!(
        "Prove-pair complete {job_id} {} + {} -> {task_id}",
        request.left,
        request.right
    );

    Ok(())
}
