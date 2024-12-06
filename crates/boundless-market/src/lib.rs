// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![cfg_attr(not(doctest), doc = include_str!("../README.md"))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

/// Re-export of [alloy], provided to ensure that the correct version of the types used in the
/// public API are available in case multiple versions of [alloy] are in use.
///
/// Because [alloy] is a v0.x crate, it is not covered under the semver policy of this crate.
#[cfg(not(target_os = "zkvm"))]
pub use alloy;

#[cfg(not(target_os = "zkvm"))]
pub mod client;
pub mod contracts;
#[cfg(not(target_os = "zkvm"))]
pub mod input;
#[cfg(not(target_os = "zkvm"))]
pub mod order_stream_client;
#[cfg(not(target_os = "zkvm"))]
pub mod storage;
