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

//! Claim-digest abstraction binding `(program_id, public_output)` to a
//! 32-byte commitment a verifier can check. Backend-specific formulas
//! live in their own crates.

use sha2::{Digest, Sha256};

/// Computes a 32-byte claim digest binding `(program_id, public_output)`.
///
/// Implementations are expected to match the formula expected by the
/// application verifier they are paired with.
pub trait ComputeClaimDigest: Send + Sync {
    /// Compute the claim digest from the full public output.
    fn compute(&self, program_id: &[u8; 32], public_output: &[u8]) -> [u8; 32];

    /// Compute the claim digest from a pre-hashed public-output digest.
    ///
    /// This is a required method because many verifier formulas distinguish
    /// "digest of output" from "output bytes". Implementations must match
    /// the verifier they are paired with.
    fn compute_from_output_digest(
        &self,
        program_id: &[u8; 32],
        output_digest: &[u8; 32],
    ) -> [u8; 32];
}

/// Default claim-digest formula:
///
/// ```text
/// sha256(program_id || sha256(public_output))
/// ```
///
/// SHA-256 is preferred over Keccak because the existing R0 assessor guest
/// has a SHA-256 accelerator. Backends without a strong reason to deviate
/// should use this formula.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sha256ClaimDigest;

impl ComputeClaimDigest for Sha256ClaimDigest {
    fn compute(&self, program_id: &[u8; 32], public_output: &[u8]) -> [u8; 32] {
        let output_digest: [u8; 32] = Sha256::digest(public_output).into();
        self.compute_from_output_digest(program_id, &output_digest)
    }

    fn compute_from_output_digest(
        &self,
        program_id: &[u8; 32],
        output_digest: &[u8; 32],
    ) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(program_id);
        hasher.update(output_digest);
        hasher.finalize().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_claim_digest_consistency() {
        let pid = [1u8; 32];
        let out = b"hello".as_slice();
        let direct = Sha256ClaimDigest.compute(&pid, out);
        let out_digest: [u8; 32] = Sha256::digest(out).into();
        let via = Sha256ClaimDigest.compute_from_output_digest(&pid, &out_digest);
        assert_eq!(direct, via);
    }
}
