// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! RISC Zero implementation of
//! [`boundless_market::backend_provider::BackendProvider`].
//!
//! Wraps a broker `Prover` (Bonsai/Bento) and provides per-selector
//! Groth16 / Blake3-Groth16 composition + on-chain seal encoding.

#![deny(missing_docs)]

#[cfg(feature = "risc0")]
pub mod risc_zero;
#[cfg(feature = "risc0")]
pub use risc_zero::{RiscZeroBackend, RiscZeroClaimDigest};

pub use boundless_market::backend_provider::{
    BackendProvider, BackendProviderError, BackendProviderObj,
};
