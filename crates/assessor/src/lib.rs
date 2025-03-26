// Copyright 2025 RISC Zero, Inc.
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

//! Assessor is a guest that verifies the fulfillment of a request.

#![deny(missing_docs)]

use alloy_primitives::{Address, PrimitiveSignature};
use alloy_sol_types::{Eip712Domain, SolStruct};
use anyhow::{bail, Result};
use boundless_market::contracts::{EIP721DomainSaltless, ProofRequest};
use risc0_zkvm::{sha::Digest, ReceiptClaim};
use serde::{Deserialize, Serialize};

/// Fulfillment contains a signed request, including offer and requirements,
/// that the prover has completed, and the journal committed
/// into the Merkle tree of the aggregated set of proofs.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Fulfillment {
    /// The request that was fulfilled.
    pub request: ProofRequest,
    /// The EIP-712 signature over the request.
    pub signature: Vec<u8>,
    /// The journal of the request.
    pub journal: Vec<u8>,
}

impl Fulfillment {
    // TODO: Change this to use a thiserror error type.
    /// Verifies the signature of the request.
    pub fn verify_signature(&self, domain: &Eip712Domain) -> Result<[u8; 32]> {
        let hash = self.request.eip712_signing_hash(domain);
        let signature = PrimitiveSignature::try_from(self.signature.as_slice())?;
        // NOTE: This could be optimized by accepting the public key as input, checking it against
        // the address, and using it to verify the signature instead of recovering the
        // public key. It would save ~1M cycles.
        let recovered = signature.recover_address_from_prehash(&hash)?;
        let client_addr = self.request.client_address()?;
        if recovered != self.request.client_address()? {
            bail!("Invalid signature: mismatched addr {recovered} - {client_addr}");
        }
        Ok(hash.into())
    }
    /// Evaluates the requirements of the request.
    pub fn evaluate_requirements(&self) -> Result<()> {
        if !self.request.requirements.predicate.eval(&self.journal) {
            bail!("Predicate evaluation failed");
        }
        Ok(())
    }
    /// Returns a [ReceiptClaim] for the fulfillment.
    pub fn receipt_claim(&self) -> ReceiptClaim {
        let image_id = Digest::from_bytes(self.request.requirements.imageId.0);
        ReceiptClaim::ok(image_id, self.journal.clone())
    }
}

/// Input of the Assessor guest.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AssessorInput {
    /// List of fulfillments that the prover has completed.
    pub fills: Vec<Fulfillment>,
    /// EIP-712 domain checking the signature of the request.
    ///
    /// The EIP-712 domain contains the chain ID and smart contract address.
    /// This smart contract address is used solely to construct the EIP-712 Domain
    /// and complete signature checks on the requests.
    pub domain: EIP721DomainSaltless,
    /// The address of the prover.
    pub prover_address: Address,
}

impl AssessorInput {
    /// Serializes the AssessorInput to a Vec<u8> using postcard.
    pub fn to_vec(&self) -> Vec<u8> {
        let bytes = postcard::to_allocvec(self).unwrap();
        let length = bytes.len() as u32;
        let mut result = Vec::with_capacity(4 + bytes.len());
        result.extend_from_slice(&length.to_le_bytes());
        result.extend_from_slice(&bytes);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        primitives::{Address, B256, U256},
        signers::local::PrivateKeySigner,
    };
    use boundless_market::contracts::{
        eip712_domain, Input, InputType, Offer, Predicate, PredicateType, ProofRequest,
        Requirements,
    };
    use guest_assessor::ASSESSOR_GUEST_ELF;
    use guest_util::{ECHO_ELF, ECHO_ID};
    use risc0_zkvm::{
        default_executor,
        sha::{Digest, Digestible},
        ExecutorEnv, ExitCode, FakeReceipt, InnerReceipt, MaybePruned, Receipt,
    };

