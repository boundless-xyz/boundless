// Copyright 2024 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::{
    redis::{self, AsyncCommands},
    tasks::{deserialize_obj, serialize_obj, RECEIPT_PATH, RECUR_RECEIPT_PATH},
    Agent,
};
use anyhow::{bail, Context, Result};
use risc0_zkvm::{InnerReceipt, Receipt, ReceiptClaim, SuccinctReceipt};
use uuid::Uuid;
use workflow_common::ResolveReq;

/// Run the resolve operation
pub async fn resolver(agent: &Agent, job_id: &Uuid, request: &ResolveReq) -> Result<Option<u64>> {
    let max_idx = &request.max_idx;
    let job_prefix = format!("job:{job_id}");
    let receipts_key = format!("{job_prefix}:{RECEIPT_PATH}");
    let root_receipt_key = format!("{job_prefix}:{RECUR_RECEIPT_PATH}:{max_idx}");

    let mut conn = redis::get_connection(&agent.redis_pool).await?;
    let receipt: Vec<u8> = conn.get::<_, Vec<u8>>(&root_receipt_key).await.with_context(|| {
        format!("segment data not found for root receipt key: {root_receipt_key}")
    })?;

    let mut conditional_receipt: SuccinctReceipt<ReceiptClaim> = deserialize_obj(&receipt)?;

    let mut assumptions_len: Option<u64> = None;
    if conditional_receipt.claim.clone().as_value()?.output.is_some() {
        if let Some(guest_output) =
            conditional_receipt.claim.clone().as_value()?.output.as_value()?
        {
            if !guest_output.assumptions.is_empty() {
                let assumptions = guest_output.assumptions.as_value()?.iter();
                tracing::info!("Resolving {} assumption(s)", assumptions.len());
                assumptions_len = Some(assumptions.len().try_into()?);
                for assumption in assumptions {
                    let assumption_claim = assumption.as_value()?.claim.to_string();
                    let assumption_key = format!("{receipts_key}:{assumption_claim}");
                    let assumption_bytes: Vec<u8> =
                        conn.get::<_, Vec<u8>>(&assumption_key)
                            .await
                            .context("corroborating receipt not found: key {assumption_key}")?;
                    let assumption_receipt: Receipt = deserialize_obj(&assumption_bytes)
                        .context("could not deserialize assumption receipt: {assumption_key}")?;
                    let assumption_receipt = match assumption_receipt.inner {
                        InnerReceipt::Succinct(res) => res.into_unknown(),
                        _ => {
                            bail!("Invalid assumption receipt type: not Succinct");
                        }
                    };

                    // Resolve
                    conditional_receipt = agent
                        .prover
                        .as_ref()
                        .context("Missing prover from resolve task")?
                        .resolve(&conditional_receipt, &assumption_receipt)?;
                }
                tracing::info!("Resolve complete");
            }
        }
    }

    // Write out the resolved receipt
    let serialized_asset =
        serialize_obj(&conditional_receipt).expect("Failed to serialize the asset");

    redis::set_key_with_expiry(
        &mut conn,
        &root_receipt_key,
        serialized_asset,
        Some(agent.args.redis_ttl),
    )
    .await
    .expect("Failed to set root receipt key with expiry");

    Ok(assumptions_len)
}
