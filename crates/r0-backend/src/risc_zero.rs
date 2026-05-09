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

//! RISC Zero `BackendProvider` impl wrapping a broker `Prover`
//! (Bonsai/Bento). Handles Groth16 / Blake3-Groth16 STARK→SNARK compression
//! and on-chain seal encoding. Set-inclusion / pass-through selectors
//! return [`BackendProviderError::UnsupportedSelector`].

use anyhow::anyhow;
use async_trait::async_trait;
use blake3_groth16::{Blake3Groth16Receipt, Blake3Groth16ReceiptClaim};
use boundless_market::{
    backend_provider::{BackendProvider, BackendProviderError},
    prover_utils::prover::ProverObj,
    selector::{is_blake3_groth16_selector, is_groth16_selector, ProofType, SelectorExt},
    ComputeClaimDigest, ProgramId, PublicOutput,
};
use risc0_ethereum_contracts::encode_seal;
use risc0_zkvm::{
    sha::{Digest as R0Digest, Digestible},
    MaybePruned, Receipt, ReceiptClaim,
};

/// RISC Zero implementation of [`BackendProvider`].
///
/// Two-prover model matches the broker:
/// - `prover` runs preflight + STARK proving (typically Bento).
/// - `snark_prover` runs Groth16 / Blake3-Groth16 compression (typically
///   Bonsai). When the broker only has one prover, the same handle is used
///   for both.
#[derive(Clone)]
pub struct RiscZeroBackend {
    prover: ProverObj,
    snark_prover: ProverObj,
    dev_mode: bool,
}

impl std::fmt::Debug for RiscZeroBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RiscZeroBackend").field("dev_mode", &self.dev_mode).finish()
    }
}

impl RiscZeroBackend {
    /// Construct an R0 provider wrapping the given prover handles.
    ///
    /// Pass the same `Arc` for `prover` and `snark_prover` if the deployment
    /// uses a single prover for everything.
    pub fn new(prover: ProverObj, snark_prover: ProverObj, dev_mode: bool) -> Self {
        Self { prover, snark_prover, dev_mode }
    }

    /// Convenience: same prover for STARK and SNARK paths.
    pub fn with_single_prover(prover: ProverObj, dev_mode: bool) -> Self {
        Self::new(prover.clone(), prover, dev_mode)
    }

    /// Reference to the underlying STARK prover. Useful when callers need
    /// access to broker-`Prover` semantics that aren't part of the
    /// [`BackendProvider`] surface (e.g. cancellation).
    pub fn prover(&self) -> &ProverObj {
        &self.prover
    }

    /// Reference to the SNARK prover (Groth16 / Blake3 compression backend).
    pub fn snark_prover(&self) -> &ProverObj {
        &self.snark_prover
    }
}

impl RiscZeroBackend {
    /// Map a selector to the [`ProofType`] this backend dispatches on.
    /// Unknown / unspecified selectors collapse to [`ProofType::Any`]; the
    /// caller turns those into [`BackendProviderError::UnsupportedSelector`].
    fn proof_type(selector: boundless_market::VerifierSelector) -> ProofType {
        if is_groth16_selector(selector) {
            ProofType::Groth16
        } else if is_blake3_groth16_selector(selector)
            || matches!(SelectorExt::from_bytes(selector.0), Some(SelectorExt::FakeBlake3Groth16))
        {
            ProofType::Blake3Groth16
        } else {
            ProofType::Any
        }
    }

    /// Compress the primitive STARK to a Groth16 SNARK and locally verify.
    /// Returns the backend's id of the compressed receipt.
    async fn compress_groth16(
        &self,
        primitive_proof_id: &str,
    ) -> Result<String, BackendProviderError> {
        let snark_id = self
            .snark_prover
            .compress(primitive_proof_id)
            .await
            .map_err(|e| anyhow!("groth16 compress: {e}"))?;
        // Fetch + verify integrity locally so we don't persist a broken id.
        let receipt_bytes = self
            .snark_prover
            .get_compressed_receipt(&snark_id)
            .await
            .map_err(|e| anyhow!("get groth16 receipt: {e}"))?
            .ok_or_else(|| anyhow!("groth16 receipt missing"))?;
        let receipt: Receipt = bincode::deserialize(&receipt_bytes)
            .map_err(|e| anyhow!("deserialize groth16: {e}"))?;
        receipt
            .verify_integrity_with_context(&Default::default())
            .map_err(|e| anyhow!("groth16 verify: {e}"))?;
        Ok(snark_id)
    }

