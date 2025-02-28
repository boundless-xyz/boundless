use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, KECCAK_RECEIPT_PATH},
    Agent,
};
use anyhow::{Context, Result};
use uuid::Uuid;
use workflow_common::UnionReq;
/// Run the union operation
pub async fn union(agent: &Agent, job_id: &Uuid, request: &UnionReq) -> Result<()> {
    tracing::info!("Starting union for job_id: {job_id}");

    // setup redis client
    let mut conn = redis::get_connection(&agent.redis_pool).await?;

    // setup redis keys
    let job_prefix = format!("job:{job_id}");
    let keccak_receipts_prefix = format!("{job_prefix}:{KECCAK_RECEIPT_PATH}");
    let left_receipt_key = format!("{keccak_receipts_prefix}:{0}", request.left);
    let right_receipt_key = format!("{keccak_receipts_prefix}:{0}", request.right);
    // get assets from redis
    let left_receipt_bytes: Vec<u8> = conn.get(&left_receipt_key).await.with_context(|| {
        format!("segment data not found for left receipt key: {left_receipt_key}")
    })?;

    let left_receipt: risc0_zkvm::SuccinctReceipt<risc0_zkvm::Unknown> =
        deserialize_obj(&left_receipt_bytes).context("Failed to deserialize left receipt")?;

    let right_receipt_bytes: Vec<u8> = conn.get(&right_receipt_key).await.with_context(|| {
        format!("segment data not found for right receipt key: {right_receipt_key}")
    })?;
    let right_receipt: risc0_zkvm::SuccinctReceipt<risc0_zkvm::Unknown> =
        deserialize_obj(&right_receipt_bytes).context("Failed to deserialize right receipt")?;

    // run union
    tracing::debug!("Union {job_id} - {} + {} -> {}", request.left, request.right, request.idx);

    let unioned = agent
        .prover
        .as_ref()
        .context("Failed to construct r0vm client in prover")?
        .union(&left_receipt, &right_receipt)
        .context("Failed to union on left/right receipt")?
        .into_unknown();

    // send result to redis
    let union_result = serialize_obj(&unioned).context("Failed to serialize union receipt")?;
    let output_key = format!("{keccak_receipts_prefix}:{}", request.idx);
    redis::set_key_with_expiry(&mut conn, &output_key, union_result, Some(agent.args.redis_ttl))
        .await
        .context("Failed to set redis key for union receipt")?;

    tracing::info!("Union complete {job_id} - {}", request.left);

    Ok(())
}
