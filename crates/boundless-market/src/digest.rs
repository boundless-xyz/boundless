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

use alloy::primitives::B256;
use serde::{Deserialize, Serialize};
use std::fmt;

/// A 32-byte hash digest representing an image ID or journal digest.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Digest([u8; 32]);

impl Digest {
    /// The zero digest.
    pub const ZERO: Self = Self([0u8; 32]);

    /// Creates a `Digest` from a byte array.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the digest as a byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Parses a hex-encoded digest (64 lowercase hex chars).
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(s, &mut bytes)?;
        Ok(Self(bytes))
    }

    /// Encodes the digest as a lowercase hex string.
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

impl From<[u8; 32]> for Digest {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<Digest> for [u8; 32] {
    fn from(d: Digest) -> Self {
        d.0
    }
}

impl From<B256> for Digest {
    fn from(b: B256) -> Self {
        Self(b.0)
    }
}

impl From<Digest> for B256 {
    fn from(d: Digest) -> Self {
        B256::from(d.0)
    }
}

impl TryFrom<&[u8]> for Digest {
    type Error = std::array::TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self(<[u8; 32]>::try_from(slice)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_bytes() {
        let bytes = [1u8; 32];
        let d = Digest::from_bytes(bytes);
        assert_eq!(<[u8; 32]>::from(d), bytes);
    }

    #[test]
    fn roundtrip_hex() {
        let d = Digest::from_bytes([0xab; 32]);
        let hex = d.to_hex();
        assert_eq!(Digest::from_hex(&hex).unwrap(), d);
    }

    #[test]
    fn display() {
        let d = Digest::from_bytes([0u8; 32]);
        assert_eq!(d.to_string(), "0".repeat(64));
    }

    #[test]
    fn b256_roundtrip() {
        let b = B256::repeat_byte(0x42);
        let d = Digest::from(b);
        assert_eq!(B256::from(d), b);
    }
}
