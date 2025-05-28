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
