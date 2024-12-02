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

use anyhow::{Context, Result};
use hex::FromHex;
use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

pub(crate) mod executor;
pub(crate) mod finalize;
pub(crate) mod join;
pub(crate) mod prove;
pub(crate) mod resolve;
pub(crate) mod snark;

/// Recursion receipts key prefix
pub(crate) const RECUR_RECEIPT_PATH: &str = "recursion_receipts";

/// Segments key prefix for redis
pub(crate) const SEGMENTS_PATH: &str = "segments";

/// Receipts key prefix for redis
pub(crate) const RECEIPT_PATH: &str = "receipts";

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
