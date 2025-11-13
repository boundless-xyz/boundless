use anyhow::{Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use derive_more::Debug;
pub use risc0_groth16::{ProofJson as Groth16ProofJson, Seal as Groth16Seal};
use risc0_zkvm::{Digest, InnerReceipt, MaybePruned, Receipt, ReceiptClaim};
use serde::{Deserialize, Serialize};

use crate::Blake3Groth16ReceiptClaim;

#[cfg(feature = "prove")]
use {risc0_zkvm::sha::Digestible, risc0_zkvm::Groth16Receipt};

#[derive(Clone, Debug, Deserialize, Serialize, BorshSerialize, BorshDeserialize)]
pub struct Blake3Groth16Receipt {
    pub journal: [u8; 32],
    #[debug("{} bytes", seal.len())]
    pub seal: Vec<u8>,
    pub claim: MaybePruned<Blake3Groth16ReceiptClaim>,
    pub verifier_parameters: Digest,
}

impl Blake3Groth16Receipt {
    fn new(
        journal: [u8; 32],
        seal: Vec<u8>,
        claim: MaybePruned<Blake3Groth16ReceiptClaim>,
        verifier_parameters: Digest,
    ) -> Self {
        Self { journal, seal, claim, verifier_parameters }
    }
    /// Verifies the BLAKE3 Groth16Seal against the BLAKE3 claim digest and wraps it in a Receipt.
    #[cfg(feature = "prove")]
    pub(crate) fn finalize(
        receipt_claim: MaybePruned<ReceiptClaim>,
        seal: Vec<u8>,
    ) -> Result<Self> {
        let receipt_claim_value =
            receipt_claim.as_value().context("receipt claim must not be pruned")?.clone();

        let blake3_claim = Blake3Groth16ReceiptClaim::try_from(receipt_claim_value)?;
        let journal: [u8; 32] = blake3_claim
            .journal
            .as_slice()
            .try_into()
            .context("invalid journal length, expected 32 bytes for blake3 groth16")?;

        let blake3_claim_digest = blake3_claim.digest();
        crate::verify::verify_seal(&seal, blake3_claim_digest)?;

        let verifier_parameters = crate::verify::verifier_parameters();

        let receipt = Blake3Groth16Receipt::new(
            journal,
            seal,
            blake3_claim.into(),
            verifier_parameters.digest(),
        );
        Ok(receipt)
    }

    /// Verify that this receipt proves a successful execution of the zkVM from
    /// the given `image_id`.
    pub fn verify(&self, image_id: impl Into<Digest>) -> Result<()> {
        self.verify_with_context(&crate::verify::verifier_parameters(), image_id)
    }

    /// Verify that this receipt proves a successful execution of the zkVM from the given
    /// `image_id`.
    pub fn verify_with_context(
        &self,
        params: &risc0_zkvm::Groth16ReceiptVerifierParameters,
        image_id: impl Into<Digest>,
    ) -> Result<()> {
        self.verify_integrity_with_context(params)?;

        let expected_claim = Blake3Groth16ReceiptClaim::ok(image_id, self.journal.to_vec());
        if self.claim.digest() != expected_claim.digest() {
            tracing::debug!("blake3 receipt claim does not match expected claim:\nreceipt: {:#?}\nexpected: {:#?}", expected_claim.digest(), self.claim.digest());
            return Err(anyhow::anyhow!(
                "blake3 groth16 claim digest mismatch: 
                expected: {},
                received: {},
            ",
                expected_claim.digest(),
                self.claim.digest()
            ));
        }

        Ok(())
    }

    /// Verify the integrity of this receipt, ensuring the claim and journal
    /// are attested to by the seal.
    pub fn verify_integrity(&self) -> Result<()> {
        let params = crate::verify::verifier_parameters();
        self.verify_integrity_with_context(&params)
    }

    /// Verify the integrity of this receipt, ensuring the claim and journal
    /// are attested to by the seal.
    pub fn verify_integrity_with_context(
        &self,
        params: &risc0_zkvm::Groth16ReceiptVerifierParameters,
    ) -> Result<()> {
        if params.digest() != self.verifier_parameters {
            return Err(anyhow::anyhow!(
                "verifier parameters digest mismatch: 
                expected: {},
                received: {},
            ",
                params.digest(),
                self.verifier_parameters
            ));
        }
        if self.journal.as_slice()
            != self.claim.as_value().context("blake3 claim must not be pruned")?.journal.as_slice()
        {
            return Err(anyhow::anyhow!("journal in receipt does not match journal in claim"));
        }
        crate::verify::verify_seal(&self.seal, self.claim.digest())?;
        Ok(())
    }
}

impl From<Blake3Groth16Receipt> for Receipt {
    fn from(value: Blake3Groth16Receipt) -> Self {
        Receipt::new(
            InnerReceipt::Groth16(Groth16Receipt::new(
                value.seal,
                MaybePruned::Pruned(value.claim.digest()),
                value.verifier_parameters,
            )),
            value.journal.to_vec(),
        )
    }
}

impl TryFrom<Receipt> for Blake3Groth16Receipt {
    type Error = anyhow::Error;

    fn try_from(receipt: Receipt) -> Result<Self, Self::Error> {
        let claim =
            receipt.claim()?.as_value().context("receipt claim must not be pruned")?.clone();
        if let InnerReceipt::Groth16(groth16_receipt) = receipt.inner {
            let blake3_claim = Blake3Groth16ReceiptClaim::try_from(claim)?;
            let journal: [u8; 32] = receipt
                .journal
                .bytes
                .as_slice()
                .try_into()
                .context("invalid journal length, expected 32 bytes for blake3 groth16")?;
            Ok(Blake3Groth16Receipt::new(
                journal,
                groth16_receipt.seal,
                blake3_claim.into(),
                groth16_receipt.verifier_parameters,
            ))
        } else {
            Err(anyhow::anyhow!("Expected Groth16 InnerReceipt, found different variant"))
        }
    }
}
