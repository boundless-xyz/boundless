// Copyright 2026 Boundless Foundation, Inc.
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

use alloy::{network::Ethereum, primitives::aliases::U96, providers::Provider};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{PredicateType, ProofRequest},
    prover_utils::{estimate_erc1271_gas, Erc1271GasCache},
    selector::{ProofType, SupportedSelectors},
};
use std::sync::Arc;
use std::time::SystemTime;

use risc0_zkvm::{
    sha::{Impl as ShaImpl, Sha256},
    MaybePruned, ReceiptClaim,
};

use crate::{config::ConfigLock, OrderRequest};

/// A very small utility function to get the current unix timestamp in seconds.
// TODO(#379): Avoid drift relative to the chain's timestamps.
pub(crate) fn now_timestamp() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

/// Format the lock and final expiries of a request in a human-readable form
/// (used by `Order::Display`).
pub(crate) fn format_expiries(request: &ProofRequest) -> String {
    let now: i64 = now_timestamp().try_into().unwrap();
    let lock_expires_at: i64 = request.lock_expires_at().try_into().unwrap();
    let lock_expires_delta = lock_expires_at - now;
    let lock_expires_delta_str = if lock_expires_delta > 0 {
        format!("{lock_expires_delta} seconds from now")
    } else {
        format!("{} seconds ago", lock_expires_delta.abs())
    };
    let expires_at: i64 = request.expires_at().try_into().unwrap();
    let expires_at_delta = expires_at - now;
    let expires_at_delta_str = if expires_at_delta > 0 {
        format!("{expires_at_delta} seconds from now")
    } else {
        format!("{} seconds ago", expires_at_delta.abs())
    };
    format!(
        "lock expires at {lock_expires_at} ({lock_expires_delta_str}), expires at {expires_at} ({expires_at_delta_str})"
    )
}

/// Returns `true` if the dev mode environment variable is enabled.
pub(crate) fn is_dev_mode() -> bool {
    std::env::var("RISC0_DEV_MODE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

/// Replace the journal bytes in a [`ReceiptClaim`] with their digest to keep claims compact while
/// preserving the existing `ReceiptClaim::digest` output.
pub fn prune_receipt_claim_journal(mut claim: ReceiptClaim) -> ReceiptClaim {
    if let MaybePruned::Value(Some(output)) = &mut claim.output {
        let digest = match &output.journal {
            MaybePruned::Value(bytes) => Some(*ShaImpl::hash_bytes(bytes)),
            MaybePruned::Pruned(_) => None,
        };

        if let Some(digest) = digest {
            output.journal = MaybePruned::Pruned(digest);
        }
    }

    claim
}

/// Estimate of gas for locking a single order.
///
/// For smart-contract-signed requests, calls `eth_estimateGas` on the ERC-1271
/// `isValidSignature` method to get a tighter estimate than the worst-case constant.
/// Falls back to the contract-defined maximum if the address has no code or the RPC call fails.
pub(crate) async fn estimate_gas_to_lock<P: Provider<Ethereum> + 'static>(
    config: &ConfigLock,
    order: &OrderRequest,
    provider: &Arc<P>,
    cache: &Erc1271GasCache,
) -> Result<u64> {
    let lockin_gas = config.lock_all().context("Failed to read config")?.market.lockin_gas_estimate;
    let erc1271_gas = estimate_erc1271_gas(order, provider, cache).await;
    Ok(lockin_gas + erc1271_gas)
}

/// Estimate of gas for to fulfill a single order
/// Currently just uses the config estimate but this may change in the future
pub(crate) async fn estimate_gas_to_fulfill(
    config: &ConfigLock,
    supported_selectors: &SupportedSelectors,
    request: &ProofRequest,
    journal_bytes: Option<usize>,
) -> Result<u64> {
    let (base, groth16, journal_gas_per_byte, max_journal_bytes) = {
        let config = config.lock_all().context("Failed to read config")?;
        (
            config.market.fulfill_gas_estimate,
            config.market.groth16_verify_gas_estimate,
            config.market.fulfill_journal_gas_per_byte,
            config.market.max_journal_bytes,
        )
    };

    let journal_size = journal_bytes.unwrap_or(max_journal_bytes);

    let mut estimate = base;

    // Add gas for journal calldata when the predicate requires journal submission.
    if request.requirements.predicate.predicateType != PredicateType::ClaimDigestMatch {
        estimate += journal_size as u64 * journal_gas_per_byte;
    }

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
        ProofType::Groth16 | ProofType::Blake3Groth16 => groth16,
        proof_type => {
            tracing::warn!("Unknown proof type in gas cost estimation: {proof_type:?}");
            0
        }
    };

    Ok(estimate)
}