    fn proving_request(id: u32, signer: Address, image_id: B256, prefix: Vec<u8>) -> ProofRequest {
        ProofRequest::new(
            id,
            &signer,
            Requirements::new(
                Digest::from_bytes(image_id.0),
                Predicate { predicateType: PredicateType::PrefixMatch, data: prefix.into() },
            ),
            "test",
            Input { inputType: InputType::Url, data: Default::default() },
            Offer {
                minPrice: U256::from(1),
                maxPrice: U256::from(10),
                biddingStart: 1741386831,
                timeout: 1000,
                rampUpPeriod: 1,
                lockTimeout: 1000,
                lockStake: U256::from(0),
            },
        )
    }

    fn to_b256(digest: Digest) -> B256 {
        <[u8; 32]>::from(digest).into()
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_claim() {
        let signer = PrivateKeySigner::random();
        let proving_request = proving_request(1, signer.address(), B256::ZERO, vec![1]);
        let signature = proving_request.sign_request(&signer, Address::ZERO, 1).await.unwrap();

        let claim = Fulfillment {
            request: proving_request,
            signature: signature.as_bytes().to_vec(),
            journal: vec![1, 2, 3],
        };

        claim.verify_signature(&eip712_domain(Address::ZERO, 1).alloy_struct()).unwrap();
        claim.evaluate_requirements().unwrap();
    }

    #[test]
    #[test_log::test]
    fn test_domain_serde() {
        let domain = eip712_domain(Address::ZERO, 1);
        let bytes = postcard::to_allocvec(&domain).unwrap();
        let domain2: EIP721DomainSaltless = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(domain, domain2);
    }

    async fn setup_proving_request_and_signature(
        signer: &PrivateKeySigner,
    ) -> (ProofRequest, Vec<u8>) {
        let request = proving_request(
            1,
            signer.address(),
            to_b256(ECHO_ID.into()),
            "test".as_bytes().to_vec(),
        );
        let signature =
            request.sign_request(signer, Address::ZERO, 1).await.unwrap().as_bytes().to_vec();
        (request, signature)
    }

    fn echo(input: &str) -> Receipt {
        let env = ExecutorEnv::builder().write_slice(input.as_bytes()).build().unwrap();

        // TODO: Change this to use SessionInfo::claim or another method.
        // See https://github.com/risc0/risc0/issues/2267.
        let session = default_executor().execute(env, ECHO_ELF).unwrap();
        Receipt::new(
            InnerReceipt::Fake(FakeReceipt::new(ReceiptClaim::ok(
                ECHO_ID,
                MaybePruned::Pruned(session.journal.digest()),
            ))),
            session.journal.bytes,
        )
    }

    fn assessor(claims: Vec<Fulfillment>, receipts: Vec<Receipt>) {
        let assessor_input = AssessorInput {
            domain: eip712_domain(Address::ZERO, 1),
            fills: claims,
            prover_address: Address::ZERO,
        };
        let mut env_builder = ExecutorEnv::builder();
        env_builder.write_slice(&assessor_input.to_vec());
        for receipt in receipts {
            env_builder.add_assumption(receipt);
        }
        let env = env_builder.build().unwrap();
        let session = default_executor().execute(env, ASSESSOR_GUEST_ELF).unwrap();
        assert_eq!(session.exit_code, ExitCode::Halted(0));
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_singleton() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();

        // 3. Prove the Assessor
        let claims = vec![Fulfillment { request, signature, journal }];
        assessor(claims, vec![application_receipt]);
    }

    #[tokio::test]
    #[test_log::test]
    async fn test_assessor_e2e_two_leaves() {
        let signer = PrivateKeySigner::random();
        // 1. Mock and sign a request
        let (request, signature) = setup_proving_request_and_signature(&signer).await;

        // 2. Prove the request via the application guest
        let application_receipt = echo("test");
        let journal = application_receipt.journal.bytes.clone();
        let claim = Fulfillment { request, signature, journal };

        // 3. Prove the Assessor reusing the same leaf twice
        let claims = vec![claim.clone(), claim];
        assessor(claims, vec![application_receipt.clone(), application_receipt]);
    }
}