    /// Compress a STARK to a Blake3-Groth16 SNARK and locally verify.
    /// Returns the backend's id of the compressed receipt.
    async fn compress_blake3(
        &self,
        primitive_proof_id: &str,
    ) -> Result<String, BackendProviderError> {
        let snark_id = self
            .snark_prover
            .compress_blake3_groth16(primitive_proof_id)
            .await
            .map_err(|e| anyhow!("blake3 compress: {e}"))?;
        let receipt_bytes = self
            .snark_prover
            .get_blake3_groth16_receipt(&snark_id)
            .await
            .map_err(|e| anyhow!("get blake3 receipt: {e}"))?
            .ok_or_else(|| anyhow!("blake3 receipt missing"))?;
        let receipt: Blake3Groth16Receipt =
            bincode::deserialize(&receipt_bytes).map_err(|e| anyhow!("deserialize blake3: {e}"))?;
        receipt.verify_integrity().map_err(|e| anyhow!("blake3 verify: {e}"))?;
        Ok(snark_id)
    }

    /// Fetch the compressed Groth16 receipt and encode it as on-chain seal bytes.
    async fn fetch_groth16_seal(&self, snark_id: &str) -> Result<Vec<u8>, BackendProviderError> {
        let receipt_bytes = self
            .snark_prover
            .get_compressed_receipt(snark_id)
            .await
            .map_err(|e| anyhow!("get groth16 receipt: {e}"))?
            .ok_or_else(|| anyhow!("groth16 receipt missing"))?;
        let receipt: Receipt = bincode::deserialize(&receipt_bytes)
            .map_err(|e| anyhow!("deserialize groth16: {e}"))?;
        encode_seal(&receipt).map_err(|e| anyhow!("encode groth16 seal: {e}").into())
    }

    /// Fetch the Blake3-Groth16 receipt and encode it as on-chain seal bytes,
    /// applying the dev-mode selector substitution where applicable.
    async fn fetch_blake3_seal(
        &self,
        snark_id: &str,
        selector: boundless_market::VerifierSelector,
    ) -> Result<Vec<u8>, BackendProviderError> {
        let receipt_bytes = self
            .snark_prover
            .get_blake3_groth16_receipt(snark_id)
            .await
            .map_err(|e| anyhow!("get blake3 receipt: {e}"))?
            .ok_or_else(|| anyhow!("blake3 receipt missing"))?;
        let receipt: Blake3Groth16Receipt =
            bincode::deserialize(&receipt_bytes).map_err(|e| anyhow!("deserialize blake3: {e}"))?;
        let mut seal =
            encode_seal(&receipt.into()).map_err(|e| anyhow!("encode blake3 seal: {e}"))?;
        if self.dev_mode {
            let fake = SelectorExt::FakeBlake3Groth16 as u32;
            seal.splice(0..4, fake.to_be_bytes());
        } else {
            seal.splice(0..4, selector.0);
        }
        Ok(seal)
    }
}

#[async_trait]
impl BackendProvider for RiscZeroBackend {
    async fn compress_proof(
        &self,
        _program_id: &ProgramId,
        primitive_proof_id: &str,
        selector: boundless_market::VerifierSelector,
    ) -> Result<String, BackendProviderError> {
        match Self::proof_type(selector) {
            ProofType::Groth16 => self.compress_groth16(primitive_proof_id).await,
            ProofType::Blake3Groth16 => self.compress_blake3(primitive_proof_id).await,
            _ => Err(BackendProviderError::UnsupportedSelector(selector)),
        }
    }

