use crate::{
    Agent,
    redis::{self, AsyncCommands},
    tasks::{RECUR_RECEIPT_PATH, SEGMENT_RECEIPT_PATH, deserialize_obj, serialize_obj},
};
use anyhow::{Context, Result};
use risc0_zkvm::{ReceiptClaim, SuccinctReceipt, WorkClaim};
use uuid::Uuid;
use workflow_common::LiftReq;

/// Run a prove request
pub async fn lifter(agent: &Agent, job_id: &Uuid, task_id: &str, request: &LiftReq) -> Result<()> {
    let index = request.index;
    tracing::debug!("Starting lifting {job_id} - {index}");

    let mut conn = agent.redis_pool.get().await?;
    let job_prefix = format!("job:{job_id}");
    let receipt_key = format!("{job_prefix}:{SEGMENT_RECEIPT_PATH}:{index}");

    let segment_vec: Vec<u8> = conn
        .get::<_, Vec<u8>>(&receipt_key)
        .await
        .with_context(|| format!("receipt data not found for receipt key: {receipt_key}"))?;
    let segment_receipt =
        deserialize_obj(&segment_vec).context("Failed to deserialize segment data from redis")?;

    let output_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{task_id}");

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
    conn.unlink::<_, ()>(&receipt_key).await.context("Failed to delete receipt key")?;

    Ok(())
}
