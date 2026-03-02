// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Shared types for the Market Log-Builder guest between guest and host.
//!
//! The market log-builder binds a prover's work log ID to a specific market fulfillment,
//! enabling on-chain attribution of PoVW cycles to market service activity.

use alloy_primitives::Address;
use alloy_sol_types::sol;
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

#[cfg(feature = "build-guest")]
pub use crate::guest_artifacts::BOUNDLESS_MARKET_LOG_BUILDER_PATH;
pub use crate::guest_artifacts::{
    BOUNDLESS_MARKET_LOG_BUILDER_ELF, BOUNDLESS_MARKET_LOG_BUILDER_ID,
};

sol! {
    /// Journal output from the Market Log-Builder guest.
    ///
    /// Binds a prover's work log ID and cycle count to a specific application proof,
    /// enabling the assessor to embed PoVW attribution in the `RequestFulfilled` event.
    struct MarketLogBuilderJournal {
        /// The prover's PoVW work log ID (their proving address).
        address workLogId;
        /// Number of work units attributed to this fulfillment.
        uint64 updateValue;
        /// risc0 digest of the application proof's `ReceiptClaim`.
        ///
        /// Subsumes both the image ID and the journal, cryptographically binding this
        /// journal to a single specific execution.
        bytes32 claimDigest;
    }
}

/// Input to the Market Log-Builder guest.
///
/// The host must add the `WorkClaim<ReceiptClaim>` receipt (with control root `Digest::ZERO`)
/// as an assumption to the [`ExecutorEnv`] before running the market log-builder guest.
/// The claim digest — identifying the specific application execution — is derived from the
/// WorkClaim's embedded `ReceiptClaim` rather than being passed separately.
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct MarketLogBuilderInput {
    /// The prover's PoVW work log ID (their proving address).
    #[borsh(
        deserialize_with = "borsh_deserialize_address",
        serialize_with = "borsh_serialize_address"
    )]
    pub log_id: Address,
    /// Borsh-serialized `WorkClaim<ReceiptClaim>` for this specific application proof.
    ///
    /// Its `work.value` is the cycle count; its embedded `ReceiptClaim` identifies the exact
    /// application execution. The host must add this as a zkVM assumption (control root
    /// `Digest::ZERO`) to the `ExecutorEnv`.
    pub work_claim: Vec<u8>,
}

impl MarketLogBuilderInput {
    /// Serialize to bytes (borsh).
    pub fn encode(&self) -> anyhow::Result<Vec<u8>> {
        borsh::to_vec(self).map_err(Into::into)
    }

    /// Deserialize from bytes (borsh).
    pub fn decode(buffer: impl AsRef<[u8]>) -> anyhow::Result<Self> {
        borsh::from_slice(buffer.as_ref()).map_err(Into::into)
    }
}

fn borsh_deserialize_address(
    reader: &mut impl borsh::io::Read,
) -> Result<Address, borsh::io::Error> {
    use ruint::aliases::U160;
    Ok(<U160 as BorshDeserialize>::deserialize_reader(reader)?.into())
}

fn borsh_serialize_address(
    address: &Address,
    writer: &mut impl borsh::io::Write,
) -> Result<(), borsh::io::Error> {
    use ruint::aliases::U160;
    <U160 as BorshSerialize>::serialize(&(*address).into(), writer)?;
    Ok(())
}