    async fn encode_seal(
        &self,
        compressed_proof_id: &str,
        selector: boundless_market::VerifierSelector,
    ) -> Result<Vec<u8>, BackendProviderError> {
        match Self::proof_type(selector) {
            ProofType::Groth16 => self.fetch_groth16_seal(compressed_proof_id).await,
            ProofType::Blake3Groth16 => self.fetch_blake3_seal(compressed_proof_id, selector).await,
            _ => Err(BackendProviderError::UnsupportedSelector(selector)),
        }
    }

    fn compute_claim_digest(
        &self,
        selector: boundless_market::VerifierSelector,
        program_id: &ProgramId,
        public_output: &PublicOutput,
    ) -> Result<[u8; 32], BackendProviderError> {
        Self::dispatch_claim_digest(selector, program_id, public_output)
    }
}

impl RiscZeroBackend {
    /// Selector-keyed claim-digest dispatch.
    ///
    /// - Groth16 / set-inclusion / unspecified: `ReceiptClaim::ok(image_id, journal).digest()`.
    /// - Blake3-Groth16 (incl. `FakeBlake3Groth16` dev variant):
    ///   `Blake3Groth16ReceiptClaim::ok` over a 32-byte commit. Non-32-byte
    ///   public outputs are rejected.
    pub(crate) fn dispatch_claim_digest(
        selector: boundless_market::VerifierSelector,
        program_id: &ProgramId,
        public_output: &PublicOutput,
    ) -> Result<[u8; 32], BackendProviderError> {
        match Self::proof_type(selector) {
            ProofType::Blake3Groth16 => {
                let commit: [u8; 32] = public_output.as_bytes().try_into().map_err(|_| {
                    anyhow!(
                        "Blake3-Groth16 selector requires a 32-byte public output (got {})",
                        public_output.as_bytes().len()
                    )
                })?;
                let claim = Blake3Groth16ReceiptClaim::ok(
                    R0Digest::from_bytes(*program_id.as_bytes()),
                    commit,
                );
                Ok(<[u8; 32]>::from(claim.claim_digest()))
            }
            _ => Ok(RiscZeroClaimDigest.compute(program_id.as_bytes(), public_output.as_bytes())),
        }
    }
}

/// RISC Zero's native claim-digest formula: `ReceiptClaim::ok(image_id, journal).digest()`.
///
/// Matches the digest `IRiscZeroVerifier.verifyIntegrity` expects on chain.
/// Used for every R0 selector except Blake3-Groth16, which has its own
/// formula and is dispatched by `RiscZeroBackend::compute_claim_digest`.
#[derive(Clone, Copy, Debug, Default)]
pub struct RiscZeroClaimDigest;

impl ComputeClaimDigest for RiscZeroClaimDigest {
    fn compute(&self, program_id: &[u8; 32], public_output: &[u8]) -> [u8; 32] {
        let image_id = R0Digest::from_bytes(*program_id);
        let claim = ReceiptClaim::ok(image_id, public_output.to_vec());
        <[u8; 32]>::from(claim.digest())
    }

