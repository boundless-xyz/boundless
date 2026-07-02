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

use alloy::{network::Ethereum, providers::Provider};
use anyhow::{Context, Result};
use boundless_market::{
    contracts::{PredicateType, ProofRequest},
    prover_utils::{callback_gas, estimate_erc1271_gas, Erc1271GasCache},
};
use std::sync::Arc;
use std::time::SystemTime;

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

/// Total on-chain gas to fulfill a single order (excluding lock gas): the path-aware `base`
/// (verifier + assessor + settlement, resolved by the caller via the backend's `fulfillment_gas`)
/// plus journal calldata and callback gas, both added here selector-agnostically.
pub(crate) fn fulfill_gas_units(
    base: u64,
    config: &ConfigLock,
    request: &ProofRequest,
    journal_bytes: Option<usize>,
) -> Result<u64> {
    let (journal_gas_per_byte, max_journal_bytes) = {
        let config = config.lock_all().context("Failed to read config")?;
        (config.market.fulfill_journal_gas_per_byte, config.market.max_journal_bytes)
    };

    // Add gas for journal calldata when the predicate requires journal submission.
    let journal_gas =
        if request.requirements.predicate.predicateType != PredicateType::ClaimDigestMatch {
            journal_bytes.unwrap_or(max_journal_bytes) as u64 * journal_gas_per_byte
        } else {
            0
        };

    // Callback gas is universal across fulfillment paths and backends, so it is priced here on
    // top of the backend-resolved base rather than inside the backend.
    Ok(base + journal_gas + callback_gas(request)?)
}
