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

//! Generic program-identity and public-output types used at the market boundary.
//!
//! These exist so the SDK can route proofs by `(program_id, public_output)`
//! without depending on any specific zkVM's native receipt or journal types.

use alloy_primitives::{Bytes, FixedBytes};
use serde::{Deserialize, Serialize};

/// Canonical 32-byte program identity at the market boundary.
///
/// For RISC Zero this is the image id. Other backends provide the canonical
/// 32-byte projection of their native program-identity construction.
///
/// Inner field is private; construct via [`Self::new`] / [`From`] and read
/// via [`Self::as_bytes`] / [`Self::as_fixed_bytes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramId(FixedBytes<32>);

impl ProgramId {
    /// Construct from raw 32 bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(FixedBytes(bytes))
    }

    /// Borrow the underlying 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0 .0
    }

    /// Borrow the underlying [`FixedBytes<32>`].
    pub const fn as_fixed_bytes(&self) -> &FixedBytes<32> {
        &self.0
    }
}

impl From<[u8; 32]> for ProgramId {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

impl From<FixedBytes<32>> for ProgramId {
    fn from(value: FixedBytes<32>) -> Self {
        Self(value)
    }
}

/// Public output produced by a zkVM execution (e.g. R0 journal).
///
/// Treated as opaque bytes at the generic market boundary; backends interpret
/// the contents per their own conventions.
///
/// Inner field is private; construct via [`Self::new`] / [`From`] and read
/// via [`Self::as_bytes`] / [`Self::as_alloy_bytes`].
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PublicOutput(Bytes);

impl PublicOutput {
    /// Construct from raw bytes.
    pub fn new(bytes: impl Into<Bytes>) -> Self {
        Self(bytes.into())
    }

    /// Borrow the underlying bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Borrow the underlying [`Bytes`].
    pub fn as_alloy_bytes(&self) -> &Bytes {
        &self.0
    }
}

impl From<Bytes> for PublicOutput {
    fn from(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

impl From<Vec<u8>> for PublicOutput {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes.into())
    }
}