    fn compute_from_output_digest(
        &self,
        program_id: &[u8; 32],
        output_digest: &[u8; 32],
    ) -> [u8; 32] {
        let image_id = R0Digest::from_bytes(*program_id);
        let journal_digest = R0Digest::from_bytes(*output_digest);
        let claim = ReceiptClaim::ok(image_id, MaybePruned::Pruned(journal_digest));
        <[u8; 32]>::from(claim.digest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boundless_market::selector::SelectorExt;
    use risc0_ethereum_contracts::selector::Selector as Sel;

    fn fb(v: u32) -> boundless_market::VerifierSelector {
        boundless_market::VerifierSelector::from(v.to_be_bytes())
    }

    #[test]
    fn groth16_selector_uses_native_r0_formula() {
        let pid = ProgramId::new([7u8; 32]);
        let journal = PublicOutput::from(b"journal-bytes".to_vec());

        let groth16_selector = fb(Sel::Groth16V3_0 as u32);
        let via_dispatch =
            RiscZeroBackend::dispatch_claim_digest(groth16_selector, &pid, &journal).unwrap();
        let via_native = RiscZeroClaimDigest.compute(pid.as_bytes(), journal.as_bytes());
        assert_eq!(via_dispatch, via_native, "Groth16 must use R0 native formula");
    }

    #[test]
    fn set_inclusion_unspecified_uses_native_r0_formula() {
        // Unspecified / set-inclusion selector falls through to R0 native.
        let pid = ProgramId::new([3u8; 32]);
        let journal = PublicOutput::from(b"set-inclusion-journal".to_vec());
        let unspecified = fb(0x0000_0000);
        let via_dispatch =
            RiscZeroBackend::dispatch_claim_digest(unspecified, &pid, &journal).unwrap();
        let via_native = RiscZeroClaimDigest.compute(pid.as_bytes(), journal.as_bytes());
        assert_eq!(via_dispatch, via_native);
    }

    #[test]
    fn blake3_selector_uses_blake3_formula() {
        // Blake3-Groth16 selector uses Blake3Groth16ReceiptClaim::ok
        // (NOT ReceiptClaim::ok); the digests must differ.
        let pid = ProgramId::new([5u8; 32]);
        let commit_bytes = [42u8; 32];
        let journal = PublicOutput::from(commit_bytes.to_vec());

        let blake3_selector = fb(SelectorExt::Blake3Groth16V0_1 as u32);
        let via_dispatch =
            RiscZeroBackend::dispatch_claim_digest(blake3_selector, &pid, &journal).unwrap();
        let via_native_r0 = RiscZeroClaimDigest.compute(pid.as_bytes(), journal.as_bytes());
        assert_ne!(
            via_dispatch, via_native_r0,
            "Blake3 dispatch must NOT collapse to the R0 native formula"
        );

        // Sanity: matches the direct Blake3 formula.
        let direct = {
            let claim =
                Blake3Groth16ReceiptClaim::ok(R0Digest::from_bytes(*pid.as_bytes()), commit_bytes);
            <[u8; 32]>::from(claim.claim_digest())
        };
        assert_eq!(via_dispatch, direct);
    }

    #[test]
    fn fake_blake3_selector_routes_to_blake3_formula() {
        // FakeBlake3Groth16 dev variant must take the Blake3 path too,
        // not fall through to R0 native.
        let pid = ProgramId::new([9u8; 32]);
        let commit_bytes = [1u8; 32];
        let journal = PublicOutput::from(commit_bytes.to_vec());
        let fake = fb(SelectorExt::FakeBlake3Groth16 as u32);
        let via_dispatch = RiscZeroBackend::dispatch_claim_digest(fake, &pid, &journal).unwrap();
        let via_native_r0 = RiscZeroClaimDigest.compute(pid.as_bytes(), journal.as_bytes());
        assert_ne!(via_dispatch, via_native_r0);
    }

    #[test]
    fn blake3_rejects_non_32_byte_journal() {
        // Spec: Blake3-Groth16 wrapped guests must commit exactly 32 bytes.
        // Non-conforming public output produces an error, not a silently
        // wrong digest.
        let pid = ProgramId::new([0u8; 32]);
        let blake3_selector = fb(SelectorExt::Blake3Groth16V0_1 as u32);

        // Too short.
        let short = PublicOutput::from(vec![0u8; 16]);
        assert!(
            RiscZeroBackend::dispatch_claim_digest(blake3_selector, &pid, &short).is_err(),
            "16-byte journal must be rejected"
        );

        // Too long.
        let long = PublicOutput::from(vec![0u8; 64]);
        assert!(
            RiscZeroBackend::dispatch_claim_digest(blake3_selector, &pid, &long).is_err(),
            "64-byte journal must be rejected"
        );

        // Exactly 32 bytes is fine.
        let ok = PublicOutput::from(vec![0u8; 32]);
        assert!(RiscZeroBackend::dispatch_claim_digest(blake3_selector, &pid, &ok).is_ok());
    }
}
