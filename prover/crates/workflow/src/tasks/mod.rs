// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use anyhow::{Context, Result};
use hex::FromHex;
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

pub(crate) mod executor;
pub(crate) mod finalize;
pub(crate) mod join;
pub(crate) mod join_povw;
pub(crate) mod keccak;
pub(crate) mod prove;
pub(crate) mod resolve;
pub(crate) mod resolve_povw;
pub(crate) mod snark;
pub(crate) mod union;

/// Recursion receipts key prefix
pub(crate) const RECUR_RECEIPT_PATH: &str = "recursion_receipts";

/// Resolved receipt key prefix — written by resolve/resolve_povw, read by finalize.
pub(crate) const RESOLVED_RECEIPT_PATH: &str = "resolved_receipt";

/// Segments key prefix for redis
pub(crate) const SEGMENTS_PATH: &str = "segments";

/// Receipts key prefix for redis
pub(crate) const RECEIPT_PATH: &str = "receipts";

/// Coprocessor callback prefix for redis
pub(crate) const COPROC_CB_PATH: &str = "coproc";

/// Keys to clean up from Redis after a task is marked done in the DB.
/// Returned by task functions so cleanup happens only after successful completion.
#[must_use]
pub(crate) struct CleanupKeys(pub Vec<String>);

impl CleanupKeys {
    pub(crate) fn none() -> Self {
        Self(Vec::new())
    }

    pub(crate) fn one(key: String) -> Self {
        Self(vec![key])
    }
}

/// Reads the [`IMAGE_ID_FILE`] and returns a [Digest]
pub(crate) fn read_image_id(image_id: &str) -> Result<Digest> {
    Digest::from_hex(image_id).context("Failed to convert imageId file to digest from_hex")
}

/// Serializes an object into a Vec<u8> using bincode.
pub(crate) fn serialize_obj<T: Serialize>(item: &T) -> Result<Vec<u8>> {
    bincode::serialize(item).map_err(anyhow::Error::new)
}

/// Deserializes a an encoded function
pub(crate) fn deserialize_obj<T: for<'de> Deserialize<'de>>(encoded: &[u8]) -> Result<T> {
    let decoded = bincode::deserialize(encoded)?;
    Ok(decoded)
}
