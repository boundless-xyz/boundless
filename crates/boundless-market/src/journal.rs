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

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::Digest;

/// The output journal of a guest program execution.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Journal {
    /// The raw bytes written to the journal by the guest program.
    pub bytes: Vec<u8>,
}

impl Journal {
    /// Creates a new `Journal` from raw bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }

    /// Returns the SHA-256 digest of the journal bytes.
    pub fn digest(&self) -> Digest {
        Digest::from(<[u8; 32]>::from(Sha256::digest(&self.bytes)))
    }
}

impl From<Vec<u8>> for Journal {
    fn from(bytes: Vec<u8>) -> Self {
        Self { bytes }
    }
}

impl From<Journal> for Vec<u8> {
    fn from(j: Journal) -> Self {
        j.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_sha256_of_bytes() {
        let data = b"hello world";
        let journal = Journal::new(data.to_vec());
        let expected = Digest::from(<[u8; 32]>::from(Sha256::digest(data)));
        assert_eq!(journal.digest(), expected);
    }

    #[test]
    fn roundtrip_vec() {
        let bytes = vec![1u8, 2, 3];
        let j = Journal::new(bytes.clone());
        assert_eq!(Vec::from(j), bytes);
    }
}
