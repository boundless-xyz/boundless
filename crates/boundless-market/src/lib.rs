// Copyright 2025 RISC Zero, Inc.
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

#![cfg_attr(not(doctest), doc = include_str!("../README.md"))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![deny(missing_docs)]

/// Re-export of [alloy], provided to ensure that the correct version of the types used in the
/// public API are available in case multiple versions of [alloy] are in use.
///
/// Because [alloy] is a v0.x crate, it is not covered under the semver policy of this crate.
#[cfg(not(target_os = "zkvm"))]
pub use alloy;

#[cfg(not(target_os = "zkvm"))]
/// A ProviderLayer module.
///
/// It can be added to an alloy Provider to log warnings and errors
/// when the balance of a given address falls below certain thresholds.
pub mod balance_alerts_layer;
#[cfg(not(target_os = "zkvm"))]
/// Client module for interacting with the Boundless Market API.
pub mod client;
/// Contracts module for interacting with the Boundless Market smart contracts.
pub mod contracts;
#[cfg(not(target_os = "zkvm"))]
/// Input module for serializing input.
pub mod input;
#[cfg(not(target_os = "zkvm"))]
/// Order stream client module for submitting requests off-chain.
pub mod order_stream_client;
#[cfg(not(target_os = "zkvm"))]
/// Selector module implementing utility functions for supported selectors.
pub mod selector;
#[cfg(not(target_os = "zkvm"))]
/// Storage module for interacting with the storage provider.
pub mod storage;

/// A very small utility function to get the current unix timestamp.
// TODO(#379): Avoid drift relative to the chain's timestamps.
#[cfg(not(target_os = "zkvm"))]
pub(crate) fn now_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}
