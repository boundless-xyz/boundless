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

extern crate alloc;

use alloc::vec::Vec;
use anyhow::{bail, Context, Result};
use borsh::{BorshDeserialize, BorshSerialize};
pub use risc0_groth16::{ProofJson as Groth16ProofJson, Seal as Groth16Seal};
use risc0_zkvm::{
    sha::Digestible, Digest, Groth16Receipt, InnerReceipt, MaybePruned, Receipt, ReceiptClaim,
};
use serde::{Deserialize, Serialize};

use crate::{is_dev_mode, Blake3Groth16ReceiptClaim};

#[derive(Clone, Debug, Deserialize, Serialize, BorshSerialize, BorshDeserialize)]
pub struct Blake3Groth16Receipt {
    journal: [u8; 32],
    claim: MaybePruned<Blake3Groth16ReceiptClaim>,
    inner: InnerReceipt,
}

impl Blake3Groth16Receipt {
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
        if self.claim_digest()? != expected_claim.digest() {
            tracing::debug!("blake3 receipt claim does not match expected claim:\nreceipt: {:#?}\nexpected: {:#?}", expected_claim.digest(), self.claim.digest());
            return Err(anyhow::anyhow!(
                "blake3 groth16 claim digest mismatch: 
                expected: {},
                received: {},
            ",
                expected_claim.digest(),
                self.claim_digest()?
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
        if is_dev_mode() {
            return Ok(());
        }
        if params.digest() != self.inner.verifier_parameters() {
            return Err(anyhow::anyhow!(
                "verifier parameters digest mismatch: 
                expected: {},
                received: {},
            ",
                params.digest(),
                self.inner.verifier_parameters()
            ));
        }
        if self.journal.as_slice()
            != self.claim.as_value().context("blake3 claim must not be pruned")?.journal.as_slice()
        {
            return Err(anyhow::anyhow!("journal in receipt does not match journal in claim"));
        }
        crate::verify::verify_seal(self.seal()?, self.claim.digest())?;
        Ok(())
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
        let groth16_receipt =
            Groth16Receipt::new(seal.to_vec(), receipt_claim, verifier_parameters.digest());
        let inner = InnerReceipt::Groth16(groth16_receipt);
        let receipt = Blake3Groth16Receipt { journal, claim: blake3_claim.into(), inner };
        Ok(receipt)
    }

    pub fn claim_digest(&self) -> Result<Digest> {
        if is_dev_mode() {
            if let InnerReceipt::Fake(fake_receipt) = &self.inner {
                return Ok(fake_receipt.claim.digest());
            } else {
                bail!(
                    "RISC0_DEV_MODE blake3_groth16 claim digest can only be used on fake receipts"
                );
            }
        }
        Ok(self.claim.digest())
    }

    fn groth16(&self) -> Result<&Groth16Receipt<ReceiptClaim>> {
        if let InnerReceipt::Groth16(groth16_receipt) = &self.inner {
            Ok(groth16_receipt)
        } else {
            Err(anyhow::anyhow!("Expected Groth16 InnerReceipt, found different variant"))
        }
    }

    pub fn seal(&self) -> Result<&Vec<u8>> {
        Ok(&self.groth16()?.seal)
    }
}

impl From<Blake3Groth16Receipt> for Receipt {
    fn from(value: Blake3Groth16Receipt) -> Self {
        if is_dev_mode() {
            Receipt::new(
                InnerReceipt::Fake(risc0_zkvm::FakeReceipt::new(MaybePruned::Pruned(
                    value.claim.digest(),
                ))),
                value.journal.to_vec(),
            )
        } else {
            Receipt::new(value.inner, value.journal.to_vec())
        }
    }
}

impl TryFrom<Receipt> for Blake3Groth16Receipt {
    type Error = anyhow::Error;

    fn try_from(receipt: Receipt) -> Result<Self, Self::Error> {
        let journal: [u8; 32] = receipt
            .journal
            .bytes
            .as_slice()
            .try_into()
            .context("invalid journal length, expected 32 bytes for blake3 groth16")?;

        if is_dev_mode() {
            println!("RISC0_DEV_MODE is set, skipping actual blake3 groth16 compression and returning fake receipt");
            if let InnerReceipt::Fake(fake_receipt) = &receipt.inner {
                return Ok(Blake3Groth16Receipt {
                    journal,
                    claim: MaybePruned::Pruned(fake_receipt.claim.digest()),
                    inner: receipt.inner,
                });
            } else {
                return Err(anyhow::anyhow!(
                    "RISC0_DEV_MODE blake3_groth16 compression can only be used on fake receipts"
                ));
            }
        }
        if let InnerReceipt::Groth16(groth16_receipt) = receipt.inner {
            let claim = Blake3Groth16ReceiptClaim::try_from(
                groth16_receipt
                    .claim
                    .as_value()
                    .context("receipt claim must not be pruned")?
                    .clone(),
            )?
            .clone();
            Ok(Blake3Groth16Receipt {
                journal,
                claim: claim.into(),
                inner: InnerReceipt::Groth16(groth16_receipt),
            })
        } else {
            Err(anyhow::anyhow!("Expected Groth16 InnerReceipt, found different variant"))
        }
    }
}
