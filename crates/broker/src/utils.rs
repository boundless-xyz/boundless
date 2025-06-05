// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use alloy::primitives::aliases::U96;
use anyhow::{Context, Result};
use boundless_market::{
    contracts::ProofRequest,
    selector::{ProofType, SupportedSelectors},
};

use crate::{config::ConfigLock, OrderRequest};

/// Gas allocated to verifying a smart contract signature. Copied from BoundlessMarket.sol.
pub const ERC1271_MAX_GAS_FOR_CHECK: u64 = 100000;

/// Cancel a proof and mark the order as failed
///
/// This utility function combines the common pattern of canceling a stark proof
/// and marking the associated order as failed.
pub async fn cancel_proof_and_fail_order(
    prover: &crate::provers::ProverObj,
    db: &crate::db::DbObj,
    proof_id: &str,
    order_id: &str,
    failure_reason: &'static str,
) {
    tracing::debug!("Cancelling proof {} for order {}", proof_id, order_id);
    if let Err(err) = prover.cancel_stark(proof_id).await {
        tracing::warn!(
            "[B-UTL-001] Failed to cancel proof {proof_id} with reason: {failure_reason} for order {order_id}: {err}",
        );
    }

    // TODO in the case of a failure to cancel, the estimated capacity will be incorrect. Still
    // setting the order as failed to avoid infinite loops of cancellations.
    if let Err(err) = db.set_order_failure(order_id, failure_reason).await {
        tracing::error!(
            "Failed to set order {order_id} as failed for reason {failure_reason}: {err}",
        );
    }
}

/// Estimate of gas for locking a single order
/// Currently just uses the config estimate but this may change in the future
pub async fn estimate_gas_to_lock(config: &ConfigLock, order: &OrderRequest) -> Result<u64> {
    let mut estimate =
        config.lock_all().context("Failed to read config")?.market.lockin_gas_estimate;

    if order.request.is_smart_contract_signed() {
        estimate += ERC1271_MAX_GAS_FOR_CHECK;
    }

    Ok(estimate)
}

/// Estimate of gas for to fulfill a single order
/// Currently just uses the config estimate but this may change in the future
pub async fn estimate_gas_to_fulfill(
    config: &ConfigLock,
    supported_selectors: &SupportedSelectors,
    request: &ProofRequest,
) -> Result<u64> {
    // TODO: Add gas costs for orders with large journals.
    let (base, groth16) = {
        let config = config.lock_all().context("Failed to read config")?;
        (config.market.fulfill_gas_estimate, config.market.groth16_verify_gas_estimate)
    };

    let mut estimate = base;

    // Add gas for orders that make use of the callbacks feature.
    estimate += u64::try_from(
        request
            .requirements
            .callback
            .as_option()
            .map(|callback| callback.gasLimit)
            .unwrap_or(U96::ZERO),
    )?;

    estimate += match supported_selectors
        .proof_type(request.requirements.selector)
        .context("unsupported selector")?
    {
        ProofType::Any | ProofType::Inclusion => 0,
        ProofType::Groth16 => groth16,
        proof_type => {
            tracing::warn!("Unknown proof type in gas cost estimation: {proof_type:?}");
            0
        }
    };

    Ok(estimate)
}
